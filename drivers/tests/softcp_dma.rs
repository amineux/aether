//! Standard-IOVA Soft-CP dispatch with counted DMA and narrow pins.
use aether_core::accel::{AccelError, AccelJobDesc, AccelOp, DType, DmaView, SliceMem};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::MapRequest;
use aether_core::types::{PhysAddr, TenantId};
use aether_drivers::fakecp::{stream_for_job, CpCmd, SoftCommandProcessor};
use aether_drivers::firewall::{validate_cp_cmd, SliceCmdStream};
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
fn fixture(
    backing: &mut [u8; 256],
    wave: bool,
) -> (SoftCommandProcessor<Counted<'_>>, AccelJobDesc) {
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
    let mut d = SoftCommandProcessor::new(mem);
    for (base, len) in [(0, 16), (16, 16), (32, 16), (80, 8)] {
        d.map_with_cap(
            &cap(),
            MapRequest::pin_accel(PhysAddr(base), len, stream_for_job(&job)),
        )
        .unwrap();
    }
    (d, job)
}
fn no_dma(d: &SoftCommandProcessor<Counted<'_>>) {
    assert_eq!(d.mem.reads.get(), 0);
    assert_eq!(d.mem.writes, 0);
}

#[test]
fn queued_unmap_and_same_iova_short_pins_refuse_before_dma() {
    for dtype in [DType::I32, DType::F16, DType::F32] {
        for base in [0, 16, 32, 80] {
            for short_pin in [false, true] {
                let mut backing = [0u8; 256];
                let (mut d, mut job) = fixture(&mut backing, true);
                job.dtype = dtype;
                let before = d.mem.inner.bytes.to_vec();
                let sid = stream_for_job(&job);
                let iova = d.iommu.translate_stream(sid.raw(), PhysAddr(base)).unwrap();
                let full = d.iommu.clone();
                d.submit(&job).unwrap();
                d.unmap_with_cap(&cap(), iova).unwrap();
                if short_pin {
                    // Public software map replacement retains the original IOVAs;
                    // ordinary unmap/map allocates a different IOVA.
                    d.iommu = aether_core::iommu::IommuMap::new();
                    for (pa, len) in [(0, 16), (16, 16), (32, 16), (80, 8)] {
                        d.map_with_cap(
                            &cap(),
                            MapRequest::pin_accel(
                                PhysAddr(pa),
                                if pa == base { 1 } else { len },
                                sid,
                            ),
                        )
                        .unwrap();
                    }
                    assert_eq!(
                        d.iommu.translate_stream(sid.raw(), PhysAddr(base)),
                        Some(iova)
                    );
                    assert!(d.iommu.resolve_stream(sid.raw(), iova).is_some());
                }
                let cpl = d.service().unwrap();
                assert_eq!((cpl.status, cpl.job_seq), (-2, 1));
                assert!(d.xqueue(0).unwrap().is_empty());
                assert!(d.iommu.submit_sid().is_none());
                no_dma(&d);
                assert_eq!(d.mem.inner.bytes, &before);
                assert_eq!(d.poll().unwrap().status, -2);
                d.iommu = full;
                d.submit(&job).unwrap();
                let retry = d.service().unwrap();
                assert_eq!((retry.status, retry.job_seq), (0, 1));
            }
        }
    }
}

#[test]
fn elementwise_cmdbuf_uses_m_by_n_spans_and_packed_row_stride() {
    for op in [AccelOp::Add, AccelOp::Relu, AccelOp::Mul, AccelOp::Max] {
        let mut backing = [0u8; 256];
        let (mut d, _) = fixture(&mut backing, false);
        d.iommu = aether_core::iommu::IommuMap::new();
        for (i, v) in [1i32, 2, 3, 4, 5, 6].iter().enumerate() {
            d.mem.inner.bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            d.mem.inner.bytes[32 + i * 4..36 + i * 4].copy_from_slice(&(v + 6).to_le_bytes());
        }
        let mut job = AccelJobDesc::add_i32(3, 2, PhysAddr(0), PhysAddr(32), PhysAddr(64), 1);
        job.op = op;
        let sid = stream_for_job(&job);
        for pa in [0, 32, 64] {
            d.map_with_cap(&cap(), MapRequest::pin_accel(PhysAddr(pa), 24, sid))
                .unwrap();
        }
        d.iommu.set_sid_bound(sid).unwrap();
        let cmd = CpCmd::pack(&job, &d.iommu).unwrap();
        d.iommu.clear_submit_sid();
        let full = d.iommu.clone();
        for (base, short) in [(0, 12), (32, 8)] {
            d.iommu = aether_core::iommu::IommuMap::new();
            for pa in [0, 32, 64] {
                d.map_with_cap(
                    &cap(),
                    MapRequest::pin_accel(PhysAddr(pa), if pa == base { short } else { 24 }, sid),
                )
                .unwrap();
            }
            assert_eq!(
                validate_cp_cmd(&cmd, &d.iommu, Some(sid)),
                Err(HalError::Fault)
            );
            let bytes = cmd.to_le_bytes();
            assert_eq!(
                d.submit_cmdbuf(0, &mut SliceCmdStream { bytes: &bytes }, &job),
                Err(HalError::Fault)
            );
            assert!(d.xqueue(0).unwrap().is_empty());
            assert!(d.iommu.submit_sid().is_none());
            no_dma(&d);
        }
        d.iommu = full;
        let bytes = cmd.to_le_bytes();
        d.submit_cmdbuf(0, &mut SliceCmdStream { bytes: &bytes }, &job)
            .unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        let expected = match op {
            AccelOp::Add => [8, 10, 12, 14, 16, 18],
            AccelOp::Relu => [1, 2, 3, 4, 5, 6],
            AccelOp::Mul => [7, 16, 27, 40, 55, 72],
            _ => [7, 8, 9, 10, 11, 12],
        };
        for (i, value) in expected.iter().enumerate() {
            assert_eq!(
                i32::from_le_bytes(
                    d.mem.inner.bytes[64 + i * 4..68 + i * 4]
                        .try_into()
                        .unwrap()
                ),
                *value
            );
        }
    }
}

#[test]
fn copied_wave_packet_cannot_inherit_an_unencoded_bias() {
    let mut backing = [0u8; 256];
    let (mut d, mut job) = fixture(&mut backing, false);
    job.op = AccelOp::Wave;
    let sid = stream_for_job(&job);
    d.iommu.set_sid_bound(sid).unwrap();
    let cmd = CpCmd::pack(&job, &d.iommu).unwrap();
    d.iommu.clear_submit_sid();
    // Packet has no bias, but the separate host template names unpinned bytes.
    job.bias = PhysAddr(96);
    d.mem.inner.bytes[96..100].copy_from_slice(&1000i32.to_le_bytes());
    let bytes = cmd.to_le_bytes();
    d.submit_cmdbuf(0, &mut SliceCmdStream { bytes: &bytes }, &job)
        .unwrap();
    assert_eq!(d.service().unwrap().status, 0);
    assert_eq!(
        i32::from_le_bytes(d.mem.inner.bytes[32..36].try_into().unwrap()),
        19
    );
}

#[test]
fn elementwise_single_row_does_not_require_matmul_sized_pins() {
    let mut backing = [0u8; 256];
    let (mut d, _) = fixture(&mut backing, false);
    let mut job = AccelJobDesc::add_i32(1, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
    job.k = 4; // unused by elementwise execution
    let sid = stream_for_job(&job);
    d.iommu = aether_core::iommu::IommuMap::new();
    for base in [0, 16, 32] {
        d.map_with_cap(&cap(), MapRequest::pin_accel(PhysAddr(base), 8, sid))
            .unwrap();
    }
    d.submit(&job).unwrap();
    assert_eq!(d.service().unwrap().status, 0);
    assert_eq!(
        i32::from_le_bytes(d.mem.inner.bytes[32..36].try_into().unwrap()),
        6
    );
}

fn split_iova_fixture(
    backing: &mut [u8],
    dtype: DType,
    operand: usize,
    second_pa: u64,
) -> (SoftCommandProcessor<Counted<'_>>, AccelJobDesc, CpCmd) {
    backing.fill(0);
    backing[0x200..0x210].fill(0x55);
    let mem = Counted {
        inner: SliceMem {
            base: PhysAddr(0),
            bytes: backing,
        },
        reads: Cell::new(0),
        writes: 0,
    };
    let mut d = SoftCommandProcessor::new(mem);
    let mut job = AccelJobDesc::matmul_i32(
        2,
        2,
        2,
        PhysAddr(0x100),
        PhysAddr(0x140),
        PhysAddr(0x200),
        1,
    );
    job.op = AccelOp::Wave;
    job.bias = PhysAddr(0x180);
    job.dtype = dtype;
    let sid = stream_for_job(&job);
    let first = d
        .map_with_cap(&cap(), MapRequest::pin_accel(PhysAddr(0), 4096, sid))
        .unwrap();
    let second = d
        .map_with_cap(
            &cap(),
            MapRequest::pin_accel(PhysAddr(second_pa), 4096, sid),
        )
        .unwrap();
    assert_eq!(second.0, first.0 + 4096);
    d.iommu.set_sid_bound(sid).unwrap();
    let mut cmd = CpCmd::pack(&job, &d.iommu).unwrap();
    let crossing = first.0 + 4096 - job.elem_bytes();
    match operand {
        0 => cmd.iova_a = crossing,
        1 => cmd.iova_b = crossing,
        2 => {
            cmd.iova_c = crossing;
            d.mem.inner.bytes[4092..4112].fill(0x55);
        }
        _ => cmd.iova_bias = crossing,
    }
    d.iommu.clear_submit_sid();
    (d, job, cmd)
}

#[test]
fn copied_iova_split_operand_requires_guest_pa_continuity_at_admission() {
    for dtype in [DType::I32, DType::F16, DType::F32] {
        for operand in 0..4 {
            for second_pa in [4096, 8192] {
                let mut backing = vec![0u8; 12288];
                let (mut d, job, cmd) = split_iova_fixture(&mut backing, dtype, operand, second_pa);
                let before = d.mem.inner.bytes.to_vec();
                let bytes = cmd.to_le_bytes();
                let result = d.submit_cmdbuf(0, &mut SliceCmdStream { bytes: &bytes }, &job);
                if second_pa == 4096 {
                    result.unwrap();
                    assert_eq!(d.service().unwrap().status, 0);
                    assert!(d.mem.writes > 0);
                    let width = job.elem_bytes() as usize;
                    let dest = if operand == 2 { 4096 - width } else { 0x200 };
                    assert!(d.mem.inner.bytes[dest..dest + 4 * width]
                        .iter()
                        .all(|byte| *byte == 0));
                } else {
                    assert_eq!(result, Err(HalError::Fault));
                    assert!(d.xqueue(0).unwrap().is_empty());
                    assert!(d.iommu.submit_sid().is_none());
                    assert!(d.service().is_none());
                    no_dma(&d);
                    assert_eq!(d.mem.inner.bytes, &before);
                }
            }
        }
    }
}

#[test]
fn queued_iova_split_operand_is_revalidated_before_dma_and_recovers() {
    for dtype in [DType::I32, DType::F16, DType::F32] {
        for operand in 0..4 {
            let mut backing = vec![0u8; 12288];
            let (mut d, job, cmd) = split_iova_fixture(&mut backing, dtype, operand, 4096);
            let before = d.mem.inner.bytes.to_vec();
            let sid = stream_for_job(&job);
            let full = d.iommu.clone();
            let bytes = cmd.to_le_bytes();
            d.submit_cmdbuf(0, &mut SliceCmdStream { bytes: &bytes }, &job)
                .unwrap();
            // Replace the public software map table, preserving IOVAs while
            // changing the second leaf's guest PA (normal re-map gets a new IOVA).
            d.iommu = aether_core::iommu::IommuMap::new();
            d.map_with_cap(&cap(), MapRequest::pin_accel(PhysAddr(0), 4096, sid))
                .unwrap();
            d.map_with_cap(&cap(), MapRequest::pin_accel(PhysAddr(8192), 4096, sid))
                .unwrap();
            let cpl = d.service().unwrap();
            assert_eq!((cpl.status, cpl.job_seq), (-2, 1));
            assert!(d.iommu.submit_sid().is_none());
            no_dma(&d);
            assert_eq!(d.mem.inner.bytes, &before);
            d.iommu = full;
            d.submit_cmdbuf(0, &mut SliceCmdStream { bytes: &bytes }, &job)
                .unwrap();
            let retry = d.service().unwrap();
            assert_eq!((retry.status, retry.job_seq), (0, 1));
        }
    }
}
