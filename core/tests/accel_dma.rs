use aether_core::accel::{AccelError, AccelJobDesc, AccelOp, DType, DmaView, SoftNpu};
use aether_core::PhysAddr;
use std::cell::RefCell;

fn job(dtype: DType) -> AccelJobDesc {
    let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(64), PhysAddr(128), 1);
    job.dtype = dtype;
    job
}

#[test]
fn spans_include_row_padding_and_wave_bias_for_every_dtype() {
    for dtype in [DType::I32, DType::F32, DType::F16] {
        let mut job = job(dtype);
        job.m = 3;
        job.k = 4;
        job.a_stride = 10;
        job.b_stride = 11;
        job.c_stride = 12;
        job.op = AccelOp::Wave;
        job.bias = PhysAddr(192);
        let spans = job.dma_spans().unwrap();
        let es = u64::from(dtype.size_bytes());
        assert_eq!(
            spans.map(|s| s.map(|s| s.len)),
            [Some(24 * es), Some(35 * es), Some(26 * es), Some(2 * es)]
        );
    }
}

struct NeverDma;
impl DmaView for NeverDma {
    fn load_i32(&self, _: PhysAddr) -> Result<i32, AccelError> {
        panic!("invalid descriptor reached DMA")
    }
    fn store_i32(&mut self, _: PhysAddr, _: i32) -> Result<(), AccelError> {
        panic!("invalid descriptor reached DMA")
    }
    fn load_u16(&self, _: PhysAddr) -> Result<u16, AccelError> {
        panic!("invalid descriptor reached DMA")
    }
    fn store_u16(&mut self, _: PhysAddr, _: u16) -> Result<(), AccelError> {
        panic!("invalid descriptor reached DMA")
    }
}

#[test]
fn every_overflowing_or_unaligned_operand_refuses_before_dma() {
    for dtype in [DType::I32, DType::F32, DType::F16] {
        for slot in 0..4 {
            for overflow in [false, true] {
                let mut job = job(dtype);
                job.op = AccelOp::Wave;
                job.bias = PhysAddr(192);
                let address = if overflow {
                    u64::MAX - u64::from(dtype.size_bytes()) + 1
                } else {
                    1
                };
                *[&mut job.a, &mut job.b, &mut job.c, &mut job.bias][slot] = PhysAddr(address);
                let mut npu = SoftNpu::new();
                assert_eq!(
                    npu.execute(&job, &mut NeverDma),
                    Err(if overflow {
                        AccelError::Overflow
                    } else {
                        AccelError::BadShape
                    })
                );
                assert_eq!(npu.seq, 1);
                assert_eq!(npu.jobs_retired, 0);
            }
        }
    }
}

#[derive(Default)]
struct SparseDma {
    reads: RefCell<Vec<u64>>,
    writes: Vec<u64>,
}
impl DmaView for SparseDma {
    fn load_i32(&self, a: PhysAddr) -> Result<i32, AccelError> {
        self.reads.borrow_mut().push(a.0);
        Ok(0)
    }
    fn store_i32(&mut self, a: PhysAddr, _: i32) -> Result<(), AccelError> {
        self.writes.push(a.0);
        Ok(())
    }
    fn load_u16(&self, a: PhysAddr) -> Result<u16, AccelError> {
        self.reads.borrow_mut().push(a.0);
        Ok(0)
    }
    fn store_u16(&mut self, a: PhysAddr, _: u16) -> Result<(), AccelError> {
        self.writes.push(a.0);
        Ok(())
    }
}

#[test]
fn row_index_is_widened_before_stride_multiplication() {
    for dtype in [DType::I32, DType::F32, DType::F16] {
        let mut job = job(dtype);
        job.m = 3;
        job.n = 1;
        job.k = 1;
        job.a_stride = u32::MAX;
        job.b_stride = 1;
        job.c_stride = 1;
        let mut mem = SparseDma::default();
        SoftNpu::new().execute(&job, &mut mem).unwrap();
        assert!(mem
            .reads
            .borrow()
            .contains(&(2 * u64::from(u32::MAX) * u64::from(dtype.size_bytes()))));
        assert_eq!(mem.writes.len(), 3);
    }
}

#[test]
fn unused_operands_do_not_require_dma_ranges() {
    let mut job = job(DType::I32);
    job.op = AccelOp::Relu;
    job.b = PhysAddr(u64::MAX);
    job.bias = PhysAddr(u64::MAX);
    job.k = 0;
    let spans = job.dma_spans().unwrap();
    assert!(spans[1].is_none() && spans[3].is_none());
    job.op = AccelOp::Nop;
    assert_eq!(job.dma_spans(), Ok([None; 4]));
    SoftNpu::new().execute(&job, &mut NeverDma).unwrap();
}
