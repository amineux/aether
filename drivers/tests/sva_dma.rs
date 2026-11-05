//! SVA range validation with real Soft-CP dispatch and counted DMA.
use aether_core::accel::{AccelError, AccelJobDesc, DType, DmaView, SliceMem};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::MmId;
use aether_core::types::{PhysAddr, TenantId};
use aether_drivers::fakecp::{stream_for_job, SoftCommandProcessor};
use aether_hal::AccelDevice;
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
    let sid = stream_for_job(&job);
    d.bind_sva_with_cap(&cap(), sid, MmId(7)).unwrap();
    for (base, len, va) in pins() {
        d.map_va_with_cap(&cap(), sid, PhysAddr(va), PhysAddr(base), len)
            .unwrap();
    }
    (d, job)
}
fn no_dma(d: &SoftCommandProcessor<Counted<'_>>) {
    assert_eq!(d.mem.reads.get(), 0);
    assert_eq!(d.mem.writes, 0);
}

fn pins() -> [(u64, u64, u64); 4] {
    [
        (0, 16, 0x10000),
        (16, 16, 0x20000),
        (32, 16, 0x30000),
        (80, 8, 0x40000),
    ]
}

#[test]
fn sva_short_or_unmapped_operand_tail_refuses_before_all_dma() {
    for dtype in [DType::I32, DType::F16, DType::F32] {
        for (base, _len, va) in pins() {
            for short in [false, true] {
                let mut backing = [0u8; 256];
                let (mut d, mut job) = fixture(&mut backing, true);
                job.dtype = dtype;
                let sid = stream_for_job(&job);
                let before = d.mem.inner.bytes.to_vec();
                let full = d.iommu.clone();
                d.submit(&job).unwrap();
                d.unmap_va(&cap(), sid, PhysAddr(va)).unwrap();
                if short {
                    d.map_va_with_cap(&cap(), sid, PhysAddr(va), PhysAddr(base), 1)
                        .unwrap();
                    assert_eq!(
                        d.iommu.resolve_ats(sid.raw(), PhysAddr(va)),
                        Ok(PhysAddr(base)),
                        "base still resolves"
                    );
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
fn sva_split_operand_requires_adjacent_va_and_guest_pa_segments() {
    for second_pa in [None, Some(8), Some(128)] {
        for warm in [false, true] {
            let mut backing = [0u8; 256];
            let (mut d, job) = fixture(&mut backing, false);
            let sid = stream_for_job(&job);
            let before = d.mem.inner.bytes.to_vec();
            d.submit(&job).unwrap();
            d.unmap_va(&cap(), sid, PhysAddr(0x10000)).unwrap();
            d.map_va_with_cap(&cap(), sid, PhysAddr(0x10000), PhysAddr(0), 8)
                .unwrap();
            if let Some(pa) = second_pa {
                d.map_va_with_cap(&cap(), sid, PhysAddr(0x10008), PhysAddr(pa), 8)
                    .unwrap();
            }
            if warm {
                d.iommu.resolve_ats(sid.raw(), PhysAddr(0x10000)).unwrap();
            }
            let cpl = d.service().unwrap();
            if second_pa == Some(8) {
                assert_eq!(cpl.status, 0);
                assert_eq!(
                    i32::from_le_bytes(d.mem.inner.bytes[32..36].try_into().unwrap()),
                    19
                );
                assert_eq!(
                    i32::from_le_bytes(d.mem.inner.bytes[40..44].try_into().unwrap()),
                    43
                );
            } else {
                assert_eq!((cpl.status, cpl.job_seq), (-2, 1));
                no_dma(&d);
                assert_eq!(d.mem.inner.bytes, &before);
            }
            assert!(d.iommu.submit_sid().is_none());
        }
    }
}
