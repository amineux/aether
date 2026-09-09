//! Accelerator job descriptors and a software NPU (matmul / wave) model.
//!
//! The descriptor is the contract silicon partners implement. SoftNpu is the
//! reference model: I32 matmul and a "wave" that is a batched matmul plus
//! a bias add, plus software IEEE-754 F16/F32 of the same ops. Enough to
//! show ownership + completion, not a BLAS and not a tensor ISA.
//!
//! This is a dispatch record, not a graph IR. Compilers own fusion and ISA.

use crate::hodge::FlowClass;
use crate::partition::PartitionId;
use crate::phase::Phase;
use crate::softfloat::{add_f16, add_f32, mul_f16, mul_f32};
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

/// Element type on [`AccelJobDesc`] / `CpCmd` / `AccelJobWire`.
///
/// Additive ABI: `I32 = 0` is unchanged. `F16 = 1` and `F32 = 2` are
/// software IEEE on SoftNPU — not a silicon tensor ISA. Unknown values
/// stay [`AccelError::UnsupportedDType`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DType {
    I32 = 0,
    F16 = 1,
    F32 = 2,
}

impl DType {
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::I32),
            1 => Some(Self::F16),
            2 => Some(Self::F32),
            _ => None,
        }
    }

    /// Element width in bytes. Strides on the job are still in elements.
    pub const fn size_bytes(self) -> u32 {
        match self {
            Self::I32 | Self::F32 => 4,
            Self::F16 => 2,
        }
    }
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
    /// Fabric class tag at submit (Gradient/tree, Curl/ring, Harmonic/persistent).
    /// Software enum on the descriptor, not a vendor `CpCmd` / path-B header.
    pub flow: FlowClass,
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
            flow: FlowClass::Gradient,
        }
    }

    /// Tag DMA / collective submit with a fabric class. Software enum only.
    pub fn with_flow(mut self, flow: FlowClass) -> Self {
        self.flow = flow;
        self
    }

    pub fn matmul_f32(
        m: u32,
        n: u32,
        k: u32,
        a: PhysAddr,
        b: PhysAddr,
        c: PhysAddr,
        tenant: u32,
    ) -> Self {
        let mut j = Self::matmul_i32(m, n, k, a, b, c, tenant);
        j.dtype = DType::F32;
        j
    }

    pub fn matmul_f16(
        m: u32,
        n: u32,
        k: u32,
        a: PhysAddr,
        b: PhysAddr,
        c: PhysAddr,
        tenant: u32,
    ) -> Self {
        let mut j = Self::matmul_i32(m, n, k, a, b, c, tenant);
        j.dtype = DType::F16;
        j
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

    pub fn elem_bytes(&self) -> u64 {
        self.dtype.size_bytes() as u64
    }

    pub fn bytes_a(&self) -> u64 {
        self.elem_bytes().saturating_mul(self.elems_a() as u64)
    }
    pub fn bytes_b(&self) -> u64 {
        self.elem_bytes().saturating_mul(self.elems_b() as u64)
    }
    pub fn bytes_c(&self) -> u64 {
        self.elem_bytes().saturating_mul(self.elems_c() as u64)
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

    /// 16-bit DMA for F16. Default is [`AccelError::UnsupportedDType`].
    fn load_u16(&self, addr: PhysAddr) -> Result<u16, AccelError> {
        let _ = addr;
        Err(AccelError::UnsupportedDType)
    }
    fn store_u16(&mut self, addr: PhysAddr, val: u16) -> Result<(), AccelError> {
        let _ = (addr, val);
        Err(AccelError::UnsupportedDType)
    }
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

    fn load_u16(&self, addr: PhysAddr) -> Result<u16, AccelError> {
        let off = addr
            .0
            .checked_sub(self.base.0)
            .ok_or(AccelError::Overflow)? as usize;
        if off + 2 > self.bytes.len() {
            return Err(AccelError::Overflow);
        }
        let mut b = [0u8; 2];
        b.copy_from_slice(&self.bytes[off..off + 2]);
        Ok(u16::from_le_bytes(b))
    }

    fn store_u16(&mut self, addr: PhysAddr, val: u16) -> Result<(), AccelError> {
        let off = addr
            .0
            .checked_sub(self.base.0)
            .ok_or(AccelError::Overflow)? as usize;
        if off + 2 > self.bytes.len() {
            return Err(AccelError::Overflow);
        }
        self.bytes[off..off + 2].copy_from_slice(&val.to_le_bytes());
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
        match job.op {
            AccelOp::Nop => {}
            AccelOp::MatMul | AccelOp::Wave => match job.dtype {
                DType::I32 => self.matmul_i32(job, mem, job.op == AccelOp::Wave)?,
                DType::F32 => self.matmul_f32(job, mem, job.op == AccelOp::Wave)?,
                DType::F16 => self.matmul_f16(job, mem, job.op == AccelOp::Wave)?,
            },
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

    fn check_shape(job: &AccelJobDesc) -> Result<(), AccelError> {
        if job.m == 0 || job.n == 0 || job.k == 0 || job.m > 64 || job.n > 64 || job.k > 64 {
            Err(AccelError::BadShape)
        } else {
            Ok(())
        }
    }

    fn elem_addr(base: PhysAddr, elem_bytes: u64, index: u64) -> Result<PhysAddr, AccelError> {
        let off = elem_bytes.checked_mul(index).ok_or(AccelError::Overflow)?;
        Ok(PhysAddr(
            base.0.checked_add(off).ok_or(AccelError::Overflow)?,
        ))
    }

    fn matmul_i32<M: DmaView>(
        &self,
        job: &AccelJobDesc,
        mem: &mut M,
        wave: bool,
    ) -> Result<(), AccelError> {
        Self::check_shape(job)?;
        let es = job.elem_bytes();
        for i in 0..job.m {
            for j in 0..job.n {
                let mut acc: i64 = 0;
                for kk in 0..job.k {
                    let a_addr = Self::elem_addr(job.a, es, (i * job.a_stride + kk) as u64)?;
                    let b_addr = Self::elem_addr(job.b, es, (kk * job.b_stride + j) as u64)?;
                    let av = mem.load_i32(a_addr)? as i64;
                    let bv = mem.load_i32(b_addr)? as i64;
                    acc = acc.checked_add(av * bv).ok_or(AccelError::Overflow)?;
                }
                if wave && job.bias.0 != 0 {
                    let bias = mem.load_i32(Self::elem_addr(job.bias, es, j as u64)?)? as i64;
                    acc = acc.checked_add(bias).ok_or(AccelError::Overflow)?;
                }
                let cv: i32 = acc.try_into().map_err(|_| AccelError::Overflow)?;
                let c_addr = Self::elem_addr(job.c, es, (i * job.c_stride + j) as u64)?;
                mem.store_i32(c_addr, cv)?;
            }
        }
        Ok(())
    }

    fn matmul_f32<M: DmaView>(
        &self,
        job: &AccelJobDesc,
        mem: &mut M,
        wave: bool,
    ) -> Result<(), AccelError> {
        Self::check_shape(job)?;
        let es = job.elem_bytes();
        for i in 0..job.m {
            for j in 0..job.n {
                let mut acc: u32 = 0;
                for kk in 0..job.k {
                    let a_addr = Self::elem_addr(job.a, es, (i * job.a_stride + kk) as u64)?;
                    let b_addr = Self::elem_addr(job.b, es, (kk * job.b_stride + j) as u64)?;
                    let av = mem.load_i32(a_addr)? as u32;
                    let bv = mem.load_i32(b_addr)? as u32;
                    let prod = mul_f32(av, bv);
                    acc = add_f32(acc, prod);
                }
                if wave && job.bias.0 != 0 {
                    let bias = mem.load_i32(Self::elem_addr(job.bias, es, j as u64)?)? as u32;
                    acc = add_f32(acc, bias);
                }
                let c_addr = Self::elem_addr(job.c, es, (i * job.c_stride + j) as u64)?;
                mem.store_i32(c_addr, acc as i32)?;
            }
        }
        Ok(())
    }

    fn matmul_f16<M: DmaView>(
        &self,
        job: &AccelJobDesc,
        mem: &mut M,
        wave: bool,
    ) -> Result<(), AccelError> {
        Self::check_shape(job)?;
        let es = job.elem_bytes();
        for i in 0..job.m {
            for j in 0..job.n {
                let mut acc: u16 = 0;
                for kk in 0..job.k {
                    let a_addr = Self::elem_addr(job.a, es, (i * job.a_stride + kk) as u64)?;
                    let b_addr = Self::elem_addr(job.b, es, (kk * job.b_stride + j) as u64)?;
                    let av = mem.load_u16(a_addr)?;
                    let bv = mem.load_u16(b_addr)?;
                    acc = add_f16(acc, mul_f16(av, bv));
                }
                if wave && job.bias.0 != 0 {
                    let bias = mem.load_u16(Self::elem_addr(job.bias, es, j as u64)?)?;
                    acc = add_f16(acc, bias);
                }
                let c_addr = Self::elem_addr(job.c, es, (i * job.c_stride + j) as u64)?;
                mem.store_u16(c_addr, acc)?;
            }
        }
        Ok(())
    }
}

/// Host-identical F16/F32 2×2 check used by the boot demo / QEMU line.
/// Bit patterns only — no runtime `f32` ops (kernel is soft-float / FTZ).
pub fn demo_f16_f32_ok() -> bool {
    // [1 2; 3 4] [5 6; 7 8] = [19 22; 43 50]
    const F32: [u32; 8] = [
        0x3f80_0000, // 1
        0x4000_0000, // 2
        0x4040_0000, // 3
        0x4080_0000, // 4
        0x40a0_0000, // 5
        0x40c0_0000, // 6
        0x40e0_0000, // 7
        0x4100_0000, // 8
    ];
    const F32_19: u32 = 0x4198_0000;
    const F32_50: u32 = 0x4248_0000;
    let mut buf = [0u8; 128];
    for (i, v) in F32.iter().enumerate() {
        buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut mem = SliceMem {
        base: PhysAddr(0),
        bytes: &mut buf,
    };
    let job = AccelJobDesc::matmul_f32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
    if SoftNpu::new().execute(&job, &mut mem).is_err() {
        return false;
    }
    let c00 = u32::from_le_bytes(buf[32..36].try_into().unwrap());
    let c11 = u32::from_le_bytes(buf[44..48].try_into().unwrap());
    if c00 != F32_19 || c11 != F32_50 {
        return false;
    }

    let f16s = [
        crate::softfloat::f32_to_f16(F32[0]),
        crate::softfloat::f32_to_f16(F32[1]),
        crate::softfloat::f32_to_f16(F32[2]),
        crate::softfloat::f32_to_f16(F32[3]),
        crate::softfloat::f32_to_f16(F32[4]),
        crate::softfloat::f32_to_f16(F32[5]),
        crate::softfloat::f32_to_f16(F32[6]),
        crate::softfloat::f32_to_f16(F32[7]),
    ];
    let mut hbuf = [0u8; 64];
    for (i, v) in f16s.iter().enumerate() {
        hbuf[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
    }
    let mut mem = SliceMem {
        base: PhysAddr(0),
        bytes: &mut hbuf,
    };
    let job = AccelJobDesc::matmul_f16(2, 2, 2, PhysAddr(0), PhysAddr(8), PhysAddr(16), 1);
    if SoftNpu::new().execute(&job, &mut mem).is_err() {
        return false;
    }
    let h00 = u16::from_le_bytes(hbuf[16..18].try_into().unwrap());
    let h11 = u16::from_le_bytes(hbuf[22..24].try_into().unwrap());
    h00 == crate::softfloat::f32_to_f16(F32_19) && h11 == crate::softfloat::f32_to_f16(F32_50)
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

    #[test]
    fn dtype_values_are_additive() {
        assert_eq!(DType::I32 as u8, 0);
        assert_eq!(DType::F16 as u8, 1);
        assert_eq!(DType::F32 as u8, 2);
        assert_eq!(DType::from_u8(0), Some(DType::I32));
        assert_eq!(DType::from_u8(1), Some(DType::F16));
        assert_eq!(DType::from_u8(2), Some(DType::F32));
        assert_eq!(DType::from_u8(3), None);
        assert_eq!(DType::I32.size_bytes(), 4);
        assert_eq!(DType::F32.size_bytes(), 4);
        assert_eq!(DType::F16.size_bytes(), 2);
    }

    #[test]
    fn softnpu_f32_via_dma() {
        assert!(demo_f16_f32_ok());
        let mut buf = [0u8; 128];
        for (i, v) in [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
            .iter()
            .enumerate()
        {
            buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_bits().to_le_bytes());
        }
        let mut mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut buf,
        };
        let job = AccelJobDesc::matmul_f32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        SoftNpu::new().execute(&job, &mut mem).unwrap();
        let out = [
            f32::from_bits(u32::from_le_bytes(buf[32..36].try_into().unwrap())),
            f32::from_bits(u32::from_le_bytes(buf[36..40].try_into().unwrap())),
            f32::from_bits(u32::from_le_bytes(buf[40..44].try_into().unwrap())),
            f32::from_bits(u32::from_le_bytes(buf[44..48].try_into().unwrap())),
        ];
        assert_eq!(out, [19.0, 22.0, 43.0, 50.0]);
    }

    #[test]
    fn wave_f32_adds_bias() {
        let mut buf = [0u8; 128];
        // A = I2, B = [1,2; 3,4], bias = [10, 20]
        let vals = [
            (0u64, 1.0f32),
            (4, 0.0),
            (8, 0.0),
            (12, 1.0),
            (16, 1.0),
            (20, 2.0),
            (24, 3.0),
            (28, 4.0),
            (48, 10.0),
            (52, 20.0),
        ];
        for (off, v) in vals {
            buf[off as usize..off as usize + 4].copy_from_slice(&v.to_bits().to_le_bytes());
        }
        let mut mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut buf,
        };
        let mut job = AccelJobDesc::matmul_f32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job.op = AccelOp::Wave;
        job.bias = PhysAddr(48);
        SoftNpu::new().execute(&job, &mut mem).unwrap();
        let c0 = f32::from_bits(u32::from_le_bytes(buf[32..36].try_into().unwrap()));
        let c1 = f32::from_bits(u32::from_le_bytes(buf[36..40].try_into().unwrap()));
        assert_eq!(c0, 11.0);
        assert_eq!(c1, 22.0);
    }

    #[test]
    fn f16_without_u16_dma_is_unsupported() {
        struct No16;
        impl DmaView for No16 {
            fn load_i32(&self, _: PhysAddr) -> Result<i32, AccelError> {
                Ok(0)
            }
            fn store_i32(&mut self, _: PhysAddr, _: i32) -> Result<(), AccelError> {
                Ok(())
            }
        }
        let job = AccelJobDesc::matmul_f16(2, 2, 2, PhysAddr(0), PhysAddr(8), PhysAddr(16), 1);
        assert_eq!(
            SoftNpu::new().execute(&job, &mut No16).unwrap_err(),
            AccelError::UnsupportedDType
        );
        let mut nop = job;
        nop.op = AccelOp::Nop;
        assert!(SoftNpu::new().execute(&nop, &mut No16).is_ok());
    }
}
