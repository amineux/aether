//! Exercise the same preparation function that gates kernel World publication.
#[path = "../../kernel/src/accel_boot.rs"]
mod accel_boot;

use accel_boot::prepare;
use aether_core::accel::SliceMem;
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::DEFAULT_STREAM;
use aether_core::types::{PhysAddr, TenantId};
use aether_drivers::mmio::REG_MAGIC;
use aether_drivers::SoftNpuDevice;
use aether_hal::HalError;

fn cap(rights: CapRights) -> Capability {
    Capability::new(CapKind::Memory, rights, 1, TenantId(1)).with_generation(1)
}
fn device(bytes: &mut [u8]) -> SoftNpuDevice<SliceMem<'_>> {
    SoftNpuDevice::new(SliceMem {
        base: PhysAddr(0),
        bytes,
    })
}

#[test]
fn failed_probe_cannot_pin_the_user_image() {
    let mut bytes = [0u8; 1];
    let mut npu = device(&mut bytes);
    npu.mmio.write_u32(REG_MAGIC, 0);
    assert_eq!(
        prepare(&mut npu, &cap(CapRights::MEM_FULL), PhysAddr(0), 256),
        Err(HalError::NotFound)
    );
    assert!(npu.iommu.is_empty());
    assert!(!npu.doorbell_pending());
}

#[test]
fn failed_pins_report_failure_and_a_corrected_retry_succeeds() {
    for (rights, base, len, expected) in [
        (CapRights(CapRights::READ), 0, 256, HalError::NoMemoryCap),
        (CapRights::MEM_FULL, 0, 0, HalError::BadArg),
        (CapRights::MEM_FULL, u64::MAX - 7, 16, HalError::BadArg),
    ] {
        let mut bytes = [0u8; 1];
        let mut npu = device(&mut bytes);
        assert_eq!(
            prepare(&mut npu, &cap(rights), PhysAddr(base), len),
            Err(expected)
        );
        assert!(npu.iommu.is_empty());
        let iova = prepare(&mut npu, &cap(CapRights::MEM_FULL), PhysAddr(0), 256).unwrap();
        assert_eq!(
            npu.iommu
                .resolve_range(DEFAULT_STREAM, iova, 256, Some(TenantId(1))),
            Ok(PhysAddr(0))
        );
    }
}

#[test]
fn each_architecture_image_gets_a_full_nonidentity_translation() {
    use aether_core::{
        USER_AA_IMAGE_BASE, USER_AA_IMAGE_END, USER_IMAGE_BASE, USER_IMAGE_END, USER_RV_IMAGE_BASE,
        USER_RV_IMAGE_END,
    };
    for (base, end) in [
        (USER_IMAGE_BASE, USER_IMAGE_END),
        (USER_RV_IMAGE_BASE, USER_RV_IMAGE_END),
        (USER_AA_IMAGE_BASE, USER_AA_IMAGE_END),
    ] {
        let mut bytes = [0u8; 1]; // Preparation never accesses tensor memory.
        let mut npu = device(&mut bytes);
        let iova = prepare(
            &mut npu,
            &cap(CapRights::MEM_FULL),
            PhysAddr(base),
            end - base,
        )
        .unwrap();
        assert_ne!(iova.0, base);
        assert!(npu
            .iommu
            .covers_stream(DEFAULT_STREAM, PhysAddr(base), end - base));
        assert_eq!(
            npu.iommu
                .resolve_range(DEFAULT_STREAM, iova, end - base, Some(TenantId(1))),
            Ok(PhysAddr(base))
        );
        assert!(!npu.doorbell_pending());
    }
}
