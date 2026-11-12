//! Test the same bounded pin authority table used by kernel SYS_MAP/SYS_UNMAP.
#[path = "../../kernel/src/dma_pins.rs"]
mod dma_pins;
use aether_core::accel::SliceMem;
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::MapRequest;
use aether_core::types::{PhysAddr, TenantId};
use aether_drivers::SoftNpuDevice;
use aether_hal::HalError;
use dma_pins::DmaPins;

fn cap(tenant: u32, rights: CapRights) -> Capability {
    Capability::new(CapKind::Memory, rights, 1, TenantId(tenant)).with_generation(1)
}
fn device(bytes: &mut [u8]) -> SoftNpuDevice<SliceMem<'_>> {
    SoftNpuDevice::new(SliceMem {
        base: PhysAddr(0),
        bytes,
    })
}

#[test]
fn every_tracked_pin_can_be_unmapped_without_authorizing_boot_or_interior_addresses() {
    for reverse in [false, true] {
        let mut bytes = [0u8; 1];
        let mut npu = device(&mut bytes);
        let owner = cap(1, CapRights::MEM_FULL);
        let boot = npu
            .map_with_cap(&owner, MapRequest::pin(PhysAddr(0x8000), 64))
            .unwrap();
        let mut pins = DmaPins::<2>::new();
        let a = pins.map(&mut npu, &owner, PhysAddr(0), 64).unwrap();
        let b = pins.map(&mut npu, &owner, PhysAddr(4096), 64).unwrap();
        for address in [boot, PhysAddr(a.0 + 4), PhysAddr(0)] {
            assert_eq!(pins.unmap(&mut npu, address), Err(HalError::Fault));
        }
        let [first, second] = if reverse { [b, a] } else { [a, b] };
        pins.unmap(&mut npu, first).unwrap();
        assert!(npu.iommu.resolve(first).is_none());
        assert!(npu.iommu.resolve(second).is_some());
        assert_eq!(pins.unmap(&mut npu, first), Err(HalError::Fault));
        pins.unmap(&mut npu, second).unwrap();
        assert!(pins.is_empty());
        assert!(
            npu.iommu.resolve(boot).is_some(),
            "boot pin remains separate"
        );
    }
}

#[test]
fn full_tracking_table_refuses_before_pinning_and_reuses_a_released_slot() {
    let mut bytes = [0u8; 1];
    let mut npu = device(&mut bytes);
    let owner = cap(1, CapRights::MEM_FULL);
    let mut pins = DmaPins::<2>::new();
    let a = pins.map(&mut npu, &owner, PhysAddr(0), 64).unwrap();
    let b = pins.map(&mut npu, &owner, PhysAddr(4096), 64).unwrap();
    assert_eq!(
        pins.map(&mut npu, &owner, PhysAddr(8192), 64),
        Err(HalError::Busy)
    );
    assert_eq!(npu.iommu.iter().count(), 2);
    assert!(npu.iommu.translate(PhysAddr(8192)).is_none());
    pins.unmap(&mut npu, a).unwrap();
    let c = pins.map(&mut npu, &owner, PhysAddr(8192), 64).unwrap();
    assert_eq!(npu.iommu.iter().count(), 2);
    pins.unmap(&mut npu, b).unwrap();
    pins.unmap(&mut npu, c).unwrap();
    assert!(pins.is_empty());
    assert!(npu.iommu.is_empty());
}

#[test]
fn failed_maps_preserve_prior_pins_and_leave_capacity_for_retry() {
    let mut bytes = [0u8; 1];
    let mut npu = device(&mut bytes);
    let owner = cap(1, CapRights::MEM_FULL);
    let mut pins = DmaPins::<2>::new();
    let a = pins.map(&mut npu, &owner, PhysAddr(0), 64).unwrap();
    assert_eq!(
        pins.map(&mut npu, &owner, PhysAddr(0), 64),
        Err(HalError::BadArg)
    );
    assert_eq!(
        pins.map(
            &mut npu,
            &cap(1, CapRights(CapRights::READ)),
            PhysAddr(4096),
            64
        ),
        Err(HalError::NoMemoryCap)
    );
    assert_eq!(
        pins.map(&mut npu, &cap(2, CapRights::MEM_FULL), PhysAddr(4096), 64),
        Err(HalError::Fault)
    );
    assert_eq!(npu.iommu.iter().count(), 1);
    let b = pins.map(&mut npu, &owner, PhysAddr(4096), 64).unwrap();
    pins.unmap(&mut npu, a).unwrap();
    pins.unmap(&mut npu, b).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn refused_device_unmap_keeps_authority_for_a_corrected_retry() {
    let mut bytes = [0u8; 1];
    let mut npu = device(&mut bytes);
    let mut pins = DmaPins::<2>::new();
    let a = pins
        .map(&mut npu, &cap(1, CapRights::MEM_FULL), PhysAddr(0), 64)
        .unwrap();
    // Explicit public-table replacement fault control, not ordinary unmap/map.
    let saved = core::mem::replace(&mut npu.iommu, aether_core::iommu::IommuMap::new());
    assert_eq!(pins.unmap(&mut npu, a), Err(HalError::Fault));
    assert!(!pins.is_empty());
    npu.iommu = saved;
    pins.unmap(&mut npu, a).unwrap();
    assert!(pins.is_empty());
}
