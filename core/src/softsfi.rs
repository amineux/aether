//! SoftSFI: Soft-CP bytecode memory sandbox (GPU-AToLL-shaped SFI).
//!
//! SpectraScout leftover (M5 digest). [GPU-AToLL][atoll] hardens NVVM-IR
//! so every memory side-effect proves a location + PTX state space before
//! a tenant kernel may run. SoftSFI borrows that *shape* on a **toy**
//! Soft-CP ISA (`load` / `store` / `add` / `dma`): every load, store, and
//! DMA proves `base+bound` sits in the SID-allowed range. CUDA-tied in
//! the source paper; the pattern is the proof, not the pipeline.
//!
//! **Not claimed.** This is not an NVVM / LLVM pass, not CUDA, not PTX,
//! and **not** “safe multi-tenant kernels” covering all side-effects.
//! GPU-AToLL itself says validation only checks memory isolation.
//!
//! Honest TODOs (same holes GPU-AToLL leaves open):
//! - Atomics (`ATOMIC_ADD`) are **refused**, not modeled.
//! - Tensor copies / SoftNPU `MatMul` / `Wave` are **refused**, not modeled.
//! - Heap / dynamic allocation is not a sandbox (no heap in this ISA).
//!
//! [atoll]: https://github.com/AERO-Project-EU/gpu-atoll

use crate::iommu::{IommuMap, StreamId};
use crate::types::{ChipletId, TileId};

/// Software cap on a bytecode image. Not a hardware I-cache.
pub const MAX_INSNS: usize = 16;
/// Integer register file. `r0` is hard-wired zero (read-as-0, writes ignored).
pub const MAX_REGS: usize = 8;
/// SID pin ranges the verifier may see. Matches Soft-SMMU `MAX_MAPS` headroom.
pub const MAX_RANGES: usize = 8;
/// Modeled access width. Other sizes are [`SfiError::Unmodeled`].
pub const WORD: u32 = 4;

/// Soft-CP-shaped SSIDs (ssid = 1), distinct STEs. Not blast's ssid 0.
pub const SFI_SID_A: u32 = StreamId::accel(ChipletId(0), TileId(2), 1).0;
pub const SFI_SID_B: u32 = StreamId::accel(ChipletId(1), TileId(3), 1).0;

/// Toy clip windows (compact so the kernel self-check stays off a big stack).
pub const SFI_BASE_A: u64 = 0x00;
pub const SFI_BASE_B: u64 = 0x40;
pub const SFI_SPAN: u64 = 0x40;
/// Tenant B secret the OOB / skip-verify path must not observe.
pub const SFI_SECRET_B: u32 = 0xDEAD_BEEF;

/// Toy Soft-CP opcodes. Only the modeled four plus `Nop` verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SoftOp {
    Nop = 0,
    /// `rd = mem[rs + imm]` (word).
    Load = 1,
    /// `mem[rs + imm] = rd` (word).
    Store = 2,
    /// `rd = rs + rt` (no memory).
    Add = 3,
    /// `rd = rs + imm` (proves addresses; no memory).
    AddImm = 4,
    /// Copy `[rs, rs+imm)` → `[rd, rd+imm)` (both spans SID-proved).
    Dma = 5,
    /// Unmodeled. GPU-AToLL leaves atomics open; we refuse.
    AtomicAdd = 0x80,
    /// Unmodeled tensor / TMA-shaped copy. Refuse.
    Tensor = 0x81,
}

impl SoftOp {
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Nop),
            1 => Some(Self::Load),
            2 => Some(Self::Store),
            3 => Some(Self::Add),
            4 => Some(Self::AddImm),
            5 => Some(Self::Dma),
            0x80 => Some(Self::AtomicAdd),
            0x81 => Some(Self::Tensor),
            _ => None,
        }
    }

    pub const fn is_modeled(self) -> bool {
        matches!(
            self,
            Self::Nop | Self::Load | Self::Store | Self::Add | Self::AddImm | Self::Dma
        )
    }

    pub const fn touches_memory(self) -> bool {
        matches!(self, Self::Load | Self::Store | Self::Dma)
    }
}

/// Packed 8-byte instruction. Layout is the toy contract, not NVVM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insn {
    pub op: u8,
    pub rd: u8,
    pub rs: u8,
    pub rt: u8,
    /// Address / addend. u64 so Soft-SMMU IOVAs (above 4 GiB) are provable.
    pub imm: u64,
}

impl Insn {
    pub const fn encode(op: SoftOp, rd: u8, rs: u8, rt: u8, imm: u64) -> Self {
        Self {
            op: op as u8,
            rd,
            rs,
            rt,
            imm,
        }
    }

    pub const fn nop() -> Self {
        Self::encode(SoftOp::Nop, 0, 0, 0, 0)
    }

    pub const fn load(rd: u8, rs: u8, off: u64) -> Self {
        Self::encode(SoftOp::Load, rd, rs, 0, off)
    }

    pub const fn store(rd: u8, rs: u8, off: u64) -> Self {
        Self::encode(SoftOp::Store, rd, rs, 0, off)
    }

    pub const fn add(rd: u8, rs: u8, rt: u8) -> Self {
        Self::encode(SoftOp::Add, rd, rs, rt, 0)
    }

    pub const fn add_imm(rd: u8, rs: u8, imm: u64) -> Self {
        Self::encode(SoftOp::AddImm, rd, rs, 0, imm)
    }

    pub const fn dma(rd_dst: u8, rs_src: u8, len: u32) -> Self {
        Self::encode(SoftOp::Dma, rd_dst, rs_src, 0, len as u64)
    }

    pub const fn atomic_add(rd: u8, rs: u8, imm: u64) -> Self {
        Self::encode(SoftOp::AtomicAdd, rd, rs, 0, imm)
    }

    pub const fn tensor(rd: u8, rs: u8, imm: u64) -> Self {
        Self::encode(SoftOp::Tensor, rd, rs, 0, imm)
    }

    pub const fn opcode(self) -> Option<SoftOp> {
        SoftOp::from_u8(self.op)
    }
}

/// Fixed-size bytecode image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Program {
    insns: [Insn; MAX_INSNS],
    len: u8,
}

impl Program {
    pub const fn new() -> Self {
        Self {
            insns: [Insn::nop(); MAX_INSNS],
            len: 0,
        }
    }

    pub const fn len(self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, insn: Insn) -> Result<(), SfiError> {
        if self.len as usize >= MAX_INSNS {
            return Err(SfiError::BadInsn);
        }
        self.insns[self.len as usize] = insn;
        self.len += 1;
        Ok(())
    }

    pub fn insns(&self) -> &[Insn] {
        &self.insns[..self.len as usize]
    }
}

/// One SID-allowed window. `bound` is the length in bytes, not the end addr.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidRange {
    pub base: u64,
    pub bound: u64,
    pub writable: bool,
}

impl SidRange {
    pub const fn new(base: u64, bound: u64, writable: bool) -> Self {
        Self {
            base,
            bound,
            writable,
        }
    }

    /// Inclusive-exclusive `[base, base+bound)` contains `[start, end)`.
    pub const fn covers(self, start: u64, end: u64, write: bool) -> bool {
        if end <= start {
            return false;
        }
        if write && !self.writable {
            return false;
        }
        let range_end = match self.base.checked_add(self.bound) {
            Some(e) => e,
            None => return false,
        };
        start >= self.base && end <= range_end
    }
}

/// SID-allowed ranges the verifier / runtime trap consult.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidSandbox {
    pub sid: StreamId,
    ranges: [Option<SidRange>; MAX_RANGES],
    n: u8,
}

impl SidSandbox {
    pub const fn new(sid: StreamId) -> Self {
        Self {
            sid,
            ranges: [None; MAX_RANGES],
            n: 0,
        }
    }

    pub const fn len(self) -> usize {
        self.n as usize
    }

    pub fn push(&mut self, range: SidRange) -> Result<(), SfiError> {
        if range.bound == 0 || range.base.checked_add(range.bound).is_none() {
            return Err(SfiError::BadInsn);
        }
        if self.n as usize >= MAX_RANGES {
            return Err(SfiError::BadInsn);
        }
        self.ranges[self.n as usize] = Some(range);
        self.n += 1;
        Ok(())
    }

    /// Build from Soft-SMMU IOVA pins on `sid`. Program addresses are IOVAs.
    pub fn from_iommu(iommu: &IommuMap, sid: StreamId) -> Self {
        let mut s = Self::new(sid);
        for r in iommu.iter() {
            if r.stream_id == sid.raw() {
                let _ = s.push(SidRange::new(r.iova.0, r.len, r.writable));
            }
        }
        s
    }

    pub fn covers(&self, start: u64, end: u64, write: bool) -> bool {
        self.ranges
            .iter()
            .flatten()
            .any(|r| r.covers(start, end, write))
    }

    /// GPU-AToLL-shaped proof: a distinct location in this SID window.
    pub fn prove(
        &self,
        base: u64,
        off: u64,
        size: u32,
        write: bool,
    ) -> Result<(u64, u64), SfiError> {
        let (start, end) = span(base, off, size)?;
        if self.covers(start, end, write) {
            Ok((start, end))
        } else {
            Err(SfiError::Oob)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfiError {
    /// `base+off+size` is outside every SID-allowed range (or overflow).
    Oob,
    /// Atomic / tensor / heap / other unmodeled side-effect.
    Unmodeled,
    /// Unknown opcode, bad register, empty program, or illegal size.
    BadInsn,
    /// Address register is not a proved constant (GPU-AToLL: no location).
    UnknownBase,
}

impl SfiError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Oob => "Oob",
            Self::Unmodeled => "Unmodeled",
            Self::BadInsn => "BadInsn",
            Self::UnknownBase => "UnknownBase",
        }
    }
}

/// Backing the interpreter reads after the SID proof. Not a CUDA arena.
pub trait SfiMem {
    fn load_u32(&self, addr: u64) -> Result<u32, SfiError>;
    fn store_u32(&mut self, addr: u64, val: u32) -> Result<(), SfiError>;
}

/// Flat host / kernel clip memory. Addresses are sandbox linear, not IOVA.
pub struct FlatMem<'a> {
    pub base: u64,
    pub bytes: &'a mut [u8],
}

impl SfiMem for FlatMem<'_> {
    fn load_u32(&self, addr: u64) -> Result<u32, SfiError> {
        let off = addr.checked_sub(self.base).ok_or(SfiError::Oob)? as usize;
        if off
            .checked_add(4)
            .map(|e| e > self.bytes.len())
            .unwrap_or(true)
        {
            return Err(SfiError::Oob);
        }
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.bytes[off..off + 4]);
        Ok(u32::from_le_bytes(b))
    }

    fn store_u32(&mut self, addr: u64, val: u32) -> Result<(), SfiError> {
        let off = addr.checked_sub(self.base).ok_or(SfiError::Oob)? as usize;
        if off
            .checked_add(4)
            .map(|e| e > self.bytes.len())
            .unwrap_or(true)
        {
            return Err(SfiError::Oob);
        }
        self.bytes[off..off + 4].copy_from_slice(&val.to_le_bytes());
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SfiExec {
    pub regs: [u64; MAX_REGS],
    pub insns: u32,
}

impl SfiExec {
    pub const fn empty() -> Self {
        Self {
            regs: [0; MAX_REGS],
            insns: 0,
        }
    }
}

fn span(base: u64, off: u64, size: u32) -> Result<(u64, u64), SfiError> {
    if size == 0 {
        return Err(SfiError::BadInsn);
    }
    let start = base.checked_add(off).ok_or(SfiError::Oob)?;
    let end = start.checked_add(size as u64).ok_or(SfiError::Oob)?;
    Ok((start, end))
}

fn check_reg(r: u8) -> Result<usize, SfiError> {
    if (r as usize) < MAX_REGS {
        Ok(r as usize)
    } else {
        Err(SfiError::BadInsn)
    }
}

/// Static verifier: every load/store/dma proves `base+bound` in the SID window.
///
/// Registers start unknown except `r0 = 0`. `Add` / `AddImm` of constants
/// refine the abstract file. A memory op whose base is not a constant is
/// [`SfiError::UnknownBase`] (no distinct location). Atomics / tensor
/// / unknown ops are [`SfiError::Unmodeled`].
pub fn verify(prog: &Program, sandbox: &SidSandbox) -> Result<(), SfiError> {
    if prog.is_empty() {
        return Err(SfiError::BadInsn);
    }
    let mut abs: [Option<u64>; MAX_REGS] = [None; MAX_REGS];
    abs[0] = Some(0);
    for insn in prog.insns() {
        step_verify(*insn, sandbox, &mut abs)?;
    }
    Ok(())
}

fn step_verify(
    insn: Insn,
    sandbox: &SidSandbox,
    abs: &mut [Option<u64>; MAX_REGS],
) -> Result<(), SfiError> {
    let op = insn.opcode().ok_or(SfiError::Unmodeled)?;
    if !op.is_modeled() {
        return Err(SfiError::Unmodeled);
    }
    let rd = check_reg(insn.rd)?;
    let rs = check_reg(insn.rs)?;
    let rt = check_reg(insn.rt)?;
    match op {
        SoftOp::Nop => Ok(()),
        SoftOp::AddImm => {
            // Data addends may be unknown. Only memory ops require a proved base.
            let sum = match abs[rs] {
                Some(v) => v.checked_add(insn.imm),
                None => None,
            };
            if abs[rs].is_some() && sum.is_none() {
                return Err(SfiError::Oob);
            }
            write_abs(abs, rd, sum);
            Ok(())
        }
        SoftOp::Add => {
            let sum = match (abs[rs], abs[rt]) {
                (Some(a), Some(b)) => a.checked_add(b),
                _ => None,
            };
            if abs[rs].is_some() && abs[rt].is_some() && sum.is_none() {
                return Err(SfiError::Oob);
            }
            write_abs(abs, rd, sum);
            Ok(())
        }
        SoftOp::Load => {
            let base = abs[rs].ok_or(SfiError::UnknownBase)?;
            sandbox.prove(base, insn.imm, WORD, false)?;
            write_abs(abs, rd, None);
            Ok(())
        }
        SoftOp::Store => {
            let base = abs[rs].ok_or(SfiError::UnknownBase)?;
            sandbox.prove(base, insn.imm, WORD, true)?;
            Ok(())
        }
        SoftOp::Dma => {
            if insn.imm == 0 || insn.imm > u32::MAX as u64 || insn.imm % WORD as u64 != 0 {
                return Err(SfiError::BadInsn);
            }
            let dst = abs[rd].ok_or(SfiError::UnknownBase)?;
            let src = abs[rs].ok_or(SfiError::UnknownBase)?;
            sandbox.prove(src, 0, insn.imm as u32, false)?;
            sandbox.prove(dst, 0, insn.imm as u32, true)?;
            Ok(())
        }
        SoftOp::AtomicAdd | SoftOp::Tensor => Err(SfiError::Unmodeled),
    }
}

fn write_abs(abs: &mut [Option<u64>; MAX_REGS], rd: usize, v: Option<u64>) {
    if rd != 0 {
        abs[rd] = v;
    }
}

/// Run after [`verify`], or skip verify to **fault-inject**.
///
/// The SID sandbox still traps every memory op. A skip-verify OOB load
/// does not fill the dest register and does not read the foreign span.
pub fn execute<M: SfiMem>(
    prog: &Program,
    sandbox: &SidSandbox,
    mem: &mut M,
) -> Result<SfiExec, SfiError> {
    if prog.is_empty() {
        return Err(SfiError::BadInsn);
    }
    let mut regs = [0u64; MAX_REGS];
    let mut n = 0u32;
    for insn in prog.insns() {
        step_exec(*insn, sandbox, mem, &mut regs)?;
        n = n.saturating_add(1);
    }
    Ok(SfiExec { regs, insns: n })
}

/// Verify then execute. The usual Soft-CP submit path.
pub fn run<M: SfiMem>(
    prog: &Program,
    sandbox: &SidSandbox,
    mem: &mut M,
) -> Result<SfiExec, SfiError> {
    verify(prog, sandbox)?;
    execute(prog, sandbox, mem)
}

/// Skip the static verifier (fault injection). Runtime SID trap remains.
pub fn execute_unverified<M: SfiMem>(
    prog: &Program,
    sandbox: &SidSandbox,
    mem: &mut M,
) -> Result<SfiExec, SfiError> {
    execute(prog, sandbox, mem)
}

fn read_reg(regs: &[u64; MAX_REGS], r: usize) -> u64 {
    if r == 0 {
        0
    } else {
        regs[r]
    }
}

fn write_reg(regs: &mut [u64; MAX_REGS], r: usize, v: u64) {
    if r != 0 {
        regs[r] = v;
    }
}

fn step_exec<M: SfiMem>(
    insn: Insn,
    sandbox: &SidSandbox,
    mem: &mut M,
    regs: &mut [u64; MAX_REGS],
) -> Result<(), SfiError> {
    let op = insn.opcode().ok_or(SfiError::Unmodeled)?;
    if !op.is_modeled() {
        return Err(SfiError::Unmodeled);
    }
    let rd = check_reg(insn.rd)?;
    let rs = check_reg(insn.rs)?;
    let rt = check_reg(insn.rt)?;
    match op {
        SoftOp::Nop => Ok(()),
        SoftOp::AddImm => {
            let sum = read_reg(regs, rs)
                .checked_add(insn.imm)
                .ok_or(SfiError::Oob)?;
            write_reg(regs, rd, sum);
            Ok(())
        }
        SoftOp::Add => {
            let sum = read_reg(regs, rs)
                .checked_add(read_reg(regs, rt))
                .ok_or(SfiError::Oob)?;
            write_reg(regs, rd, sum);
            Ok(())
        }
        SoftOp::Load => {
            let base = read_reg(regs, rs);
            let (start, _) = sandbox.prove(base, insn.imm, WORD, false)?;
            let v = mem.load_u32(start)?;
            write_reg(regs, rd, v as u64);
            Ok(())
        }
        SoftOp::Store => {
            let base = read_reg(regs, rs);
            let (start, _) = sandbox.prove(base, insn.imm, WORD, true)?;
            mem.store_u32(start, read_reg(regs, rd) as u32)
        }
        SoftOp::Dma => {
            if insn.imm == 0 || insn.imm > u32::MAX as u64 || insn.imm % WORD as u64 != 0 {
                return Err(SfiError::BadInsn);
            }
            let dst = read_reg(regs, rd);
            let src = read_reg(regs, rs);
            sandbox.prove(src, 0, insn.imm as u32, false)?;
            sandbox.prove(dst, 0, insn.imm as u32, true)?;
            let words = insn.imm / WORD as u64;
            for i in 0..words {
                let s = src.checked_add(i * WORD as u64).ok_or(SfiError::Oob)?;
                let d = dst.checked_add(i * WORD as u64).ok_or(SfiError::Oob)?;
                let v = mem.load_u32(s)?;
                mem.store_u32(d, v)?;
            }
            Ok(())
        }
        SoftOp::AtomicAdd | SoftOp::Tensor => Err(SfiError::Unmodeled),
    }
}

/// Host-identical clip. Kernel prints `[softsfi] …`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftSfiReport {
    pub in_bounds: bool,
    pub oob_reject: bool,
    pub unmodeled_reject: bool,
    pub no_cross_read: bool,
}

impl SoftSfiReport {
    pub fn all_ok(&self) -> bool {
        self.in_bounds && self.oob_reject && self.unmodeled_reject && self.no_cross_read
    }
}

fn box_a() -> SidSandbox {
    let mut s = SidSandbox::new(StreamId::from_raw(SFI_SID_A));
    let _ = s.push(SidRange::new(SFI_BASE_A, SFI_SPAN, true));
    s
}

fn box_b() -> SidSandbox {
    let mut s = SidSandbox::new(StreamId::from_raw(SFI_SID_B));
    let _ = s.push(SidRange::new(SFI_BASE_B, SFI_SPAN, true));
    s
}

/// In-bounds: load A's word, add 1, store back. Bases are proved constants.
pub fn in_bounds_prog(base: u64) -> Program {
    let mut p = Program::new();
    let _ = p.push(Insn::add_imm(1, 0, base));
    let _ = p.push(Insn::load(2, 1, 0));
    let _ = p.push(Insn::add_imm(2, 2, 1));
    let _ = p.push(Insn::store(2, 1, 0));
    p
}

/// Load from an address outside this SID window (tenant B's base).
pub fn oob_load_prog(foreign_base: u64) -> Program {
    let mut p = Program::new();
    let _ = p.push(Insn::add_imm(1, 0, foreign_base));
    let _ = p.push(Insn::load(2, 1, 0));
    p
}

/// Two tenants, same toy Soft-CP ISA: accept in-bounds, reject OOB and
/// unmodeled ops, skip-verify fault injection does not cross-read B.
pub fn run_softsfi_demo() -> SoftSfiReport {
    let mut bytes = [0u8; 128];
    bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
    bytes[SFI_BASE_B as usize..SFI_BASE_B as usize + 4]
        .copy_from_slice(&SFI_SECRET_B.to_le_bytes());

    let a = box_a();
    let b = box_b();
    let inb = in_bounds_prog(SFI_BASE_A);
    let oob = oob_load_prog(SFI_BASE_B);

    let in_bounds = verify(&inb, &a).is_ok() && verify(&inb, &b).is_err();
    let mut mem = FlatMem {
        base: 0,
        bytes: &mut bytes,
    };
    let ran = run(&inb, &a, &mut mem);
    let in_bounds = in_bounds
        && ran.as_ref().map(|e| e.regs[2] == 2).unwrap_or(false)
        && u32::from_le_bytes(bytes[0..4].try_into().unwrap_or([0; 4])) == 2;

    let oob_reject = verify(&oob, &a) == Err(SfiError::Oob);

    let mut atom = Program::new();
    let _ = atom.push(Insn::add_imm(1, 0, SFI_BASE_A));
    let _ = atom.push(Insn::atomic_add(2, 1, 0));
    let mut tens = Program::new();
    let _ = tens.push(Insn::add_imm(1, 0, SFI_BASE_A));
    let _ = tens.push(Insn::tensor(2, 1, 4));
    let unmodeled_reject = verify(&atom, &a) == Err(SfiError::Unmodeled)
        && verify(&tens, &a) == Err(SfiError::Unmodeled);

    // Fault inject: skip verifier. Runtime SID trap; B's secret unread.
    let mut mem = FlatMem {
        base: 0,
        bytes: &mut bytes,
    };
    let injected = execute_unverified(&oob, &a, &mut mem);
    let secret = u32::from_le_bytes(
        bytes[SFI_BASE_B as usize..SFI_BASE_B as usize + 4]
            .try_into()
            .unwrap_or([0; 4]),
    );
    let no_cross_read = injected == Err(SfiError::Oob) && secret == SFI_SECRET_B;

    SoftSfiReport {
        in_bounds,
        oob_reject,
        unmodeled_reject,
        no_cross_read,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_with_secret() -> ([u8; 128], SidSandbox, SidSandbox) {
        let mut bytes = [0u8; 128];
        bytes[0..4].copy_from_slice(&7u32.to_le_bytes());
        bytes[SFI_BASE_B as usize..SFI_BASE_B as usize + 4]
            .copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        (bytes, box_a(), box_b())
    }

    #[test]
    fn verifier_accepts_in_bounds_program() {
        let a = box_a();
        let p = in_bounds_prog(SFI_BASE_A);
        assert_eq!(verify(&p, &a), Ok(()));
    }

    #[test]
    fn verifier_rejects_oob() {
        let a = box_a();
        let p = oob_load_prog(SFI_BASE_B);
        assert_eq!(verify(&p, &a), Err(SfiError::Oob));
        let mut over = Program::new();
        let _ = over.push(Insn::add_imm(1, 0, SFI_BASE_A));
        let _ = over.push(Insn::load(2, 1, SFI_SPAN));
        assert_eq!(verify(&over, &a), Err(SfiError::Oob));
    }

    #[test]
    fn verifier_rejects_atomics_and_tensor() {
        let a = box_a();
        let mut atom = Program::new();
        let _ = atom.push(Insn::atomic_add(1, 0, 0));
        let mut tens = Program::new();
        let _ = tens.push(Insn::tensor(1, 0, 16));
        assert_eq!(verify(&atom, &a), Err(SfiError::Unmodeled));
        assert_eq!(verify(&tens, &a), Err(SfiError::Unmodeled));
        let mut unk = Program::new();
        let _ = unk.push(Insn {
            op: 0xFF,
            rd: 0,
            rs: 0,
            rt: 0,
            imm: 0,
        });
        assert_eq!(verify(&unk, &a), Err(SfiError::Unmodeled));
    }

    #[test]
    fn verifier_rejects_unknown_base() {
        let a = box_a();
        let mut p = Program::new();
        // r3 never proved — GPU-AToLL: no distinct location.
        let _ = p.push(Insn::load(2, 3, 0));
        assert_eq!(verify(&p, &a), Err(SfiError::UnknownBase));
    }

    #[test]
    fn dma_in_bounds_copies_word() {
        let (mut bytes, a, _) = mem_with_secret();
        let mut p = Program::new();
        let _ = p.push(Insn::add_imm(1, 0, SFI_BASE_A));
        let _ = p.push(Insn::add_imm(2, 0, SFI_BASE_A + 8));
        let _ = p.push(Insn::dma(2, 1, WORD));
        let mut mem = FlatMem {
            base: 0,
            bytes: &mut bytes,
        };
        run(&p, &a, &mut mem).unwrap();
        let copied = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        assert_eq!(copied, 7);
    }

    #[test]
    fn dma_oob_rejected() {
        let a = box_a();
        let mut p = Program::new();
        let _ = p.push(Insn::add_imm(1, 0, SFI_BASE_A));
        let _ = p.push(Insn::add_imm(2, 0, SFI_BASE_B));
        let _ = p.push(Insn::dma(2, 1, WORD));
        assert_eq!(verify(&p, &a), Err(SfiError::Oob));
    }

    #[test]
    fn skip_verify_oob_does_not_cross_read() {
        let (mut bytes, a, _) = mem_with_secret();
        let p = oob_load_prog(SFI_BASE_B);
        let mut mem = FlatMem {
            base: 0,
            bytes: &mut bytes,
        };
        assert_eq!(execute_unverified(&p, &a, &mut mem), Err(SfiError::Oob));
        let secret = u32::from_le_bytes(
            bytes[SFI_BASE_B as usize..SFI_BASE_B as usize + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(secret, SFI_SECRET_B);
    }

    #[test]
    fn two_tenants_same_isa_distinct_sids() {
        assert_ne!(SFI_SID_A, SFI_SID_B);
        let a = box_a();
        let b = box_b();
        assert_ne!(a.sid, b.sid);
        let pa = in_bounds_prog(SFI_BASE_A);
        let pb = in_bounds_prog(SFI_BASE_B);
        assert!(verify(&pa, &a).is_ok());
        assert_eq!(verify(&pa, &b), Err(SfiError::Oob));
        assert!(verify(&pb, &b).is_ok());
        assert_eq!(verify(&pb, &a), Err(SfiError::Oob));
    }

    #[test]
    fn softsfi_demo_two_tenant_fault_inject() {
        let r = run_softsfi_demo();
        assert!(r.in_bounds, "in-bounds accept");
        assert!(r.oob_reject, "OOB reject");
        assert!(r.unmodeled_reject, "atomic/tensor refuse");
        assert!(r.no_cross_read, "skip-verify does not leak B");
        assert!(r.all_ok());
    }

    #[test]
    fn not_nvvm_and_not_safe_multitenant_kernels() {
        // Honest bound: heap is unmodeled; we do not invent malloc.
        let a = box_a();
        assert_eq!(a.len(), 1);
        assert!(!SoftOp::AtomicAdd.is_modeled());
        assert!(!SoftOp::Tensor.is_modeled());
        assert!(SoftOp::Load.touches_memory());
        assert!(!SoftOp::Add.touches_memory());
    }
}
