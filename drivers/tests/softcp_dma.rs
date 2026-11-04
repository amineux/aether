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
