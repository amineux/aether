//! Accelerator job descriptors and a software NPU (matmul / wave) model.
//!
//! The descriptor is the contract silicon partners implement. SoftNpu is the
//! reference model: integer matmul and a "wave" that is a batched matmul
//! plus a bias add — enough to show ownership + completion, not a BLAS.
//!
//! This is a dispatch record, not a graph IR. Compilers own fusion and ISA.

use crate::partition::PartitionId;
use crate::phase::Phase;
use crate::space::{MemorySpace, Place};
use crate::types::{ChipletId, PhysAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum AccelOp {
    Nop = 0,
    MatMul = 1,
    Wave = 2,
}

impl AccelOp {
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::Nop),
            1 => Some(Self::MatMul),
            2 => Some(Self::Wave),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DType {
    I32 = 0,
    // STUB: F16 / F32 need a software libm or hard-float HAL; not in v0.1.
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccelJobDesc {
    pub op: AccelOp,
    pub flags: u32,
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub a: PhysAddr,
    pub b: PhysAddr,
    pub c: PhysAddr,
    /// Optional bias vector for Wave (n elements). Zero addr = no bias.
    pub bias: PhysAddr,
    pub a_stride: u32,
    pub b_stride: u32,
    pub c_stride: u32,
    pub dtype: DType,
    pub tenant: u32,
    pub completion_ep: u32,
    /// Typed memory space the buffers are bound to. Not a unified VAS.
    pub space: MemorySpace,
    /// Fabric place for `(place, local)` addressing. Remote ≠ silent load.
    pub place: Place,
    pub phase: Phase,
    pub partition: PartitionId,
    pub fence_id: u64,
}

impl AccelJobDesc {
    pub fn matmul_i32(
        m: u32,
        n: u32,
        k: u32,
        a: PhysAddr,
        b: PhysAddr,
        c: PhysAddr,
        tenant: u32,
    ) -> Self {
        Self {
            op: AccelOp::MatMul,
            flags: 0,
            m,
            n,
            k,
            a,
            b,
            c,
            bias: PhysAddr(0),
            a_stride: k,
            b_stride: n,
            c_stride: n,
            dtype: DType::I32,
            tenant,
            completion_ep: 0,
            space: MemorySpace::Host,
            place: Place::new(ChipletId(0), MemorySpace::Host),
            phase: Phase::Compute,
            partition: PartitionId(0),
            fence_id: 0,
        }
    }

    pub fn elems_a(&self) -> usize {
        self.m as usize * self.k as usize
    }
    pub fn elems_b(&self) -> usize {
        self.k as usize * self.n as usize
    }
    pub fn elems_c(&self) -> usize {
        self.m as usize * self.n as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub job_seq: u32,
    pub status: i32,
    pub cycles: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccelError {
    BadOp,
    BadShape,
    UnsupportedDType,
    Overflow,
}

/// In-memory view the software NPU uses. Kernel identity-maps physical
/// pages; host tests pass a `SliceMem`.
pub trait DmaView {
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, AccelError>;
    fn store_i32(&mut self, addr: PhysAddr, val: i32) -> Result<(), AccelError>;
}

/// Host/test backing store: a flat buffer at `base`.
pub struct SliceMem<'a> {
    pub base: PhysAddr,
    pub bytes: &'a mut [u8],
}

impl DmaView for SliceMem<'_> {
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, AccelError> {
        let off = addr
            .0
            .checked_sub(self.base.0)
            .ok_or(AccelError::Overflow)? as usize;
        if off + 4 > self.bytes.len() {
            return Err(AccelError::Overflow);
        }
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.bytes[off..off + 4]);
        Ok(i32::from_le_bytes(b))
    }

    fn store_i32(&mut self, addr: PhysAddr, val: i32) -> Result<(), AccelError> {
        let off = addr
            .0
            .checked_sub(self.base.0)
            .ok_or(AccelError::Overflow)? as usize;
        if off + 4 > self.bytes.len() {
            return Err(AccelError::Overflow);
        }
        self.bytes[off..off + 4].copy_from_slice(&val.to_le_bytes());
        Ok(())
    }
}

/// Reference NPU. Deterministic, side-effect free aside from DMA stores.
#[derive(Clone, Debug, Default)]
pub struct SoftNpu {
    pub seq: u32,
    pub jobs_retired: u32,
}

impl SoftNpu {
    pub const fn new() -> Self {
        Self {
            seq: 1,
            jobs_retired: 0,
        }
    }

    pub fn execute<M: DmaView>(
        &mut self,
        job: &AccelJobDesc,
        mem: &mut M,
    ) -> Result<Completion, AccelError> {
        if job.dtype != DType::I32 {
            return Err(AccelError::UnsupportedDType);
        }
        match job.op {
            AccelOp::Nop => {}
            AccelOp::MatMul => self.matmul(job, mem, false)?,
            AccelOp::Wave => self.matmul(job, mem, true)?,
        }
        let seq = self.seq;
        self.seq += 1;
        self.jobs_retired += 1;
        let cycles = job.m.saturating_mul(job.n).saturating_mul(job.k).max(1);
        Ok(Completion {
            job_seq: seq,
            status: 0,
            cycles,
        })
    }

    fn matmul<M: DmaView>(
        &self,
        job: &AccelJobDesc,
        mem: &mut M,
        wave: bool,
    ) -> Result<(), AccelError> {
        if job.m == 0 || job.n == 0 || job.k == 0 || job.m > 64 || job.n > 64 || job.k > 64 {
            return Err(AccelError::BadShape);
        }
        for i in 0..job.m {
            for j in 0..job.n {
                let mut acc: i64 = 0;
                for kk in 0..job.k {
                    let a_addr = PhysAddr(job.a.0 + 4 * (i * job.a_stride + kk) as u64);
                    let b_addr = PhysAddr(job.b.0 + 4 * (kk * job.b_stride + j) as u64);
                    let av = mem.load_i32(a_addr)? as i64;
                    let bv = mem.load_i32(b_addr)? as i64;
                    acc = acc.checked_add(av * bv).ok_or(AccelError::Overflow)?;
                }
                if wave && job.bias.0 != 0 {
                    let bias = mem.load_i32(PhysAddr(job.bias.0 + 4 * j as u64))? as i64;
                    acc = acc.checked_add(bias).ok_or(AccelError::Overflow)?;
                }
                let cv: i32 = acc.try_into().map_err(|_| AccelError::Overflow)?;
                let c_addr = PhysAddr(job.c.0 + 4 * (i * job.c_stride + j) as u64);
                mem.store_i32(c_addr, cv)?;
            }
        }
        Ok(())
    }
}

/// Pure slice matmul used by unit tests (no PhysAddr).
pub fn matmul_i32_slices(
    m: usize,
    n: usize,
    k: usize,
    a: &[i32],
    lda: usize,
    b: &[i32],
    ldb: usize,
    c: &mut [i32],
    ldc: usize,
) -> Result<(), AccelError> {
    if a.len() < m * lda || b.len() < k * ldb || c.len() < m * ldc {
        return Err(AccelError::BadShape);
    }
    for i in 0..m {
        for j in 0..n {
            let mut acc: i64 = 0;
            for kk in 0..k {
                acc += a[i * lda + kk] as i64 * b[kk * ldb + j] as i64;
            }
            c[i * ldc + j] = acc.try_into().map_err(|_| AccelError::Overflow)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_times_matrix() {
        let a = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
        let b = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let mut c = [0i32; 16];
        matmul_i32_slices(4, 4, 4, &a, 4, &b, 4, &mut c, 4).unwrap();
        assert_eq!(&c, &b);
    }

    #[test]
    fn known_product() {
        // [1 2; 3 4] [5 6; 7 8] = [19 22; 43 50]
        let a = [1, 2, 3, 4];
        let b = [5, 6, 7, 8];
        let mut c = [0i32; 4];
        matmul_i32_slices(2, 2, 2, &a, 2, &b, 2, &mut c, 2).unwrap();
        assert_eq!(c, [19, 22, 43, 50]);
    }

    #[test]
    fn softnpu_via_dma() {
        let mut buf = [0u8; 256];
        let base = PhysAddr(0x1000);
        // A at 0, B at 64, C at 128
        let a = [1i32, 2, 3, 4];
        let b = [5i32, 6, 7, 8];
        for (i, v) in a.iter().enumerate() {
            buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in b.iter().enumerate() {
            buf[64 + i * 4..64 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut mem = SliceMem {
            base,
            bytes: &mut buf,
        };
        let job = AccelJobDesc::matmul_i32(
            2,
            2,
            2,
            PhysAddr(0x1000),
            PhysAddr(0x1000 + 64),
            PhysAddr(0x1000 + 128),
            1,
        );
        let mut npu = SoftNpu::new();
        let cpl = npu.execute(&job, &mut mem).unwrap();
        assert_eq!(cpl.status, 0);
        assert_eq!(npu.jobs_retired, 1);
        let mut out = [0i32; 4];
        for i in 0..4 {
            let off = 128 + i * 4;
            out[i] = i32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
        }
        assert_eq!(out, [19, 22, 43, 50]);
    }

    #[test]
    fn wave_adds_bias() {
        let mut buf = [0u8; 256];
        let base = PhysAddr(0);
        // A = I2, B = [1,2; 3,4], bias = [10, 20]
        let vals = [
            (0u64, 1i32),
            (4, 0),
            (8, 0),
            (12, 1),
            (16, 1),
            (20, 2),
            (24, 3),
            (28, 4),
            (48, 10),
            (52, 20),
        ];
        for (off, v) in vals {
            buf[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut mem = SliceMem {
            base,
            bytes: &mut buf,
        };
        let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job.op = AccelOp::Wave;
        job.bias = PhysAddr(48);
        SoftNpu::new().execute(&job, &mut mem).unwrap();
        let c0 = i32::from_le_bytes(buf[32..36].try_into().unwrap());
        let c1 = i32::from_le_bytes(buf[36..40].try_into().unwrap());
        // I * B + bias = [1,2; 3,4] + [10,20] per column → [11, 22; 13, 24]
        assert_eq!(c0, 11);
        assert_eq!(c1, 22);
    }

    #[test]
    fn bad_shape() {
        let mut buf = [0u8; 16];
        let mut mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut buf,
        };
        let job = AccelJobDesc::matmul_i32(0, 1, 1, PhysAddr(0), PhysAddr(0), PhysAddr(0), 1);
        assert_eq!(
            SoftNpu::new().execute(&job, &mut mem).unwrap_err(),
            AccelError::BadShape
        );
    }
}
