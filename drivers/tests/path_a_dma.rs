use aether_core::accel::{AccelError, AccelJobDesc, AccelOp, DType, DmaView, SliceMem};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::MapRequest;
use aether_core::{PhysAddr, TenantId};
use aether_drivers::path_a::PathABar;
use aether_hal::AccelDevice;
use std::cell::Cell;

struct Tracked<'a> {
    mem: SliceMem<'a>,
    reads: Cell<usize>,
    writes: usize,
}

impl DmaView for Tracked<'_> {
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, AccelError> {
        self.reads.set(self.reads.get() + 1);
        self.mem.load_i32(addr)
    }
    fn store_i32(&mut self, addr: PhysAddr, value: i32) -> Result<(), AccelError> {
        self.writes += 1;
        self.mem.store_i32(addr, value)
    }
    fn load_u16(&self, addr: PhysAddr) -> Result<u16, AccelError> {
        self.reads.set(self.reads.get() + 1);
        self.mem.load_u16(addr)
    }
    fn store_u16(&mut self, addr: PhysAddr, value: u16) -> Result<(), AccelError> {
        self.writes += 1;
        self.mem.store_u16(addr, value)
    }
}

fn device(backing: &mut [u8], lengths: [u64; 4]) -> PathABar<Tracked<'_>> {
    let mut dev = PathABar::new(Tracked {
        mem: SliceMem {
            base: PhysAddr(0),
            bytes: backing,
        },
        reads: Cell::new(0),
        writes: 0,
    });
    dev.probe().unwrap();
    let cap = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1));
    for (base, len) in [0, 64, 128, 192].into_iter().zip(lengths) {
        dev.map_with_cap(&cap, MapRequest::pin(PhysAddr(base), len))
            .unwrap();
    }
    dev
}

#[test]
fn unmapped_stride_or_bias_tail_refuses_before_any_dma() {
    for dtype in [DType::I32, DType::F32, DType::F16] {
        for bad_buffer in 0..4 {
            let es = u64::from(dtype.size_bytes());
            let mut backing = [0u8; 256];
            backing[128..176].fill(0xa5);
            let mut dev = device(&mut backing, [4 * es, 4 * es, 4 * es, es]);
            let mut job =
                AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(64), PhysAddr(128), 1);
            job.dtype = dtype;
            match bad_buffer {
                0 => job.a_stride = 8,
                1 => job.b_stride = 8,
                2 => job.c_stride = 8,
                _ => {
                    job.op = AccelOp::Wave;
                    job.bias = PhysAddr(192);
                }
            }
            dev.submit(&job).unwrap();
            let cpl = dev.service().unwrap();
            assert_eq!(cpl.status, -2, "{dtype:?} buffer={bad_buffer}");
            assert_eq!(dev.mem.reads.get(), 0);
            assert_eq!(dev.mem.writes, 0);
            assert_eq!(&dev.mem.mem.bytes[128..176], &[0xa5; 48]);
            assert_eq!(dev.npu.jobs_retired, 0);
        }
    }
}

#[test]
fn exact_strided_spans_execute_and_preserve_padding() {
    for dtype in [DType::I32, DType::F32, DType::F16] {
        let es = u64::from(dtype.size_bytes());
        let mut backing = [0u8; 256];
        backing[128..176].fill(0xa5);
        let mut dev = device(&mut backing, [10 * es, 10 * es, 10 * es, 2 * es]);
        let mut job =
            AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(64), PhysAddr(128), 1);
        job.dtype = dtype;
        job.a_stride = 8;
        job.b_stride = 8;
        job.c_stride = 8;
        job.op = AccelOp::Wave;
        job.bias = PhysAddr(192);
        dev.submit(&job).unwrap();
        assert_eq!(dev.service().unwrap().status, 0);
        assert_eq!(dev.mem.writes, 4);
        for i in 0..48 {
            let used = i < 2 * es as usize || (8 * es as usize..10 * es as usize).contains(&i);
            assert_eq!(dev.mem.mem.bytes[128 + i], if used { 0 } else { 0xa5 });
        }
    }
}

#[test]
fn refused_descriptor_does_not_poison_following_valid_work() {
    let mut backing = [0u8; 256];
    let mut dev = device(&mut backing, [16, 16, 16, 8]);
    let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(64), PhysAddr(128), 1);
    job.c_stride = 8;
    dev.submit(&job).unwrap();
    assert_eq!(dev.service().unwrap().status, -2);
    assert_eq!(dev.poll().unwrap().status, -2);
    assert_eq!(dev.mem.reads.get(), 0);
    job.c_stride = 2;
    dev.submit(&job).unwrap();
    assert_eq!(dev.service().unwrap().status, 0);
    assert_eq!(dev.poll().unwrap().status, 0);
    assert_eq!(dev.npu.jobs_retired, 1);
    assert_eq!(dev.mem.writes, 4);
}

#[test]
fn malformed_descriptor_refuses_before_enqueue() {
    for dtype in [DType::I32, DType::F32, DType::F16] {
        for slot in 0..4 {
            for overflow in [false, true] {
                let mut backing = [0u8; 256];
                let mut dev = device(&mut backing, [16, 16, 16, 8]);
                let mut job =
                    AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(64), PhysAddr(128), 1);
                job.dtype = dtype;
                job.op = AccelOp::Wave;
                job.bias = PhysAddr(192);
                *[&mut job.a, &mut job.b, &mut job.c, &mut job.bias][slot] =
                    PhysAddr(if overflow {
                        u64::MAX - u64::from(dtype.size_bytes()) + 1
                    } else {
                        1
                    });
                assert_eq!(dev.submit(&job), Err(aether_hal::HalError::BadArg));
                assert!(!dev.doorbell_pending());
                assert!(dev.service().is_none());
                assert!(dev.poll().is_none());
                assert_eq!(dev.mem.reads.get(), 0);
                assert_eq!(dev.mem.writes, 0);
                assert_eq!(dev.npu.jobs_retired, 0);
            }
        }
    }
}

#[test]
fn unused_operands_and_nop_do_not_require_pins() {
    let mut backing = [0u8; 256];
    let mut dev = device(&mut backing, [16, 16, 16, 8]);
    let mut job = AccelJobDesc::relu_i32(2, 2, PhysAddr(0), PhysAddr(128), 1);
    job.b = PhysAddr(u64::MAX);
    job.bias = PhysAddr(u64::MAX);
    job.k = 0;
    dev.submit(&job).unwrap();
    assert_eq!(dev.service().unwrap().status, 0);
    assert_eq!(dev.mem.reads.get(), 4);
    assert_eq!(dev.mem.writes, 4);

    let mut bytes = [0u8; 1];
    let mut empty = PathABar::new(Tracked {
        mem: SliceMem {
            base: PhysAddr(0),
            bytes: &mut bytes,
        },
        reads: Cell::new(0),
        writes: 0,
    });
    empty.probe().unwrap();
    job.op = AccelOp::Nop;
    job.a = PhysAddr(u64::MAX);
    job.c = PhysAddr(u64::MAX);
    empty.submit(&job).unwrap();
    assert_eq!(empty.peek_job_wire(0).flags, empty.stream_id().raw());
    assert_eq!(empty.service().unwrap().status, 0);
    assert_eq!(empty.mem.reads.get(), 0);
    assert_eq!(empty.mem.writes, 0);
}

#[test]
fn queued_bias_is_rechecked_after_unmap_and_wrong_sid_is_preserved() {
    let mut backing = [0u8; 256];
    let mut dev = device(&mut backing, [16, 16, 16, 8]);
    let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(64), PhysAddr(128), 1);
    job.op = AccelOp::Wave;
    job.bias = PhysAddr(192);
    let cap = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1));
    let bias = dev.translate(PhysAddr(192)).unwrap();
    dev.submit(&job).unwrap();
    dev.unmap_with_cap(&cap, bias).unwrap();
    assert_eq!(dev.service().unwrap().status, -2);
    assert_eq!(dev.poll().unwrap().status, -2);
    assert_eq!(dev.mem.reads.get(), 0);
    assert_eq!(dev.mem.writes, 0);
    dev.map_with_cap(&cap, MapRequest::pin(PhysAddr(192), 8))
        .unwrap();
    dev.submit(&job).unwrap();
    dev.inject_dma_sid(aether_core::iommu::StreamId::from_raw(0));
    assert_eq!(dev.service().unwrap().status, -2);
    assert_eq!(dev.poll().unwrap().status, -2);
    assert_eq!(dev.mem.reads.get(), 0);
    assert_eq!(dev.mem.writes, 0);
    dev.inject_dma_sid(dev.stream_id());
    dev.submit(&job).unwrap();
    assert_eq!(dev.service().unwrap().status, 0);
    assert_eq!(dev.npu.jobs_retired, 1);
    assert_eq!(dev.mem.writes, 4);
}

// Two adjacent IOVA leaves need not represent adjacent guest pages. Keep all
// ordinary operands in the first leaf and move one operand across its end.
fn split_device(backing: &mut [u8], second_pa: u64) -> PathABar<Tracked<'_>> {
    let mut dev = PathABar::new(Tracked {
        mem: SliceMem {
            base: PhysAddr(0),
            bytes: backing,
        },
        reads: Cell::new(0),
        writes: 0,
    });
    dev.probe().unwrap();
    let cap = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1));
    let first = dev
        .map_with_cap(&cap, MapRequest::pin(PhysAddr(0), 4096))
        .unwrap();
    let second = dev
        .map_with_cap(&cap, MapRequest::pin(PhysAddr(second_pa), 4096))
        .unwrap();
    assert_eq!(second.0, first.0 + 4096);
    dev
}

fn split_job(dtype: DType, operand: usize, padded: bool) -> AccelJobDesc {
    let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(256), PhysAddr(320), PhysAddr(512), 1);
    job.dtype = dtype;
    job.op = AccelOp::Wave;
    job.bias = PhysAddr(384);
    if padded {
        job.a_stride = 8;
        job.b_stride = 8;
        job.c_stride = 8;
    }
    *[&mut job.a, &mut job.b, &mut job.c, &mut job.bias][operand] =
        PhysAddr(4096 - u64::from(dtype.size_bytes()));
    job
}

#[test]
fn split_iova_operands_require_contiguous_guest_pages_before_dma() {
    for dtype in [DType::I32, DType::F32, DType::F16] {
        for operand in 0..4 {
            for padded in [false, true] {
                for second_pa in [4096, 8192] {
                    let mut backing = vec![0u8; 12288];
                    if operand == 2 {
                        backing[4096..4160].fill(0xa5);
                    }
                    let mut dev = split_device(&mut backing, second_pa);
                    let job = split_job(dtype, operand, padded);
                    let seq = dev.npu.seq;
                    dev.submit(&job).unwrap();
                    let wire = dev.peek_job_wire(0);
                    let iova = [wire.a, wire.b, wire.c, wire.bias][operand];
                    let span = job.dma_spans().unwrap()[operand].unwrap();
                    assert!(dev
                        .iommu
                        .covers_iova(dev.stream_id().raw(), PhysAddr(iova), span.len));
                    let before = dev.mem.mem.bytes.to_vec();
                    let cpl = dev.service().unwrap();
                    assert_eq!(
                        cpl.status,
                        if second_pa == 4096 { 0 } else { -2 },
                        "{dtype:?} operand={operand} padded={padded} second_pa={second_pa}"
                    );
                    assert_eq!(dev.poll().unwrap(), cpl);
                    if second_pa == 4096 {
                        // Only tensor output elements may change, including
                        // when a padded C span crosses the page boundary.
                        let es = dtype.size_bytes() as usize;
                        for (i, old) in before.iter().enumerate() {
                            let output = (0..2).any(|row| {
                                (0..2).any(|col| {
                                    let start =
                                        job.c.0 as usize + (row * job.c_stride as usize + col) * es;
                                    (start..start + es).contains(&i)
                                })
                            });
                            assert_eq!(dev.mem.mem.bytes[i], if output { 0 } else { *old });
                        }
                    } else {
                        assert_eq!(dev.mem.reads.get(), 0);
                        assert_eq!(dev.mem.writes, 0);
                        assert_eq!(dev.mem.mem.bytes, before);
                        assert_eq!(dev.npu.seq, seq);
                        assert_eq!(dev.npu.jobs_retired, 0);
                        // A refused ring entry must not poison subsequent work.
                        let valid = AccelJobDesc::matmul_i32(
                            2,
                            2,
                            2,
                            PhysAddr(256),
                            PhysAddr(320),
                            PhysAddr(512),
                            2,
                        );
                        dev.submit(&valid).unwrap();
                        assert_eq!(dev.service().unwrap().status, 0);
                        assert_eq!(dev.poll().unwrap().status, 0);
                    }
                    assert!(dev.mem.reads.get() > 0);
                    assert_eq!(dev.mem.writes, 4);
                    assert_eq!(dev.npu.jobs_retired, 1);
                }
            }
        }
    }
}

#[test]
fn queued_split_mapping_is_revalidated_at_service() {
    for dtype in [DType::I32, DType::F32, DType::F16] {
        for operand in 0..4 {
            let mut backing = vec![0u8; 12288];
            let mut dev = split_device(&mut backing, 4096);
            let job = split_job(dtype, operand, true);
            let seq = dev.npu.seq;
            dev.submit(&job).unwrap();
            // Replace the public software table at identical IOVA bases. Normal
            // unmap/map allocates new IOVAs; this is a table-replacement control.
            let mut other_backing = vec![0u8; 12288];
            let replacement = split_device(&mut other_backing, 8192);
            let valid_map = std::mem::replace(&mut dev.iommu, replacement.iommu);
            assert_eq!(dev.service().unwrap().status, -2);
            assert_eq!(dev.poll().unwrap().status, -2);
            assert_eq!(dev.mem.reads.get(), 0);
            assert_eq!(dev.mem.writes, 0);
            assert_eq!(dev.npu.seq, seq);
            assert_eq!(dev.npu.jobs_retired, 0);
            dev.iommu = valid_map;
            dev.submit(&job).unwrap();
            assert_eq!(dev.service().unwrap().status, 0);
            assert_eq!(dev.poll().unwrap().status, 0);
            assert_eq!(dev.mem.writes, 4);
            assert_eq!(dev.npu.jobs_retired, 1);
        }
    }
}
