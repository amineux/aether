//! SoftCmdFirewall — copy-then-validate on the Soft-CP submit path.
//!
//! Host1x lesson: if the kernel validates a userspace command buffer
//! **in place**, a client can rewrite opcodes / relocs / SID / addresses
//! after the check and before enqueue. The hardware (or `service()`)
//! then sees the mutated stream. Tegra Host1x DRM learned this the hard
//! way; the fix is to **copy into a kernel-owned arena first**, then
//! validate the copy, then enqueue the copy.
//!
//! This is **integrity of the command stream only**. It is not
//! confidential GPU, not HBM encryption, not NVIDIA SEC2, and not a
//! Tegra Host1x driver. A later GPU-CC HMAC over the kernel copy is
//! optional integrity — not this slice.
//!
//! Software step counts (`copy_steps` / `validate_steps`) are a sim
//! note, not vendor microseconds.

use aether_core::accel::{AccelJobDesc, AccelOp, DType};
use aether_core::iommu::{IommuMap, StreamId, SOFT_SMMU_IOVA_BASE};
use aether_core::phase::Phase;
use aether_core::space::{MemorySpace, Place};
use aether_core::types::{ChipletId, PhysAddr};
use aether_hal::HalError;

use crate::fakecp::{CpCmd, CP_CMD_SIZE, CP_FLAG_HAS_BIAS, CP_FLAG_SET_SID, CP_PKT_MAGIC, CP_SSID};

/// Known `CpCmd.flags` bits. Anything else is a Fault.
pub const CP_KNOWN_FLAGS: u16 = CP_FLAG_HAS_BIAS | CP_FLAG_SET_SID;

/// Wire offsets (same contract as `docs/ACCEL.md`). Relocs live here.
pub const CP_OFF_MAGIC: usize = 0x00;
pub const CP_OFF_OPCODE: usize = 0x04;
pub const CP_OFF_DTYPE: usize = 0x05;
pub const CP_OFF_SPACE: usize = 0x06;
pub const CP_OFF_PHASE: usize = 0x07;
pub const CP_OFF_FLAGS: usize = 0x0E;
pub const CP_OFF_STREAM_ID: usize = 0x10;
pub const CP_OFF_IOVA_A: usize = 0x18;
pub const CP_OFF_IOVA_B: usize = 0x20;
pub const CP_OFF_IOVA_C: usize = 0x28;
pub const CP_OFF_IOVA_BIAS: usize = 0x30;

/// Kernel-owned snapshot slots. Depth is software; not a HW ring.
pub const FIREWALL_SLOTS: usize = 2;

/// Userspace-owned command image. The client may rewrite it between
/// [`ClientCmdStream::read_at`] calls (the Host1x race).
pub trait ClientCmdStream {
    fn len(&self) -> usize;
    fn read_at(&mut self, off: usize, dst: &mut [u8]) -> Result<(), HalError>;
}

/// Borrowed client bytes. No race unless the caller mutates the slice
/// through another alias (tests use [`RacingCmdStream`]).
pub struct SliceCmdStream<'a> {
    pub bytes: &'a [u8],
}

impl ClientCmdStream for SliceCmdStream<'_> {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&mut self, off: usize, dst: &mut [u8]) -> Result<(), HalError> {
        let end = off.checked_add(dst.len()).ok_or(HalError::BadArg)?;
        let src = self.bytes.get(off..end).ok_or(HalError::BadArg)?;
        dst.copy_from_slice(src);
        Ok(())
    }
}

/// Malicious client: after `mutate_after` reads, the live image becomes
/// `poison`. Models a rewrite during the validate window.
pub struct RacingCmdStream {
    live: [u8; CP_CMD_SIZE],
    poison: [u8; CP_CMD_SIZE],
    reads: u32,
    mutate_after: u32,
    mutated: bool,
}

impl RacingCmdStream {
    pub fn new(good: [u8; CP_CMD_SIZE], poison: [u8; CP_CMD_SIZE], mutate_after: u32) -> Self {
        Self {
            live: good,
            poison,
            reads: 0,
            mutate_after,
            mutated: false,
        }
    }

    pub fn reads(&self) -> u32 {
        self.reads
    }

    pub fn mutated(&self) -> bool {
        self.mutated
    }
}

impl ClientCmdStream for RacingCmdStream {
    fn len(&self) -> usize {
        CP_CMD_SIZE
    }

    fn read_at(&mut self, off: usize, dst: &mut [u8]) -> Result<(), HalError> {
        let end = off.checked_add(dst.len()).ok_or(HalError::BadArg)?;
        if end > CP_CMD_SIZE {
            return Err(HalError::BadArg);
        }
        dst.copy_from_slice(&self.live[off..end]);
        self.reads = self.reads.saturating_add(1);
        // Rewrite *after* this read. Copy-then-validate already has its
        // snapshot; in-place re-read sees poison.
        if self.reads >= self.mutate_after {
            self.live = self.poison;
            self.mutated = true;
        }
        Ok(())
    }
}

/// How submit ingests a client image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirewallMode {
    /// Host1x lesson: snapshot, then validate the snapshot, then enqueue
    /// the snapshot. Default. Mutation of the client buffer is ignored.
    CopyThenValidate,
    /// Insecure comparison path: validate one client read, then re-read
    /// for enqueue. A rewrite in the window sneaks. Test-only.
    ValidateInPlace,
}

/// Software step counters. Not vendor microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FirewallSim {
    pub copy_steps: u32,
    pub validate_steps: u32,
}

impl FirewallSim {
    /// Copy-then-validate always pays a snapshot plus a walk of the copy.
    pub const fn noted(&self) -> bool {
        self.copy_steps > 0 && self.validate_steps > 0
    }
}

/// Kernel-owned command-stream arena.
#[derive(Clone, Copy, Debug)]
pub struct SoftCmdFirewall {
    slots: [[u8; CP_CMD_SIZE]; FIREWALL_SLOTS],
    used: [bool; FIREWALL_SLOTS],
    head: u8,
    pub mode: FirewallMode,
    pub last_sim: FirewallSim,
    last_len: u8,
}

impl Default for SoftCmdFirewall {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftCmdFirewall {
    pub const fn new() -> Self {
        Self {
            slots: [[0u8; CP_CMD_SIZE]; FIREWALL_SLOTS],
            used: [false; FIREWALL_SLOTS],
            head: 0,
            mode: FirewallMode::CopyThenValidate,
            last_sim: FirewallSim {
                copy_steps: 0,
                validate_steps: 0,
            },
            last_len: 0,
        }
    }

    pub const fn last_image(&self) -> Option<&[u8; CP_CMD_SIZE]> {
        if self.last_len == 0 {
            None
        } else {
            let i = (self.head as usize + FIREWALL_SLOTS - 1) % FIREWALL_SLOTS;
            if self.used[i] {
                Some(&self.slots[i])
            } else {
                None
            }
        }
    }

    fn store(&mut self, img: [u8; CP_CMD_SIZE]) {
        let i = self.head as usize % FIREWALL_SLOTS;
        self.slots[i] = img;
        self.used[i] = true;
        self.head = ((i + 1) % FIREWALL_SLOTS) as u8;
        self.last_len = CP_CMD_SIZE as u8;
    }

    /// Snapshot a already-packed kernel `CpCmd` (golden Soft-CP path).
    pub fn admit_packed(
        &mut self,
        cmd: CpCmd,
        iommu: &IommuMap,
        expected_sid: Option<StreamId>,
    ) -> Result<CpCmd, HalError> {
        let img = cmd.to_le_bytes();
        self.store(img);
        self.last_sim.copy_steps = 1;
        let copy = CpCmd::from_le_bytes(img)?;
        self.last_sim.validate_steps = validate_cp_cmd(&copy, iommu, expected_sid)?;
        Ok(copy)
    }

    /// Ingest a userspace image. Copy-then-validate by default.
    pub fn ingest<S: ClientCmdStream>(
        &mut self,
        stream: &mut S,
        iommu: &IommuMap,
        expected_sid: Option<StreamId>,
    ) -> Result<CpCmd, HalError> {
        match self.mode {
            FirewallMode::CopyThenValidate => self.ingest_copy(stream, iommu, expected_sid),
            FirewallMode::ValidateInPlace => self.ingest_in_place(stream, iommu, expected_sid),
        }
    }

    fn read_full<S: ClientCmdStream>(
        stream: &mut S,
        img: &mut [u8; CP_CMD_SIZE],
    ) -> Result<(), HalError> {
        if stream.len() < CP_CMD_SIZE {
            return Err(HalError::BadArg);
        }
        stream.read_at(0, img)
    }

    fn ingest_copy<S: ClientCmdStream>(
        &mut self,
        stream: &mut S,
        iommu: &IommuMap,
        expected_sid: Option<StreamId>,
    ) -> Result<CpCmd, HalError> {
        let mut img = [0u8; CP_CMD_SIZE];
        Self::read_full(stream, &mut img)?;
        self.store(img);
        self.last_sim.copy_steps = 1;
        let cmd = CpCmd::from_le_bytes(img)?;
        self.last_sim.validate_steps = validate_cp_cmd(&cmd, iommu, expected_sid)?;
        Ok(cmd)
    }

    /// Validate one client read, then re-read for enqueue. The second
    /// image is **not** re-checked — that is the Host1x hole.
    fn ingest_in_place<S: ClientCmdStream>(
        &mut self,
        stream: &mut S,
        iommu: &IommuMap,
        expected_sid: Option<StreamId>,
    ) -> Result<CpCmd, HalError> {
        let mut check = [0u8; CP_CMD_SIZE];
        Self::read_full(stream, &mut check)?;
        self.last_sim.copy_steps = 0;
        let checked = CpCmd::from_le_bytes(check)?;
        self.last_sim.validate_steps = validate_cp_cmd(&checked, iommu, expected_sid)?;
        let mut live = [0u8; CP_CMD_SIZE];
        Self::read_full(stream, &mut live)?;
        // Enqueue the live (possibly mutated) image. Do not store it as
        // a trusted arena copy — this path is the insecure comparison.
        CpCmd::from_le_bytes(live)
    }
}

/// Parse + opcode / reloc / SID / addr-cap walk on a kernel copy.
///
/// Returns the software validate-step count (field + reloc checks).
pub fn validate_cp_cmd(
    cmd: &CpCmd,
    iommu: &IommuMap,
    expected_sid: Option<StreamId>,
) -> Result<u32, HalError> {
    let mut steps = 0u32;
    steps += 1;
    if cmd.magic != CP_PKT_MAGIC {
        return Err(HalError::BadArg);
    }
    steps += 1;
    let op = AccelOp::from_u32(cmd.opcode as u32).ok_or(HalError::Fault)?;
    steps += 1;
    let dtype = DType::from_u8(cmd.dtype).ok_or(HalError::Fault)?;
    steps += 1;
    if MemorySpace::from_u8(cmd.space).is_none() {
        return Err(HalError::Fault);
    }
    steps += 1;
    if Phase::from_u8(cmd.phase).is_none() {
        return Err(HalError::Fault);
    }
    steps += 1;
    if cmd.flags & !CP_KNOWN_FLAGS != 0 {
        return Err(HalError::Fault);
    }

    let sid = StreamId::from_raw(cmd.stream_id);
    steps += 1;
    if sid.ssid() != CP_SSID {
        return Err(HalError::Fault);
    }
    if let Some(exp) = expected_sid {
        if sid != exp {
            return Err(HalError::Fault);
        }
    }

    if op == AccelOp::Nop {
        if cmd.iova_a != 0 || cmd.iova_b != 0 || cmd.iova_c != 0 || cmd.iova_bias != 0 {
            return Err(HalError::Fault);
        }
        return Ok(steps);
    }

    steps += 1;
    if cmd.flags & CP_FLAG_SET_SID == 0 {
        return Err(HalError::Fault);
    }

    let es = dtype.size_bytes() as u64;
    let a_len = es.saturating_mul(cmd.m as u64).saturating_mul(cmd.k as u64);
    let b_len = es.saturating_mul(cmd.k as u64).saturating_mul(cmd.n as u64);
    let c_len = es.saturating_mul(cmd.m as u64).saturating_mul(cmd.n as u64);
    let bias_len = if cmd.flags & CP_FLAG_HAS_BIAS != 0 {
        es.saturating_mul(cmd.n as u64)
    } else {
        0
    };

    validate_reloc(iommu, sid, cmd.iova_a, a_len.max(es))?;
    steps += 1;
    validate_reloc(iommu, sid, cmd.iova_b, b_len.max(es))?;
    steps += 1;
    validate_reloc(iommu, sid, cmd.iova_c, c_len.max(es))?;
    steps += 1;
    if cmd.flags & CP_FLAG_HAS_BIAS != 0 {
        validate_reloc(iommu, sid, cmd.iova_bias, bias_len.max(es))?;
        steps += 1;
    } else if cmd.iova_bias != 0 {
        return Err(HalError::Fault);
    }
    Ok(steps)
}

/// One command-stream reloc: DMA address must translate on `sid` and
/// cover `len` bytes. Non-SVA packets must use the Soft-SMMU IOVA
/// window (refuse identity guest PAs sneaking into the packet). A
/// SVA-bound SSID may use the bound process VA (Linux SVA-shaped;
/// not CUDA UVA).
fn validate_reloc(iommu: &IommuMap, sid: StreamId, iova: u64, len: u64) -> Result<(), HalError> {
    let sva = iommu.mm_of(sid).is_some();
    if !sva && iova < SOFT_SMMU_IOVA_BASE {
        return Err(HalError::Fault);
    }
    if !iommu.covers_iova(sid.raw(), PhysAddr(iova), len) {
        return Err(HalError::Fault);
    }
    iommu
        .walk(sid, PhysAddr(iova))
        .map(|_| ())
        .map_err(|_| HalError::Fault)
}

/// Host-identical clip. Kernel prints `[firewall] …`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirewallReport {
    pub sneak_without: bool,
    pub hold_with: bool,
    pub golden: bool,
    pub sim_noted: bool,
}

impl FirewallReport {
    pub fn all_ok(&self) -> bool {
        self.sneak_without && self.hold_with && self.golden && self.sim_noted
    }
}

/// Race + golden + sim-note clip. Not a Tegra driver and not GPU-CC.
pub fn run_firewall_demo() -> FirewallReport {
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::iommu::MapRequest;
    use aether_core::types::TenantId;

    let mut iommu = IommuMap::new();
    let cap =
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1)).with_generation(1);
    let sid = StreamId::accel(ChipletId(0), aether_core::types::TileId(2), CP_SSID);
    let pin = iommu
        .map(&cap, MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, sid))
        .expect("pin");
    let _ = iommu.set_sid(&cap, sid);

    let good = demo_matmul_cmd(sid, pin.iova);
    let good_bytes = good.to_le_bytes();
    let mut poison = good_bytes;
    poison[CP_OFF_OPCODE] = 0x7F;

    // Without firewall: validate one read, re-read for enqueue → sneak.
    let mut hole = SoftCmdFirewall::new();
    hole.mode = FirewallMode::ValidateInPlace;
    let mut race = RacingCmdStream::new(good_bytes, poison, 1);
    let sneaked = hole.ingest(&mut race, &iommu, Some(sid)).unwrap();
    let sneak_without = sneaked.opcode == 0x7F && race.mutated();

    // With firewall: one snapshot, mutation ignored.
    let mut wall = SoftCmdFirewall::new();
    let mut race2 = RacingCmdStream::new(good_bytes, poison, 1);
    let held = wall.ingest(&mut race2, &iommu, Some(sid)).unwrap();
    let hold_with = held.opcode == AccelOp::MatMul as u32 as u8
        && held.iova_a == pin.iova.0
        && race2.mutated()
        && wall.last_sim.noted();

    // Golden packed admit + identity-PA reloc refuse.
    let golden_cmd = wall.admit_packed(good, &iommu, Some(sid)).is_ok();
    let mut ident = good;
    ident.iova_a = 0x1000;
    let ident_refused = wall.admit_packed(ident, &iommu, Some(sid)) == Err(HalError::Fault);

    FirewallReport {
        sneak_without,
        hold_with,
        golden: golden_cmd && ident_refused,
        sim_noted: wall.last_sim.noted(),
    }
}

fn demo_matmul_cmd(sid: StreamId, iova: PhysAddr) -> CpCmd {
    CpCmd {
        magic: CP_PKT_MAGIC,
        opcode: AccelOp::MatMul as u32 as u8,
        dtype: DType::I32 as u8,
        space: MemorySpace::Host as u8,
        phase: Phase::Compute as u8,
        m: 2,
        n: 2,
        k: 2,
        flags: CP_FLAG_SET_SID,
        stream_id: sid.raw(),
        chiplet: 0,
        tile: 2,
        iova_a: iova.0,
        iova_b: iova.0 + 16,
        iova_c: iova.0 + 32,
        iova_bias: 0,
        fence_id: 0,
    }
}

/// Rebuild a job template from a validated packet (cmdbuf path).
pub fn job_template_from_cmd(cmd: &CpCmd) -> Result<AccelJobDesc, HalError> {
    let op = AccelOp::from_u32(cmd.opcode as u32).ok_or(HalError::Fault)?;
    let dtype = DType::from_u8(cmd.dtype).ok_or(HalError::Fault)?;
    let space = MemorySpace::from_u8(cmd.space).ok_or(HalError::Fault)?;
    let phase = Phase::from_u8(cmd.phase).ok_or(HalError::Fault)?;
    if cmd.chiplet > u8::MAX as u16 {
        return Err(HalError::Fault);
    }
    let mut place = Place::new(ChipletId(cmd.chiplet as u8), space);
    if cmd.tile != 0 {
        place = place.with_tile(cmd.tile);
    }
    Ok(AccelJobDesc {
        op,
        flags: cmd.flags as u32,
        m: cmd.m as u32,
        n: cmd.n as u32,
        k: cmd.k as u32,
        a: PhysAddr(0),
        b: PhysAddr(0),
        c: PhysAddr(0),
        bias: PhysAddr(0),
        a_stride: cmd.k as u32,
        b_stride: cmd.n as u32,
        c_stride: cmd.n as u32,
        dtype,
        tenant: 0,
        completion_ep: 0,
        space,
        place,
        phase,
        partition: aether_core::partition::PartitionId(0),
        fence_id: cmd.fence_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::iommu::MapRequest;
    use aether_core::types::TenantId;

    fn setup() -> (IommuMap, StreamId, PhysAddr) {
        let mut iommu = IommuMap::new();
        let cap = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1))
            .with_generation(1);
        let sid = StreamId::accel(ChipletId(0), aether_core::types::TileId(2), CP_SSID);
        let pin = iommu
            .map(&cap, MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, sid))
            .unwrap();
        iommu.set_sid(&cap, sid).unwrap();
        (iommu, sid, pin.iova)
    }

    #[test]
    fn demo_seals_host1x_race() {
        let r = run_firewall_demo();
        assert!(
            r.sneak_without,
            "in-place validate must let the rewrite sneak"
        );
        assert!(r.hold_with, "copy-then-validate must ignore the rewrite");
        assert!(r.golden);
        assert!(r.sim_noted);
        assert!(r.all_ok());
    }

    #[test]
    fn copy_then_validate_ignores_opcode_rewrite() {
        let (iommu, sid, iova) = setup();
        let good = demo_matmul_cmd(sid, iova).to_le_bytes();
        let mut poison = good;
        poison[CP_OFF_OPCODE] = 0x7F;
        let mut fw = SoftCmdFirewall::new();
        let mut race = RacingCmdStream::new(good, poison, 1);
        let cmd = fw.ingest(&mut race, &iommu, Some(sid)).unwrap();
        assert_eq!(cmd.opcode, AccelOp::MatMul as u32 as u8);
        assert!(race.mutated());
        assert!(fw.last_sim.noted());
    }

    #[test]
    fn in_place_opcode_rewrite_sneaks() {
        let (iommu, sid, iova) = setup();
        let good = demo_matmul_cmd(sid, iova).to_le_bytes();
        let mut poison = good;
        poison[CP_OFF_OPCODE] = 0x7F;
        let mut fw = SoftCmdFirewall::new();
        fw.mode = FirewallMode::ValidateInPlace;
        let mut race = RacingCmdStream::new(good, poison, 1);
        let cmd = fw.ingest(&mut race, &iommu, Some(sid)).unwrap();
        assert_eq!(cmd.opcode, 0x7F, "without firewall the bad opcode sneaks");
        assert_eq!(fw.last_sim.copy_steps, 0);
    }

    #[test]
    fn in_place_iova_rewrite_sneaks_identity() {
        let (iommu, sid, iova) = setup();
        let good = demo_matmul_cmd(sid, iova).to_le_bytes();
        let mut poison = good;
        poison[CP_OFF_IOVA_A..CP_OFF_IOVA_A + 8].copy_from_slice(&0x1000u64.to_le_bytes());
        let mut fw = SoftCmdFirewall::new();
        fw.mode = FirewallMode::ValidateInPlace;
        let mut race = RacingCmdStream::new(good, poison, 1);
        let cmd = fw.ingest(&mut race, &iommu, Some(sid)).unwrap();
        assert_eq!(cmd.iova_a, 0x1000, "identity PA sneaks without copy");
    }

    #[test]
    fn copy_then_validate_ignores_iova_rewrite() {
        let (iommu, sid, iova) = setup();
        let good_cmd = demo_matmul_cmd(sid, iova);
        let good = good_cmd.to_le_bytes();
        let mut poison = good;
        poison[CP_OFF_IOVA_A..CP_OFF_IOVA_A + 8].copy_from_slice(&0x1000u64.to_le_bytes());
        let mut fw = SoftCmdFirewall::new();
        let mut race = RacingCmdStream::new(good, poison, 1);
        let cmd = fw.ingest(&mut race, &iommu, Some(sid)).unwrap();
        assert_eq!(cmd.iova_a, good_cmd.iova_a);
        assert!(cmd.iova_a >= SOFT_SMMU_IOVA_BASE);
    }

    #[test]
    fn refuses_bad_opcode_sid_and_identity_reloc() {
        let (iommu, sid, iova) = setup();
        let mut fw = SoftCmdFirewall::new();
        let mut bad = demo_matmul_cmd(sid, iova);
        bad.opcode = 0x3;
        assert_eq!(
            fw.admit_packed(bad, &iommu, Some(sid)).unwrap_err(),
            HalError::Fault
        );
        let mut wrong = demo_matmul_cmd(sid, iova);
        wrong.stream_id = StreamId::accel(ChipletId(0), aether_core::types::TileId(2), 0).raw();
        assert_eq!(
            fw.admit_packed(wrong, &iommu, Some(sid)).unwrap_err(),
            HalError::Fault
        );
        let mut ident = demo_matmul_cmd(sid, iova);
        ident.iova_a = 0x1000;
        assert_eq!(
            fw.admit_packed(ident, &iommu, Some(sid)).unwrap_err(),
            HalError::Fault
        );
    }

    #[test]
    fn packed_roundtrip_from_le_bytes() {
        let (iommu, sid, iova) = setup();
        let cmd = demo_matmul_cmd(sid, iova);
        let again = CpCmd::from_le_bytes(cmd.to_le_bytes()).unwrap();
        assert_eq!(cmd, again);
        assert!(validate_cp_cmd(&again, &iommu, Some(sid)).unwrap() > 0);
    }
}
