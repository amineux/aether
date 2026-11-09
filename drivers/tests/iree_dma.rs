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

// Guest pages are all pinned in either order. Only the IOVA layout changes:
// 0 -> 4096 -> 8192 is contiguous; 0 -> 8192 -> 4096 is not.
fn split_fixture(
    backing: &mut [u8],
    dtype: DType,
    operand: usize,
    ordered: bool,
) -> (IreeShapedCp<Counted<'_>>, AccelJobDesc) {
    let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(256), PhysAddr(320), PhysAddr(512), 1);
    job.op = aether_core::accel::AccelOp::Wave;
    job.dtype = dtype;
    job.bias = PhysAddr(384);
    job.fence_id = 77;
    *[&mut job.a, &mut job.b, &mut job.c, &mut job.bias][operand] =
        PhysAddr(4096 - u64::from(dtype.size_bytes()));
    let mut d = IreeShapedCp::new(Counted {
        inner: SliceMem {
            base: PhysAddr(0),
            bytes: backing,
        },
        reads: Cell::new(0),
        writes: 0,
    });
    for pa in if ordered {
        [0, 4096, 8192]
    } else {
        [0, 8192, 4096]
    } {
        d.map_with_cap(
            &cap(),
            MapRequest::pin_accel(PhysAddr(pa), 4096, stream_for_job(&job)),
        )
        .unwrap();
    }
    (d, job)
}

#[test]
fn discontinuous_iova_bindings_refuse_pack_and_both_submit_paths() {
    for dtype in [DType::I32, DType::F16, DType::F32] {
        for operand in 0..4 {
            let mut backing = vec![0u8; 12288];
            let (mut d, job) = split_fixture(&mut backing, dtype, operand, false);
            let sid = stream_for_job(&job);
            let span = job.dma_spans().unwrap()[operand].unwrap();
            let iova = d.iommu.translate_stream(sid.raw(), span.base).unwrap();
            assert!(d.iommu.covers_stream(sid.raw(), span.base, span.len));
            assert!(d.iommu.covers_iova(sid.raw(), iova, span.len));
            assert_eq!(
                d.iommu
                    .resolve_stream(sid.raw(), PhysAddr(iova.0 + u64::from(dtype.size_bytes()))),
                Some(PhysAddr(8192))
            );
            let before = d.mem.inner.bytes.to_vec();
            assert_eq!(IreeHalCmd::pack(&job, &d.iommu), Err(HalError::Fault));
            assert_eq!(d.submit(&job), Err(HalError::Fault));
            assert!(d.iommu.submit_sid().is_none());
            // The same base offsets and lengths form a valid copied image
            // against the ordered table; reject it against the current table.
            let mut other = vec![0u8; 12288];
            let (good, _) = split_fixture(&mut other, dtype, operand, true);
            let cmd = IreeHalCmd::pack(&job, &good.iommu).unwrap();
            assert_eq!(d.submit_hal(cmd, &job), Err(HalError::Fault));
            assert!(!d.doorbell_pending());
            assert!(!d.irq_pending());
            assert!(d.last_cmd().is_none());
            assert!(d.completed_fence().is_none());
            assert!(d.service().is_none());
            assert!(d.poll().is_none());
            no_dma(&d);
            assert_eq!(d.mem.inner.bytes, before);
            d.iommu = good.iommu;
            d.submit(&job).unwrap();
            assert_eq!(d.service().unwrap().status, 0);
            assert_eq!(d.mem.writes, 4);
            assert_eq!(d.completed_fence(), Some(77));
        }
    }
}

#[test]
fn queued_binding_rechecks_iova_continuity_and_recovers() {
    for dtype in [DType::I32, DType::F16, DType::F32] {
        for operand in 0..4 {
            let mut backing = vec![0u8; 12288];
            let (mut d, job) = split_fixture(&mut backing, dtype, operand, true);
            let seq = d.submit(&job).unwrap();
            let before = d.mem.inner.bytes.to_vec();
            let mut other = vec![0u8; 12288];
            let (replacement, _) = split_fixture(&mut other, dtype, operand, false);
            // Public software-table replacement preserves IOVA bases. Normal
            // unmap/map allocates new IOVAs and does not model this control.
            let good = core::mem::replace(&mut d.iommu, replacement.iommu);
            d.iommu.set_sid_bound(stream_for_job(&job)).unwrap();
            let cpl = d.service().unwrap();
            assert_eq!((cpl.status, cpl.job_seq), (-2, seq));
            assert_eq!(d.poll().unwrap(), cpl);
            assert_eq!(d.completed_fence(), Some(77));
            assert!(d.iommu.submit_sid().is_none());
            no_dma(&d);
            assert_eq!(d.mem.inner.bytes, before);
            d.iommu = good;
            assert_eq!(d.submit(&job).unwrap(), seq);
            let retry = d.service().unwrap();
            assert_eq!((retry.status, retry.job_seq), (0, seq));
            assert_eq!(d.mem.writes, 4);
            assert!(d.iommu.submit_sid().is_none());
        }
    }
}
