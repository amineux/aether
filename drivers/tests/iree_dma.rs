//! Real IREE-shaped software dispatch, with DMA counters and narrow pins.
use aether_core::accel::{AccelError, AccelJobDesc, DType, DmaView, SliceMem};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::MapRequest;
use aether_core::types::{PhysAddr, TenantId};
use aether_drivers::ireecp::{stream_for_job, IreeHalCmd, IreeHalNouns, IreeShapedCp, HAL_FN_ADD};
use aether_hal::{AccelDevice, HalError};
use core::cell::Cell;

struct Counted<'a> {
    inner: SliceMem<'a>,
    reads: Cell<usize>,
    writes: usize,
}
impl DmaView for Counted<'_> {
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, AccelError> {
        self.reads.set(self.reads.get() + 1);
        self.inner.load_i32(addr)
    }
    fn store_i32(&mut self, addr: PhysAddr, val: i32) -> Result<(), AccelError> {
        self.writes += 1;
        self.inner.store_i32(addr, val)
    }
    fn load_u16(&self, addr: PhysAddr) -> Result<u16, AccelError> {
        self.reads.set(self.reads.get() + 1);
        self.inner.load_u16(addr)
    }
    fn store_u16(&mut self, addr: PhysAddr, val: u16) -> Result<(), AccelError> {
        self.writes += 1;
        self.inner.store_u16(addr, val)
    }
}
fn cap() -> Capability {
    Capability::new(CapKind::Memory, CapRights::MEM_FULL, 4, TenantId(1)).with_generation(1)
}
fn fixture(backing: &mut [u8; 256], wave: bool) -> (IreeShapedCp<Counted<'_>>, AccelJobDesc) {
    backing.fill(0x7f);
    for (i, v) in [1i32, 2, 3, 4, 5, 6, 7, 8].iter().enumerate() {
        backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    backing[80..88].fill(0);
    let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
    if wave {
        job.op = aether_core::accel::AccelOp::Wave;
        job.bias = PhysAddr(80);
    }
    let mem = Counted {
        inner: SliceMem {
            base: PhysAddr(0),
            bytes: backing,
        },
        reads: Cell::new(0),
        writes: 0,
    };
    let mut d = IreeShapedCp::new(mem);
    for (base, len) in [(0, 16), (16, 16), (32, 16), (80, 8)] {
        d.map_with_cap(
            &cap(),
            MapRequest::pin_accel(PhysAddr(base), len, stream_for_job(&job)),
        )
        .unwrap();
    }
    (d, job)
}
fn no_dma(d: &IreeShapedCp<Counted<'_>>) {
    assert_eq!(d.mem.reads.get(), 0);
    assert_eq!(d.mem.writes, 0);
}

#[test]
fn direct_packet_metadata_and_binding_tampering_refuses_before_dma() {
    let mut backing = [0u8; 256];
    let (mut d, job) = fixture(&mut backing, true);
    let before = d.mem.inner.bytes.to_vec();
    let good = IreeHalCmd::pack(&job, &d.iommu).unwrap();
    let changes: &[fn(&mut IreeHalCmd)] = &[
        |c| c.workgroup_count_x += 1,
        |c| c.workgroup_count_y += 1,
        |c| c.workgroup_count_z += 1,
        |c| c.element_type ^= 1,
        |c| c.queue_affinity ^= 1,
        |c| c.binding_count = 3,
        |c| c.binding_count = 5,
        |c| c.binding0_offset += 4,
        |c| c.binding1_offset += 4,
        |c| c.binding2_offset += 4,
        |c| c.binding3_offset = 0,
        |c| c.binding0_length = 4,
        |c| c.binding1_length = 4,
        |c| c.binding2_length = 4,
        |c| c.binding3_length = 4,
        |c| c.binding2_length = 32,
    ];
    for change in changes {
        let mut bad = good;
        change(&mut bad);
        assert_eq!(d.submit_hal(bad, &job), Err(HalError::Fault));
        assert!(!d.doorbell_pending());
        assert!(!d.irq_pending());
        assert!(d.last_cmd().is_none());
        assert!(d.iommu.submit_sid().is_none());
        assert!(d.service().is_none());
        no_dma(&d);
        assert_eq!(d.mem.inner.bytes, &before);
    }
    d.submit_hal(good, &job).unwrap();
    assert_eq!(d.service().unwrap().status, 0);
}

#[test]
fn mismatched_host_template_cannot_expand_the_copied_packet() {
    let mut backing = [0u8; 256];
    let (mut d, job) = fixture(&mut backing, false);
    let before = d.mem.inner.bytes.to_vec();
    let cmd = IreeHalCmd::pack(&job, &d.iommu).unwrap();
    let changes: &[fn(&mut AccelJobDesc)] = &[
        |j| j.m = 3,
        |j| j.n = 3,
        |j| j.k = 3,
        |j| j.dtype = DType::F16,
        |j| j.a = PhysAddr(4),
        |j| j.c = PhysAddr(36),
        |j| j.bias = PhysAddr(80),
        |j| j.place = j.place.with_tile(1),
    ];
    for change in changes {
        let mut hidden = job;
        change(&mut hidden);
        assert!(d.submit_hal(cmd, &hidden).is_err());
        assert!(!d.doorbell_pending());
        assert!(d.iommu.submit_sid().is_none());
        no_dma(&d);
        assert_eq!(d.mem.inner.bytes, &before);
    }
    d.submit_hal(cmd, &job).unwrap();
    let cpl = d.service().unwrap();
    assert_eq!((cpl.status, cpl.job_seq), (0, 1));
    assert_eq!(
        i32::from_le_bytes(d.mem.inner.bytes[32..36].try_into().unwrap()),
        19
    );
    assert_eq!(&d.mem.inner.bytes[48..56], &[0x7f; 8]);
}

#[test]
fn queued_unmap_and_same_iova_short_pins_revalidate_every_full_operand() {
    for base in [0, 16, 32, 80] {
        for remap in [false, true] {
            let mut backing = [0u8; 256];
            let (mut d, job) = fixture(&mut backing, true);
            let before = d.mem.inner.bytes.to_vec();
            let sid = stream_for_job(&job);
            let iova = d.iommu.translate_stream(sid.raw(), PhysAddr(base)).unwrap();
            let full_table = d.iommu.clone();
            d.submit(&job).unwrap();
            d.unmap_with_cap(&cap(), iova).unwrap();
            if remap {
                // Replace the public software map with a shorter pin at the same
                // IOVA. Ordinary unmap/map allocates a new IOVA and is weaker.
                d.iommu = aether_core::iommu::IommuMap::new();
                for (pa, size) in [(0, 16), (16, 16), (32, 16), (80, 8)] {
                    d.map_with_cap(
                        &cap(),
                        MapRequest::pin_accel(PhysAddr(pa), if pa == base { 4 } else { size }, sid),
                    )
                    .unwrap();
                }
                d.iommu.set_sid_bound(sid).unwrap();
                assert_eq!(
                    d.iommu.translate_stream(sid.raw(), PhysAddr(base)),
                    Some(iova)
                );
                assert!(
                    d.iommu.resolve_stream(sid.raw(), iova).is_some(),
                    "base alone remains resolvable"
                );
            }
            let cpl = d.service().unwrap();
            assert_eq!((cpl.status, cpl.job_seq), (-2, 1));
            assert!(d.iommu.submit_sid().is_none());
            no_dma(&d);
            assert_eq!(d.mem.inner.bytes, &before);
            assert_eq!(d.poll().unwrap().status, -2);
            d.iommu = full_table;
            d.submit(&job).unwrap();
            let retry = d.service().unwrap();
            assert_eq!((retry.status, retry.job_seq), (0, 1));
        }
    }
}

#[test]
fn image_operation_and_custom_semaphore_payload_remain_authoritative() {
    let mut backing = [0u8; 256];
    let (mut d, mut job) = fixture(&mut backing, false);
    job.fence_id = 3;
    let mut nouns = IreeHalNouns::from_job(&job);
    nouns.event.fence = aether_core::fence::FenceId(257);
    let mut cmd = IreeHalCmd::pack_with_nouns(&job, &d.iommu, &nouns).unwrap();
    cmd.function = HAL_FN_ADD;
    d.submit_hal(cmd, &job).unwrap();
    assert_eq!(d.service().unwrap().status, 0);
    assert_eq!(d.completed_fence(), Some(257));
    assert_eq!(
        i32::from_le_bytes(d.mem.inner.bytes[32..36].try_into().unwrap()),
        6
    );
}

#[test]
fn invalid_shape_clears_latch_and_busy_submit_preserves_pending_dispatch() {
    let mut backing = [0u8; 256];
    let (mut d, job) = fixture(&mut backing, false);
    let mut bad = job;
    bad.m = 0;
    assert_eq!(d.submit(&bad), Err(HalError::BadArg));
    assert!(d.iommu.submit_sid().is_none());
    no_dma(&d);
    d.submit(&job).unwrap();
    let latched = d.iommu.submit_sid();
    bad.place = bad.place.with_tile(7);
    assert_eq!(d.submit(&bad), Err(HalError::Busy));
    assert_eq!(d.iommu.submit_sid(), latched);
    assert_eq!(d.service().unwrap().status, 0);
}
