//! OperatorInject: Soft-CP resident worker + versioned operator table.
//!
//! [GPUOS][gpuos] / [Mirage MPK][mirage] inspiration: one resident worker
//! stays up; the host publishes operator slots (a versioned function table);
//! a new fused op **hot-adds** without relaunch. Own bytecode / IR only —
//! **not** NVRTC, **not** CUDA, **not** a full LLM compiler, **not** NVIDIA.
//! Distinct from [`crate::opkernel::OperatorKernelHandle`] (Hodge collective
//! inject).
//!
//! Built-in slots: [`InjectKind::Memcpy`] and [`InjectKind::Saxpy`]. The
//! third slot ([`InjectKind::Scale`]) is unpublished until
//! [`OperatorInject::hot_add_scale`]. SID windows still gate every submit
//! ([`crate::softsfi::SidSandbox`]).
//!
//! [gpuos]: https://github.com/XpuOS
//! [mirage]: https://github.com/mirage-project/mirage
//!
//! Mirage's persistent kernel (MPK) fuses tensor ops into a resident GPU
//! kernel so a new fused graph does not relaunch CUDA. GPUOS / XpuOS is the
//! resident-service shape: a live worker that receives host-published work.
//! This crate copies **neither** — a software table + loop on Soft-CP.

use crate::iommu::StreamId;
use crate::softsfi::{SidSandbox, WORD};
use crate::types::{ChipletId, TileId};

/// Software cap on the function table. Not a hardware I-cache.
pub const MAX_OP_SLOTS: usize = 4;
/// Packed call image. Own IR, not NVVM / PTX.
pub const OP_CALL_SIZE: usize = 32;
/// Canonical memcpy / saxpy / scale length (I32 words). Compact kernel clip.
pub const DEMO_WORDS: u32 = 4;

/// Soft-CP-shaped SSID (ssid = 1). Same family as SoftSFI, not blast ssid 0.
pub const OPINJECT_SID: u32 = StreamId::accel(ChipletId(0), TileId(2), 1).0;
/// Toy clip window (src at 0, dst at 16). Kernel self-check stays off a big stack.
pub const OPINJECT_BASE: u64 = 0x00;
pub const OPINJECT_SPAN: u64 = 0x20;
pub const OPINJECT_DST: u64 = 0x10;

pub const SLOT_MEMCPY: u8 = 0;
pub const SLOT_SAXPY: u8 = 1;
/// Unpublished until [`OperatorInject::hot_add_scale`].
pub const SLOT_SCALE: u8 = 2;

/// Injectable operator. Own ISA — not a CUDA kernel and not SoftNPU `AccelOp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InjectKind {
    /// `dst[i] = src[i]` (word copy).
    Memcpy = 0,
    /// `dst[i] = alpha * src[i] + dst[i]` (SAXPY).
    Saxpy = 1,
    /// `dst[i] = alpha * src[i]`. Hot-added third; unpublished at seed.
    Scale = 2,
}

impl InjectKind {
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Memcpy),
            1 => Some(Self::Saxpy),
            2 => Some(Self::Scale),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Memcpy => "memcpy",
            Self::Saxpy => "saxpy",
            Self::Scale => "scale",
        }
    }

    pub const fn slot(self) -> u8 {
        self as u8
    }
}

/// One versioned function-table entry. Host publishes; the resident worker
/// looks up by slot + version. Not an ELF, not NVRTC cubin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpSlot {
    pub kind: InjectKind,
    pub version: u32,
    pub published: bool,
}

impl OpSlot {
    pub const fn empty() -> Self {
        Self {
            kind: InjectKind::Memcpy,
            version: 0,
            published: false,
        }
    }
}

/// Versioned operator table the resident worker dispatches through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpTable {
    slots: [OpSlot; MAX_OP_SLOTS],
    pub table_version: u32,
}

impl OpTable {
    pub const fn new() -> Self {
        Self {
            slots: [OpSlot::empty(); MAX_OP_SLOTS],
            table_version: 0,
        }
    }

    pub fn get(self, slot: u8) -> Option<OpSlot> {
        let i = slot as usize;
        if i >= MAX_OP_SLOTS {
            return None;
        }
        let s = self.slots[i];
        if s.published {
            Some(s)
        } else {
            None
        }
    }

    /// Publish or replace a slot. Bumps `table_version`. Does **not**
    /// relaunch the worker — that is the hot-add contract.
    pub fn publish(&mut self, slot: u8, kind: InjectKind) -> Result<u32, InjectError> {
        let i = slot as usize;
        if i >= MAX_OP_SLOTS {
            return Err(InjectError::UnknownSlot);
        }
        if kind.slot() != slot {
            return Err(InjectError::BadArg);
        }
        let prev = self.slots[i];
        self.slots[i] = OpSlot {
            kind,
            version: prev.version.saturating_add(1).max(1),
            published: true,
        };
        self.table_version = self.table_version.saturating_add(1);
        Ok(self.slots[i].version)
    }
}

/// One resident worker. `epoch` / `launches` increment only on
/// [`ResidentWorker::start`], never on a table publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentWorker {
    pub epoch: u32,
    pub launches: u32,
    pub running: bool,
    pub table_seen: u32,
    pub calls: u32,
}

impl ResidentWorker {
    pub const fn new() -> Self {
        Self {
            epoch: 0,
            launches: 0,
            running: false,
            table_seen: 0,
            calls: 0,
        }
    }

    pub fn start(&mut self) -> Result<(), InjectError> {
        if self.running {
            return Err(InjectError::Busy);
        }
        self.epoch = self.epoch.saturating_add(1);
        self.launches = self.launches.saturating_add(1);
        self.running = true;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    /// Observe a newer table. Not a relaunch.
    pub fn observe(&mut self, table: &OpTable) {
        self.table_seen = table.table_version;
    }
}

/// Packed 32-byte call. Layout is the toy contract, not NVVM.
///
/// ```text
///  0     slot u8
///  1     kind u8
///  2..4  pad
///  4..8  version u32
///  8..12 n u32          (I32 words)
/// 12..16 alpha i32      (saxpy / scale)
/// 16..24 src u64        (IOVA / sandbox linear)
/// 24..32 dst u64
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpCall {
    pub slot: u8,
    pub kind: InjectKind,
    pub version: u32,
    pub n: u32,
    pub alpha: i32,
    pub src: u64,
    pub dst: u64,
}

impl OpCall {
    pub const fn new(
        slot: u8,
        kind: InjectKind,
        version: u32,
        n: u32,
        alpha: i32,
        src: u64,
        dst: u64,
    ) -> Self {
        Self {
            slot,
            kind,
            version,
            n,
            alpha,
            src,
            dst,
        }
    }

    pub fn memcpy(version: u32, n: u32, src: u64, dst: u64) -> Self {
        Self::new(SLOT_MEMCPY, InjectKind::Memcpy, version, n, 0, src, dst)
    }

    pub fn saxpy(version: u32, n: u32, alpha: i32, src: u64, dst: u64) -> Self {
        Self::new(SLOT_SAXPY, InjectKind::Saxpy, version, n, alpha, src, dst)
    }

    pub fn scale(version: u32, n: u32, alpha: i32, src: u64, dst: u64) -> Self {
        Self::new(SLOT_SCALE, InjectKind::Scale, version, n, alpha, src, dst)
    }

    pub fn to_le_bytes(self) -> [u8; OP_CALL_SIZE] {
        let mut b = [0u8; OP_CALL_SIZE];
        b[0] = self.slot;
        b[1] = self.kind as u8;
        b[4..8].copy_from_slice(&self.version.to_le_bytes());
        b[8..12].copy_from_slice(&self.n.to_le_bytes());
        b[12..16].copy_from_slice(&self.alpha.to_le_bytes());
        b[16..24].copy_from_slice(&self.src.to_le_bytes());
        b[24..32].copy_from_slice(&self.dst.to_le_bytes());
        b
    }

    pub fn from_le_bytes(b: [u8; OP_CALL_SIZE]) -> Result<Self, InjectError> {
        let kind = InjectKind::from_u8(b[1]).ok_or(InjectError::BadArg)?;
        if b[0] != kind.slot() {
            return Err(InjectError::BadArg);
        }
        Ok(Self {
            slot: b[0],
            kind,
            version: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            n: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            alpha: i32::from_le_bytes(b[12..16].try_into().unwrap()),
            src: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            dst: u64::from_le_bytes(b[24..32].try_into().unwrap()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectError {
    /// Worker is not in the resident loop.
    NotRunning,
    /// `start` while already running. Hot-add must not take this path.
    Busy,
    /// Slot empty, out of range, or kind/slot mismatch.
    UnknownSlot,
    /// Call version does not match the published slot.
    StaleVersion,
    /// Src/dst span escapes the SID window.
    Oob,
    /// n = 0, overflow, or malformed bytecode.
    BadArg,
}

impl InjectError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRunning => "NotRunning",
            Self::Busy => "Busy",
            Self::UnknownSlot => "UnknownSlot",
            Self::StaleVersion => "StaleVersion",
            Self::Oob => "Oob",
            Self::BadArg => "BadArg",
        }
    }
}

/// Word-addressed backing. Soft-CP maps IOVA → PA before this trait.
pub trait OpMem {
    fn load_i32(&self, addr: u64) -> Result<i32, InjectError>;
    fn store_i32(&mut self, addr: u64, val: i32) -> Result<(), InjectError>;
}

/// Flat host / kernel clip memory. Addresses are sandbox linear, not IOVA.
pub struct FlatOpMem<'a> {
    pub base: u64,
    pub bytes: &'a mut [u8],
}

impl OpMem for FlatOpMem<'_> {
    fn load_i32(&self, addr: u64) -> Result<i32, InjectError> {
        let off = addr.checked_sub(self.base).ok_or(InjectError::Oob)? as usize;
        if off
            .checked_add(4)
            .map(|e| e > self.bytes.len())
            .unwrap_or(true)
        {
            return Err(InjectError::Oob);
        }
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.bytes[off..off + 4]);
        Ok(i32::from_le_bytes(b))
    }

    fn store_i32(&mut self, addr: u64, val: i32) -> Result<(), InjectError> {
        let off = addr.checked_sub(self.base).ok_or(InjectError::Oob)? as usize;
        if off
            .checked_add(4)
            .map(|e| e > self.bytes.len())
            .unwrap_or(true)
        {
            return Err(InjectError::Oob);
        }
        self.bytes[off..off + 4].copy_from_slice(&val.to_le_bytes());
        Ok(())
    }
}

/// Resident worker + versioned table. One of these lives on Soft-CP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperatorInject {
    pub table: OpTable,
    pub worker: ResidentWorker,
}

impl OperatorInject {
    pub const fn new() -> Self {
        Self {
            table: OpTable::new(),
            worker: ResidentWorker::new(),
        }
    }

    /// Canonical Soft-CP seed: one resident loop, memcpy + saxpy published.
    pub fn with_resident_memcpy_saxpy() -> Self {
        let mut s = Self::new();
        let _ = s.start();
        let _ = s.publish(SLOT_MEMCPY, InjectKind::Memcpy);
        let _ = s.publish(SLOT_SAXPY, InjectKind::Saxpy);
        s
    }

    pub fn start(&mut self) -> Result<(), InjectError> {
        self.worker.start()?;
        self.worker.observe(&self.table);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.worker.stop();
    }

    pub const fn running(&self) -> bool {
        self.worker.running
    }

    pub const fn epoch(&self) -> u32 {
        self.worker.epoch
    }

    pub const fn launches(&self) -> u32 {
        self.worker.launches
    }

    pub fn slot_version(&self, slot: u8) -> Option<u32> {
        self.table.get(slot).map(|s| s.version)
    }

    pub fn publish(&mut self, slot: u8, kind: InjectKind) -> Result<u32, InjectError> {
        let v = self.table.publish(slot, kind)?;
        if self.worker.running {
            self.worker.observe(&self.table);
        }
        Ok(v)
    }

    /// Hot-add the third op. Worker epoch / launches must not change.
    pub fn hot_add_scale(&mut self) -> Result<u32, InjectError> {
        if !self.worker.running {
            return Err(InjectError::NotRunning);
        }
        self.publish(SLOT_SCALE, InjectKind::Scale)
    }

    /// SID-gated dispatch through the resident worker. Copy-then-validate of
    /// the packed [`OpCall`] belongs on the Soft-CP host (SoftCmdFirewall).
    pub fn submit(
        &mut self,
        call: &OpCall,
        sandbox: &SidSandbox,
        mem: &mut impl OpMem,
    ) -> Result<(), InjectError> {
        if !self.worker.running {
            return Err(InjectError::NotRunning);
        }
        if call.n == 0 {
            return Err(InjectError::BadArg);
        }
        let slot = self.table.get(call.slot).ok_or(InjectError::UnknownSlot)?;
        if slot.kind != call.kind {
            return Err(InjectError::UnknownSlot);
        }
        if slot.version != call.version {
            return Err(InjectError::StaleVersion);
        }
        let bytes = (call.n as u64).saturating_mul(WORD as u64);
        prove_span(sandbox, call.src, bytes, false)?;
        prove_span(sandbox, call.dst, bytes, true)?;
        run_op(call, mem)?;
        self.worker.calls = self.worker.calls.saturating_add(1);
        self.worker.observe(&self.table);
        Ok(())
    }
}

fn prove_span(sandbox: &SidSandbox, base: u64, bytes: u64, write: bool) -> Result<(), InjectError> {
    if bytes == 0 {
        return Err(InjectError::BadArg);
    }
    let end = base.checked_add(bytes).ok_or(InjectError::Oob)?;
    if sandbox.covers(base, end, write) {
        Ok(())
    } else {
        Err(InjectError::Oob)
    }
}

fn run_op(call: &OpCall, mem: &mut impl OpMem) -> Result<(), InjectError> {
    for i in 0..call.n {
        let off = (i as u64).saturating_mul(WORD as u64);
        let src = call.src.checked_add(off).ok_or(InjectError::Oob)?;
        let dst = call.dst.checked_add(off).ok_or(InjectError::Oob)?;
        let x = mem.load_i32(src)?;
        let y = match call.kind {
            InjectKind::Memcpy => x,
            InjectKind::Saxpy => {
                let old = mem.load_i32(dst)?;
                call.alpha.wrapping_mul(x).wrapping_add(old)
            }
            InjectKind::Scale => call.alpha.wrapping_mul(x),
        };
        mem.store_i32(dst, y)?;
    }
    Ok(())
}

fn toy_sandbox() -> SidSandbox {
    use crate::softsfi::SidRange;
    let mut s = SidSandbox::new(StreamId::from_raw(OPINJECT_SID));
    let _ = s.push(SidRange::new(OPINJECT_BASE, OPINJECT_SPAN, true));
    s
}

/// Host-identical clip. Kernel prints `[opinject] …`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpInjectReport {
    pub memcpy_ok: bool,
    pub saxpy_ok: bool,
    pub scale_refused_before: bool,
    pub hot_add_no_relaunch: bool,
    pub scale_ok: bool,
    pub sid_oob: bool,
}

impl OpInjectReport {
    pub fn all_ok(&self) -> bool {
        self.memcpy_ok
            && self.saxpy_ok
            && self.scale_refused_before
            && self.hot_add_no_relaunch
            && self.scale_ok
            && self.sid_oob
    }
}

fn read_i32(bytes: &[u8], off: usize) -> i32 {
    let mut b = [0u8; 4];
    if off + 4 <= bytes.len() {
        b.copy_from_slice(&bytes[off..off + 4]);
    }
    i32::from_le_bytes(b)
}

/// Resident memcpy + saxpy, hot-add scale without relaunch, SID OOB refuse.
#[inline(never)]
pub fn run_opinject_demo() -> OpInjectReport {
    let mut bytes = [0u8; OPINJECT_SPAN as usize];
    for i in 0..DEMO_WORDS {
        let off = (i as usize) * 4;
        bytes[off..off + 4].copy_from_slice(&(i as i32 + 1).to_le_bytes());
    }

    let sandbox = toy_sandbox();
    let mut inj = OperatorInject::with_resident_memcpy_saxpy();
    let epoch0 = inj.epoch();
    let launches0 = inj.launches();

    let mv = inj.slot_version(SLOT_MEMCPY).unwrap_or(0);
    let sv = inj.slot_version(SLOT_SAXPY).unwrap_or(0);
    {
        let mut mem = FlatOpMem {
            base: OPINJECT_BASE,
            bytes: &mut bytes,
        };
        let memcpy_ok = inj
            .submit(
                &OpCall::memcpy(mv, DEMO_WORDS, OPINJECT_BASE, OPINJECT_DST),
                &sandbox,
                &mut mem,
            )
            .is_ok();
        drop(mem);
        let memcpy_ok = memcpy_ok
            && read_i32(&bytes, OPINJECT_DST as usize) == 1
            && read_i32(&bytes, OPINJECT_DST as usize + 12) == 4;

        let before_scale = inj.submit(
            &OpCall::scale(1, DEMO_WORDS, 3, OPINJECT_BASE, OPINJECT_DST),
            &sandbox,
            &mut FlatOpMem {
                base: OPINJECT_BASE,
                bytes: &mut bytes,
            },
        );
        let scale_refused_before = before_scale == Err(InjectError::UnknownSlot);

        let saxpy_ok = inj
            .submit(
                &OpCall::saxpy(sv, DEMO_WORDS, 2, OPINJECT_BASE, OPINJECT_DST),
                &sandbox,
                &mut FlatOpMem {
                    base: OPINJECT_BASE,
                    bytes: &mut bytes,
                },
            )
            .is_ok();
        // dst was a copy of src (1,2,3,4); saxpy α=2 → 2*src+dst = 3*(1,2,3,4)
        let saxpy_ok = saxpy_ok
            && read_i32(&bytes, OPINJECT_DST as usize) == 3
            && read_i32(&bytes, OPINJECT_DST as usize + 4) == 6;

        let hot = inj.hot_add_scale();
        let hot_add_no_relaunch = hot.is_ok()
            && inj.epoch() == epoch0
            && inj.launches() == launches0
            && inj.running()
            && launches0 == 1;

        let scv = inj.slot_version(SLOT_SCALE).unwrap_or(0);
        let scale_ok = inj
            .submit(
                &OpCall::scale(scv, DEMO_WORDS, 10, OPINJECT_BASE, OPINJECT_DST),
                &sandbox,
                &mut FlatOpMem {
                    base: OPINJECT_BASE,
                    bytes: &mut bytes,
                },
            )
            .is_ok();
        let scale_ok = scale_ok && read_i32(&bytes, OPINJECT_DST as usize) == 10;

        let sid_oob = inj.submit(
            &OpCall::memcpy(mv, DEMO_WORDS, OPINJECT_BASE, OPINJECT_SPAN),
            &sandbox,
            &mut FlatOpMem {
                base: OPINJECT_BASE,
                bytes: &mut bytes,
            },
        ) == Err(InjectError::Oob);

        OpInjectReport {
            memcpy_ok,
            saxpy_ok,
            scale_refused_before,
            hot_add_no_relaunch,
            scale_ok,
            sid_oob,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::softsfi::SidRange;

    fn sandbox() -> SidSandbox {
        toy_sandbox()
    }

    #[test]
    fn seed_publishes_memcpy_saxpy_not_scale() {
        let inj = OperatorInject::with_resident_memcpy_saxpy();
        assert!(inj.running());
        assert_eq!(inj.launches(), 1);
        assert!(inj.table.get(SLOT_MEMCPY).is_some());
        assert!(inj.table.get(SLOT_SAXPY).is_some());
        assert!(inj.table.get(SLOT_SCALE).is_none());
    }

    #[test]
    fn memcpy_and_saxpy_on_resident_worker() {
        let r = run_opinject_demo();
        assert!(r.memcpy_ok);
        assert!(r.saxpy_ok);
        assert!(r.all_ok());
    }

    #[test]
    fn hot_add_scale_does_not_relaunch() {
        let mut inj = OperatorInject::with_resident_memcpy_saxpy();
        let epoch = inj.epoch();
        inj.hot_add_scale().unwrap();
        assert_eq!(inj.epoch(), epoch);
        assert_eq!(inj.launches(), 1);
        assert!(inj.table.get(SLOT_SCALE).is_some());
        // A real relaunch would bump launches.
        inj.stop();
        inj.start().unwrap();
        assert_eq!(inj.launches(), 2);
        assert_eq!(inj.epoch(), epoch + 1);
    }

    #[test]
    fn stale_version_and_unknown_slot_refuse() {
        let mut bytes = [0u8; 32];
        let mut inj = OperatorInject::with_resident_memcpy_saxpy();
        let mut mem = FlatOpMem {
            base: 0,
            bytes: &mut bytes,
        };
        let sb = sandbox();
        assert_eq!(
            inj.submit(&OpCall::memcpy(99, 1, 0, 16), &sb, &mut mem),
            Err(InjectError::StaleVersion)
        );
        assert_eq!(
            inj.submit(&OpCall::scale(1, 1, 1, 0, 16), &sb, &mut mem),
            Err(InjectError::UnknownSlot)
        );
    }

    #[test]
    fn sid_oob_refused() {
        let mut bytes = [0u8; 32];
        let mut inj = OperatorInject::with_resident_memcpy_saxpy();
        let mv = inj.slot_version(SLOT_MEMCPY).unwrap();
        let mut mem = FlatOpMem {
            base: 0,
            bytes: &mut bytes,
        };
        let mut tight = SidSandbox::new(StreamId::from_raw(OPINJECT_SID));
        let _ = tight.push(SidRange::new(0, 8, true));
        assert_eq!(
            inj.submit(&OpCall::memcpy(mv, 4, 0, 16), &tight, &mut mem),
            Err(InjectError::Oob)
        );
    }

    #[test]
    fn bytecode_roundtrip() {
        let c = OpCall::saxpy(3, 8, -2, 0x1000, 0x2000);
        assert_eq!(OpCall::from_le_bytes(c.to_le_bytes()).unwrap(), c);
    }

    #[test]
    fn demo_report_all_ok() {
        assert!(run_opinject_demo().all_ok());
    }
}
