//! Soft SMMU: a software stream-ID page-table for accelerator DMA.
//!
//! Shape matches a future hardware SMMU / IOMMU as far as QEMU allows:
//! a Memory cap authorizes a guest PA range, each stream is its own IOVA
//! namespace, and `walk` / `resolve` follow
//! **STE → CD (SSID) → Stage-1 → Stage-2**. This is **not** a hardware
//! SMMU and **not** a full SMMUv3 emulator (no MMIO register file, no
//! command/event queue, no silicon SID).
//!
//! StreamIDs are **accelerator / chiplet** identities
//! (`chiplet | tile | ssid`), not PCIe BDF. That is the intentional
//! hole versus CPU IOMMU emulators (QEMU / OpenVMM), which index the
//! stream table by guest RequesterID.
//!
//! Walk (software tables, not 4K hardware PTEs):
//! ```text
//! StreamID → STE (must be Bound, config ≠ Abort)
//!          → SSID ≤ S1CDMax, CD.valid
//!          → Stage-1: IOVA → IPA
//!          → Stage-2 (Nested / S2): IPA → PA
//! ```
//! Default `map` installs Nested with **identity Stage-2** (`IPA == PA`)
//! so SoftNPU still sees `resolve(iova) == guest_pa`. `bind_nested`
//! allocates a distinct IPA window (`SOFT_SMMU_IPA_BASE`) so a host
//! test can watch both stages. `unbind_stage2` drops S2 only — S1
//! remains and the next nested walk is `Stage2Fault`.
//!
//! Lifecycle (OpenVMM / smmuv3-accel shaped, software only):
//! - A SID starts **Unbound**. DMA translate **aborts** until it is Bound.
//! - **Capture** on first sighting records the STE; it does not authorize
//!   translation.
//! - **Bind** (Memory+MAP, or the first authorized `map`) installs a
//!   context descriptor (SSID → CD) and enables the stream.
//! - `invalidate` is ATS / TLBI / CFG-cache shaped (software ATC).
//! - `unbind_cd` drops one SSID. `unbind_stream` / `flr` drop the STE
//!   (all SSIDs) and its pins — the FLR analogue.
//!
//! Policy:
//! - Overlap is **per full SID** (STE + SSID) on guest PA. Two SIDs may
//!   pin the same guest PA to different IOVAs.
//! - IOVAs are allocated from per-(STE, CD) windows above 4 GiB, so a
//!   typical QEMU guest PA is never identity-mapped (`iova != guest_pa`).
//! - Entries are block descriptors (one pin = one range), not 4K PTEs.
//! - SSID ≥ `MAX_CDS` / above `S1CDMax`, or a missing / invalid CD, is
//!   `StreamAbort` (SSID/CD walk hardening).
//! - `CrossTenant` / `WrongStream` / `StreamAbort` / `NotMapped` /
//!   `Stage2Fault` as below.
//!
//! Bank QoS / bandwidth coloring is out of scope here.

use crate::caps::{CapKind, CapRights, Capability};
use crate::types::{ChipletId, PhysAddr, TenantId, TileId, PAGE_4K};

pub const MAX_MAPS: usize = 16;
/// Software stream-table size (STE slots). Not a hardware stream-table.
pub const MAX_STES: usize = 8;
/// Context descriptors per STE (SSID → CD). Not a hardware CD table.
pub const MAX_CDS: usize = 8;
/// Software Stage-1 / Stage-2 block slots per CD / STE.
pub const MAX_PTES: usize = 16;
/// Software ATS / ATC lines. Not a device ATC.
pub const MAX_ATC: usize = 16;

/// SoftNPU / default virtqueue stream: chiplet 0, tile 0, ssid 0.
pub const DEFAULT_STREAM: u32 = 0;

/// Soft-SMMU IOVA window base (4 GiB). Guest RAM in the QEMU 128 MiB
/// demo lives well below this, so allocated IOVAs are non-identity.
pub const SOFT_SMMU_IOVA_BASE: u64 = 0x0001_0000_0000;

/// Distinct Stage-2 IPA window (8 GiB). Used only after [`IommuMap::bind_nested`].
pub const SOFT_SMMU_IPA_BASE: u64 = 0x0002_0000_0000;

/// 256 MiB of IOVA / IPA per STE slot.
pub const SOFT_SMMU_STE_SHIFT: u32 = 28;
/// 4 MiB of IOVA / IPA per CD slot inside an STE window.
pub const SOFT_SMMU_CD_SHIFT: u32 = 22;

/// Software S1CDMax (SSID must be `<=` this). Matches [`MAX_CDS`] − 1.
pub const SOFT_SMMU_S1CDMAX: u8 = (MAX_CDS - 1) as u8;

/// Accelerator / chiplet StreamID. **Not** a PCIe BDF.
///
/// Packing: `[31:24] chiplet | [23:8] tile | [7:0] ssid`.
/// `ssid` indexes a software context descriptor on that STE.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StreamId(pub u32);

impl StreamId {
    /// Compose a chiplet/tile/substream SID. This is the Aether-shaped
    /// identity; do not treat it as `bus:dev.fn`.
    pub const fn accel(chiplet: ChipletId, tile: TileId, ssid: u8) -> Self {
        Self(((chiplet.0 as u32) << 24) | ((tile.0 as u32) << 8) | (ssid as u32))
    }

    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    pub const fn chiplet(self) -> ChipletId {
        ChipletId((self.0 >> 24) as u8)
    }

    pub const fn tile(self) -> TileId {
        TileId(((self.0 >> 8) & 0xffff) as u16)
    }

    /// Substream / context-descriptor index (software SSID).
    pub const fn ssid(self) -> u8 {
        (self.0 & 0xff) as u8
    }

    /// STE key: chiplet + tile, SSID cleared.
    pub const fn stream_key(self) -> u32 {
        self.0 & !0xff
    }

    pub const fn with_ssid(self, ssid: u8) -> Self {
        Self(self.stream_key() | (ssid as u32))
    }
}

impl From<u32> for StreamId {
    fn from(raw: u32) -> Self {
        Self(raw)
    }
}

/// STE lifecycle. DMA aborts until [`StreamState::Bound`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Unbound,
    Captured,
    Bound,
}

/// Software STE.Config (SMMUv3-shaped). Not a hardware STE word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteConfig {
    /// STE invalid / disabled — walk aborts.
    Abort = 0,
    /// Stage-1 only: IOVA → PA.
    Stage1 = 1,
    /// Stage-2 only: IPA → PA (device programs IPA as the address).
    Stage2 = 2,
    /// Nested: IOVA → IPA → PA.
    Nested = 3,
}

/// Software ATS / TLBI / CFG invalidate. Not a hardware command queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvCmd {
    /// CMD_ATC_INV-shaped: drop ATC lines for `sid` (optionally an IOVA range).
    Ats {
        sid: StreamId,
        iova: Option<PhysAddr>,
        len: u64,
    },
    /// CMD_TLBI-shaped: drop ATC for one SID, or all if `sid` is `None`.
    Tlbi { sid: Option<StreamId> },
    /// CMD_CFGI_STE-shaped: drop ATC for every SSID on this STE.
    CfgSte { sid: StreamId },
    /// CMD_CFGI_CD-shaped: drop ATC for this SSID.
    CfgCd { sid: StreamId },
    /// Drop the whole software ATC.
    All,
}

/// Result of an STE → CD → Stage-1 → Stage-2 walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkResult {
    /// Address that was walked (IOVA, or IPA when [`SteConfig::Stage2`]).
    pub iova: PhysAddr,
    /// Stage-1 output (IPA). Equals `pa` on Stage-1-only / identity S2.
    pub ipa: PhysAddr,
    /// Final guest PA.
    pub pa: PhysAddr,
    pub config: SteConfig,
    /// Covering leaf length (bytes from the Stage-1 block).
    pub len: u64,
}

/// One software block PTE (Stage-1 or Stage-2). Not a hardware descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftPte {
    pub va: u64,
    pub out: u64,
    pub len: u64,
    pub writable: bool,
}

/// Pin request. `stream_id` is a packed [`StreamId`] (chiplet/tile/ssid).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapRequest {
    pub guest_pa: PhysAddr,
    pub len: u64,
    pub stream_id: u32,
    pub writable: bool,
}

impl MapRequest {
    pub const fn pin(guest_pa: PhysAddr, len: u64) -> Self {
        Self::pin_stream(guest_pa, len, DEFAULT_STREAM)
    }

    pub const fn pin_stream(guest_pa: PhysAddr, len: u64, stream_id: u32) -> Self {
        Self {
            guest_pa,
            len,
            stream_id,
            writable: true,
        }
    }

    pub const fn pin_accel(guest_pa: PhysAddr, len: u64, sid: StreamId) -> Self {
        Self::pin_stream(guest_pa, len, sid.raw())
    }

    pub const fn stream(self) -> StreamId {
        StreamId(self.stream_id)
    }
}

/// One pinned Soft-SMMU block descriptor (host ledger; the walk uses S1/S2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappedRegion {
    pub guest_pa: PhysAddr,
    pub iova: PhysAddr,
    pub len: u64,
    pub tenant: TenantId,
    pub object: u32,
    pub stream_id: u32,
    pub writable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    /// No Memory cap, or the cap lacks [`CapRights::MAP`].
    NoMemoryCap,
    BadRange,
    /// Guest-PA overlap on the **same** SID (same tenant).
    Overlap,
    TableFull,
    NotMapped,
    /// Another tenant owns the window, or the authorizing cap's tenant
    /// does not match the pinned region / bound STE.
    CrossTenant,
    /// The PA / IOVA is mapped, but not on the requested SID.
    WrongStream,
    /// SID is Unbound or Captured, SSID is illegal, or its CD is missing.
    /// DMA must abort.
    StreamAbort,
    /// Nested / Stage-2 walk: Stage-1 hit, Stage-2 miss.
    Stage2Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContextDesc {
    ssid: u8,
    valid: bool,
    asid: u16,
    s1: [Option<SoftPte>; MAX_PTES],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StreamTableEntry {
    key: u32,
    state: StreamState,
    tenant: TenantId,
    config: SteConfig,
    s1cdmax: u8,
    /// When true, `map` allocates IPA in [`SOFT_SMMU_IPA_BASE`].
    distinct_ipa: bool,
    s2_vmid: u16,
    s2: [Option<SoftPte>; MAX_PTES],
    cds: [Option<ContextDesc>; MAX_CDS],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AtsLine {
    sid: u32,
    iova: u64,
    pa: u64,
    len: u64,
}

/// Software IOMMU: STE → CD → Stage-1 → Stage-2.
#[derive(Clone, Debug)]
pub struct IommuMap {
    stes: [Option<StreamTableEntry>; MAX_STES],
    regions: [Option<MappedRegion>; MAX_MAPS],
    atc: [Option<AtsLine>; MAX_ATC],
    atc_hits: u32,
    atc_misses: u32,
}

impl IommuMap {
    pub const fn new() -> Self {
        Self {
            stes: [None; MAX_STES],
            regions: [None; MAX_MAPS],
            atc: [None; MAX_ATC],
            atc_hits: 0,
            atc_misses: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.regions.iter().filter(|r| r.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ste_count(&self) -> usize {
        self.stes.iter().filter(|s| s.is_some()).count()
    }

    pub fn atc_len(&self) -> usize {
        self.atc.iter().filter(|l| l.is_some()).count()
    }

    pub fn atc_hits(&self) -> u32 {
        self.atc_hits
    }

    pub fn atc_misses(&self) -> u32 {
        self.atc_misses
    }

    pub fn ste_config(&self, sid: StreamId) -> SteConfig {
        self.ste(sid).map(|s| s.config).unwrap_or(SteConfig::Abort)
    }

    /// IOVA window for software STE slot `ste_idx` and CD slot `cd_slot`.
    pub const fn window_base(ste_idx: usize, cd_slot: usize) -> u64 {
        SOFT_SMMU_IOVA_BASE
            .wrapping_add((ste_idx as u64) << SOFT_SMMU_STE_SHIFT)
            .wrapping_add((cd_slot as u64) << SOFT_SMMU_CD_SHIFT)
    }

    /// IPA window for software STE slot `ste_idx` and CD slot `cd_slot`.
    pub const fn ipa_window_base(ste_idx: usize, cd_slot: usize) -> u64 {
        SOFT_SMMU_IPA_BASE
            .wrapping_add((ste_idx as u64) << SOFT_SMMU_STE_SHIFT)
            .wrapping_add((cd_slot as u64) << SOFT_SMMU_CD_SHIFT)
    }

    /// Authorize + pin. Refuses anything that is not a Memory cap with MAP.
    /// First authorized use **captures and binds** the SID, then allocates
    /// a non-identity IOVA in that stream namespace and installs S1 + S2.
    pub fn map(&mut self, cap: &Capability, req: MapRequest) -> Result<MappedRegion, MapError> {
        Self::check_cap(cap)?;
        if req.len == 0 {
            return Err(MapError::BadRange);
        }
        if req.guest_pa.0.checked_add(req.len).is_none() {
            return Err(MapError::BadRange);
        }
        self.bind_stream(cap, req.stream())?;
        if let Some(err) =
            self.guest_overlap_error(req.guest_pa, req.len, req.stream_id, cap.tenant)
        {
            return Err(err);
        }
        let slot = self
            .regions
            .iter()
            .position(|r| r.is_none())
            .ok_or(MapError::TableFull)?;
        let iova = self.alloc_iova(req.stream(), req.guest_pa, req.len)?;
        let ipa = self.alloc_ipa(req.stream(), req.guest_pa, req.len)?;
        self.install_s1(req.stream(), iova.0, ipa, req.len, req.writable)?;
        if let Err(e) = self.install_s2(req.stream(), ipa, req.guest_pa.0, req.len, req.writable) {
            self.remove_s1(req.stream(), iova.0);
            return Err(e);
        }
        let region = MappedRegion {
            guest_pa: req.guest_pa,
            iova,
            len: req.len,
            tenant: cap.tenant,
            object: cap.object,
            stream_id: req.stream_id,
            writable: req.writable,
        };
        self.regions[slot] = Some(region);
        self.invalidate_ats_range(req.stream(), iova, req.len);
        Ok(region)
    }

    pub fn check_cap(cap: &Capability) -> Result<(), MapError> {
        if cap.kind != CapKind::Memory {
            return Err(MapError::NoMemoryCap);
        }
        if !cap.rights.contains(CapRights::MAP) {
            return Err(MapError::NoMemoryCap);
        }
        Ok(())
    }

    /// Record first sighting of a SID. Does not authorize DMA.
    pub fn capture(&mut self, sid: StreamId) -> Result<StreamState, MapError> {
        if let Some(i) = self.ste_index(sid) {
            return Ok(self.stes[i].as_ref().unwrap().state);
        }
        let slot = self
            .stes
            .iter()
            .position(|s| s.is_none())
            .ok_or(MapError::TableFull)?;
        self.stes[slot] = Some(StreamTableEntry {
            key: sid.stream_key(),
            state: StreamState::Captured,
            tenant: TenantId(0),
            config: SteConfig::Abort,
            s1cdmax: SOFT_SMMU_S1CDMAX,
            distinct_ipa: false,
            s2_vmid: 0,
            s2: [None; MAX_PTES],
            cds: [None; MAX_CDS],
        });
        Ok(StreamState::Captured)
    }

    /// Bind a SID (STE + this SSID's CD). Requires Memory+MAP.
    /// Capture-only streams stay aborting until this succeeds.
    pub fn bind_stream(
        &mut self,
        cap: &Capability,
        sid: StreamId,
    ) -> Result<StreamState, MapError> {
        Self::check_cap(cap)?;
        Self::require_ssid_range(sid)?;
        let _ = self.capture(sid)?;
        let ste_i = self.ste_index(sid).ok_or(MapError::TableFull)?;
        {
            let ste = self.stes[ste_i].as_mut().unwrap();
            if sid.ssid() > ste.s1cdmax {
                return Err(MapError::StreamAbort);
            }
            if ste.state == StreamState::Bound && ste.tenant != cap.tenant {
                return Err(MapError::CrossTenant);
            }
            ste.tenant = cap.tenant;
            ste.state = StreamState::Bound;
            if ste.config == SteConfig::Abort {
                ste.config = SteConfig::Nested;
            }
        }
        self.ensure_cd(ste_i, sid.ssid())?;
        Ok(StreamState::Bound)
    }

    /// Bind for a nested walk whose Stage-1 IPA is **not** the guest PA.
    /// SoftNPU does not need this; host tests use it to watch S1 then S2.
    pub fn bind_nested(
        &mut self,
        cap: &Capability,
        sid: StreamId,
    ) -> Result<StreamState, MapError> {
        let st = self.bind_stream(cap, sid)?;
        if let Some(i) = self.ste_index(sid) {
            let ste = self.stes[i].as_mut().unwrap();
            ste.config = SteConfig::Nested;
            ste.distinct_ipa = true;
            ste.s2_vmid = sid.tile().0;
        }
        Ok(st)
    }

    /// FLR analogue: drop the STE (all SSIDs) and its pins. Next DMA aborts.
    pub fn unbind_stream(&mut self, sid: StreamId) -> Result<(), MapError> {
        let Some(ste_i) = self.ste_index(sid) else {
            return Err(MapError::NotMapped);
        };
        let key = sid.stream_key();
        for r in self.regions.iter_mut() {
            if r.as_ref()
                .is_some_and(|x| StreamId(x.stream_id).stream_key() == key)
            {
                *r = None;
            }
        }
        self.invalidate_ste_key(key);
        self.stes[ste_i] = None;
        Ok(())
    }

    /// Function-level reset analogue. Same as [`Self::unbind_stream`].
    pub fn flr(&mut self, sid: StreamId) -> Result<(), MapError> {
        self.unbind_stream(sid)
    }

    /// Drop one SSID's CD + S1 + pins. Sibling SSIDs on the STE stay Bound.
    pub fn unbind_cd(&mut self, sid: StreamId) -> Result<(), MapError> {
        let ste_i = self.ste_index(sid).ok_or(MapError::NotMapped)?;
        let cd_i = self.cd_slot_of(sid).ok_or(MapError::NotMapped)?;
        let raw = sid.raw();
        for r in self.regions.iter_mut() {
            if r.as_ref().is_some_and(|x| x.stream_id == raw) {
                *r = None;
            }
        }
        self.stes[ste_i].as_mut().unwrap().cds[cd_i] = None;
        self.gc_s2(ste_i);
        let _ = self.invalidate(InvCmd::CfgCd { sid });
        let empty = self.stes[ste_i]
            .as_ref()
            .unwrap()
            .cds
            .iter()
            .all(|c| c.is_none());
        if empty {
            if let Some(ste) = self.stes[ste_i].as_mut() {
                ste.state = StreamState::Captured;
                ste.config = SteConfig::Abort;
                ste.s2 = [None; MAX_PTES];
                ste.distinct_ipa = false;
            }
        }
        Ok(())
    }

    /// Drop Stage-2 PTEs for this STE. S1 remains; a Nested walk faults.
    pub fn unbind_stage2(&mut self, sid: StreamId) -> Result<(), MapError> {
        let ste_i = self.ste_index(sid).ok_or(MapError::NotMapped)?;
        self.stes[ste_i].as_mut().unwrap().s2 = [None; MAX_PTES];
        self.invalidate_ste_key(sid.stream_key());
        Ok(())
    }

    /// ATS / TLBI / CFG-cache invalidate. Page tables are not dropped.
    pub fn invalidate(&mut self, cmd: InvCmd) -> Result<u32, MapError> {
        let before = self.atc_len();
        match cmd {
            InvCmd::All => {
                self.atc = [None; MAX_ATC];
            }
            InvCmd::Tlbi { sid: None } => {
                self.atc = [None; MAX_ATC];
            }
            InvCmd::Tlbi { sid: Some(sid) } => {
                self.drop_atc(|l| l.sid == sid.raw());
            }
            InvCmd::CfgCd { sid } => {
                self.drop_atc(|l| l.sid == sid.raw());
            }
            InvCmd::CfgSte { sid } => {
                let key = sid.stream_key();
                self.drop_atc(|l| StreamId(l.sid).stream_key() == key);
            }
            InvCmd::Ats { sid, iova, len } => match iova {
                None => self.drop_atc(|l| l.sid == sid.raw()),
                Some(base) => self.invalidate_ats_range(sid, base, len),
            },
        }
        Ok((before.saturating_sub(self.atc_len())) as u32)
    }

    pub fn stream_state(&self, sid: StreamId) -> StreamState {
        if Self::ssid_out_of_range(sid) {
            return StreamState::Unbound;
        }
        self.ste(sid)
            .map(|s| {
                if sid.ssid() > s.s1cdmax {
                    return StreamState::Unbound;
                }
                if s.state != StreamState::Bound || s.config == SteConfig::Abort {
                    return s.state;
                }
                if self.cd_slot_of(sid).is_some() {
                    StreamState::Bound
                } else {
                    StreamState::Captured
                }
            })
            .unwrap_or(StreamState::Unbound)
    }

    pub fn is_bound(&self, sid: StreamId) -> bool {
        self.stream_state(sid) == StreamState::Bound
    }

    /// STE → CD → Stage-1 → Stage-2. Does not touch the ATC.
    pub fn walk(&self, sid: StreamId, addr: PhysAddr) -> Result<WalkResult, MapError> {
        self.require_bound(sid)?;
        let ste = self.ste(sid).ok_or(MapError::StreamAbort)?;
        let cd_i = self.cd_slot_of(sid).ok_or(MapError::StreamAbort)?;
        let cd = ste.cds[cd_i].as_ref().ok_or(MapError::StreamAbort)?;
        if !cd.valid {
            return Err(MapError::StreamAbort);
        }
        let config = ste.config;
        if config == SteConfig::Abort {
            return Err(MapError::StreamAbort);
        }
        let (s1_va, ipa, leaf_len) = if config == SteConfig::Stage2 {
            (addr.0, addr.0, 0)
        } else {
            let pte = Self::pte_hit(&cd.s1, addr.0).ok_or(MapError::NotMapped)?;
            let off = addr.0 - pte.va;
            (pte.va, pte.out + off, pte.len)
        };
        if config == SteConfig::Stage1 {
            return Ok(WalkResult {
                iova: addr,
                ipa: PhysAddr(ipa),
                pa: PhysAddr(ipa),
                config,
                len: leaf_len,
            });
        }
        let s2 = Self::pte_hit(&ste.s2, ipa).ok_or(MapError::Stage2Fault)?;
        let off = ipa - s2.va;
        let pa = s2.out + off;
        let len = if leaf_len == 0 { s2.len } else { leaf_len };
        let _ = s1_va;
        Ok(WalkResult {
            iova: addr,
            ipa: PhysAddr(ipa),
            pa: PhysAddr(pa),
            config,
            len,
        })
    }

    /// Resolve through the software ATC, then walk and fill. Tables stay.
    pub fn resolve_ats(&mut self, stream_id: u32, iova: PhysAddr) -> Result<PhysAddr, MapError> {
        let sid = StreamId(stream_id);
        self.require_bound(sid)?;
        if let Some(pa) = self.atc_lookup(stream_id, iova.0) {
            self.atc_hits = self.atc_hits.saturating_add(1);
            return Ok(pa);
        }
        self.atc_misses = self.atc_misses.saturating_add(1);
        let w = self.walk(sid, iova)?;
        let base_off = iova
            .0
            .saturating_sub(self.s1_base(sid, iova.0).unwrap_or(iova.0));
        let iova_base = iova.0.saturating_sub(base_off);
        let pa_base = w.pa.0.saturating_sub(base_off);
        self.atc_insert(stream_id, iova_base, pa_base, w.len);
        Ok(w.pa)
    }

    fn ste_index(&self, sid: StreamId) -> Option<usize> {
        let key = sid.stream_key();
        self.stes
            .iter()
            .position(|s| s.as_ref().is_some_and(|e| e.key == key))
    }

    fn ste(&self, sid: StreamId) -> Option<&StreamTableEntry> {
        self.ste_index(sid).and_then(|i| self.stes[i].as_ref())
    }

    fn cd_slot_of(&self, sid: StreamId) -> Option<usize> {
        self.ste(sid).and_then(|e| {
            e.cds
                .iter()
                .position(|c| c.as_ref().is_some_and(|d| d.valid && d.ssid == sid.ssid()))
        })
    }

    fn ssid_out_of_range(sid: StreamId) -> bool {
        sid.ssid() as usize >= MAX_CDS
    }

    fn require_ssid_range(sid: StreamId) -> Result<(), MapError> {
        if Self::ssid_out_of_range(sid) {
            Err(MapError::StreamAbort)
        } else {
            Ok(())
        }
    }

    fn ensure_cd(&mut self, ste_i: usize, ssid: u8) -> Result<usize, MapError> {
        let s1cdmax = self.stes[ste_i].as_ref().unwrap().s1cdmax;
        if ssid > s1cdmax {
            return Err(MapError::StreamAbort);
        }
        if let Some(i) = self.stes[ste_i].as_ref().and_then(|e| {
            e.cds
                .iter()
                .position(|c| c.as_ref().is_some_and(|d| d.ssid == ssid))
        }) {
            if let Some(cd) = self.stes[ste_i].as_mut().unwrap().cds[i].as_mut() {
                cd.valid = true;
            }
            return Ok(i);
        }
        let slot = self.stes[ste_i]
            .as_ref()
            .unwrap()
            .cds
            .iter()
            .position(|c| c.is_none())
            .ok_or(MapError::TableFull)?;
        self.stes[ste_i].as_mut().unwrap().cds[slot] = Some(ContextDesc {
            ssid,
            valid: true,
            asid: ssid as u16,
            s1: [None; MAX_PTES],
        });
        Ok(slot)
    }

    fn require_bound(&self, sid: StreamId) -> Result<(), MapError> {
        Self::require_ssid_range(sid)?;
        if self.is_bound(sid) {
            Ok(())
        } else {
            Err(MapError::StreamAbort)
        }
    }

    fn page_align_up(len: u64) -> Option<u64> {
        if len == 0 {
            return None;
        }
        let mask = PAGE_4K - 1;
        len.checked_add(mask).map(|x| x & !mask)
    }

    fn ranges_overlap(a: u64, a_len: u64, b: u64, b_len: u64) -> bool {
        let a_end = a.saturating_add(a_len);
        let b_end = b.saturating_add(b_len);
        a < b_end && b < a_end
    }

    fn pte_hit(table: &[Option<SoftPte>], va: u64) -> Option<SoftPte> {
        table
            .iter()
            .flatten()
            .copied()
            .find(|p| va >= p.va && va < p.va + p.len)
    }

    fn guest_overlap_error(
        &self,
        pa: PhysAddr,
        len: u64,
        stream_id: u32,
        tenant: TenantId,
    ) -> Option<MapError> {
        self.regions.iter().flatten().find_map(|r| {
            if r.stream_id != stream_id {
                return None;
            }
            if !Self::ranges_overlap(pa.0, len, r.guest_pa.0, r.len) {
                return None;
            }
            if r.tenant != tenant {
                Some(MapError::CrossTenant)
            } else {
                Some(MapError::Overlap)
            }
        })
    }

    fn alloc_in_window(
        used: impl Iterator<Item = (u64, u64)>,
        base: u64,
        window: u64,
        guest_pa: PhysAddr,
        len: u64,
    ) -> Result<u64, MapError> {
        let span = Self::page_align_up(len).ok_or(MapError::BadRange)?;
        let mut next = base;
        for (va, rlen) in used {
            let rspan = Self::page_align_up(rlen).ok_or(MapError::BadRange)?;
            let end = va.checked_add(rspan).ok_or(MapError::BadRange)?;
            if end > next {
                next = end;
            }
        }
        if next == guest_pa.0 {
            next = next.checked_add(span).ok_or(MapError::BadRange)?;
        }
        let end = next.checked_add(span).ok_or(MapError::BadRange)?;
        if end.saturating_sub(base) > window {
            return Err(MapError::TableFull);
        }
        Ok(next)
    }

    fn alloc_iova(
        &self,
        sid: StreamId,
        guest_pa: PhysAddr,
        len: u64,
    ) -> Result<PhysAddr, MapError> {
        let ste_i = self.ste_index(sid).ok_or(MapError::StreamAbort)?;
        let cd_i = self.cd_slot_of(sid).ok_or(MapError::StreamAbort)?;
        let base = Self::window_base(ste_i, cd_i);
        let window = 1u64 << SOFT_SMMU_CD_SHIFT;
        let used = self.regions.iter().flatten().filter_map(|r| {
            if r.stream_id == sid.raw() {
                Some((r.iova.0, r.len))
            } else {
                None
            }
        });
        Ok(PhysAddr(Self::alloc_in_window(
            used, base, window, guest_pa, len,
        )?))
    }

    fn alloc_ipa(&self, sid: StreamId, guest_pa: PhysAddr, len: u64) -> Result<u64, MapError> {
        let ste = self.ste(sid).ok_or(MapError::StreamAbort)?;
        if !ste.distinct_ipa {
            return Ok(guest_pa.0);
        }
        let ste_i = self.ste_index(sid).ok_or(MapError::StreamAbort)?;
        let cd_i = self.cd_slot_of(sid).ok_or(MapError::StreamAbort)?;
        let base = Self::ipa_window_base(ste_i, cd_i);
        let window = 1u64 << SOFT_SMMU_CD_SHIFT;
        let cd = ste.cds[cd_i].as_ref().ok_or(MapError::StreamAbort)?;
        let used = cd.s1.iter().flatten().map(|p| (p.out, p.len));
        Self::alloc_in_window(used, base, window, guest_pa, len)
    }

    fn install_s1(
        &mut self,
        sid: StreamId,
        iova: u64,
        ipa: u64,
        len: u64,
        writable: bool,
    ) -> Result<(), MapError> {
        let ste_i = self.ste_index(sid).ok_or(MapError::StreamAbort)?;
        let cd_i = self.cd_slot_of(sid).ok_or(MapError::StreamAbort)?;
        let s1 = &mut self.stes[ste_i].as_mut().unwrap().cds[cd_i]
            .as_mut()
            .unwrap()
            .s1;
        if s1
            .iter()
            .flatten()
            .any(|p| Self::ranges_overlap(iova, len, p.va, p.len))
        {
            return Err(MapError::Overlap);
        }
        let slot = s1
            .iter()
            .position(|p| p.is_none())
            .ok_or(MapError::TableFull)?;
        s1[slot] = Some(SoftPte {
            va: iova,
            out: ipa,
            len,
            writable,
        });
        Ok(())
    }

    fn install_s2(
        &mut self,
        sid: StreamId,
        ipa: u64,
        pa: u64,
        len: u64,
        writable: bool,
    ) -> Result<(), MapError> {
        let ste_i = self.ste_index(sid).ok_or(MapError::StreamAbort)?;
        let s2 = &mut self.stes[ste_i].as_mut().unwrap().s2;
        if let Some(p) = s2
            .iter()
            .flatten()
            .find(|p| Self::ranges_overlap(ipa, len, p.va, p.len))
        {
            let off = ipa.saturating_sub(p.va);
            if p.out.saturating_add(off) == pa {
                return Ok(());
            }
            return Err(MapError::Overlap);
        }
        let slot = s2
            .iter()
            .position(|p| p.is_none())
            .ok_or(MapError::TableFull)?;
        s2[slot] = Some(SoftPte {
            va: ipa,
            out: pa,
            len,
            writable,
        });
        Ok(())
    }

    fn remove_s1(&mut self, sid: StreamId, iova: u64) -> Option<SoftPte> {
        let ste_i = self.ste_index(sid)?;
        let cd_i = self.cd_slot_of(sid)?;
        let s1 = &mut self.stes[ste_i].as_mut().unwrap().cds[cd_i]
            .as_mut()
            .unwrap()
            .s1;
        let pos = s1.iter().position(|p| {
            p.as_ref()
                .is_some_and(|x| iova >= x.va && iova < x.va + x.len)
        })?;
        s1[pos].take()
    }

    fn s1_still_uses_ipa(&self, ste_i: usize, ipa: u64, len: u64) -> bool {
        let Some(ste) = self.stes[ste_i].as_ref() else {
            return false;
        };
        ste.cds.iter().flatten().any(|cd| {
            cd.s1
                .iter()
                .flatten()
                .any(|p| Self::ranges_overlap(ipa, len, p.out, p.len))
        })
    }

    fn remove_s2_if_unused(&mut self, ste_i: usize, ipa: u64, len: u64) {
        if self.s1_still_uses_ipa(ste_i, ipa, len) {
            return;
        }
        if let Some(ste) = self.stes[ste_i].as_mut() {
            for p in ste.s2.iter_mut() {
                if p.as_ref()
                    .is_some_and(|x| Self::ranges_overlap(ipa, len, x.va, x.len))
                {
                    *p = None;
                }
            }
        }
    }

    fn gc_s2(&mut self, ste_i: usize) {
        let Some(ste) = self.stes[ste_i].as_ref() else {
            return;
        };
        let keep: [Option<SoftPte>; MAX_PTES] = ste.s2;
        for (i, p) in keep.iter().enumerate() {
            if let Some(pte) = p {
                if !self.s1_still_uses_ipa(ste_i, pte.va, pte.len) {
                    if let Some(ste) = self.stes[ste_i].as_mut() {
                        ste.s2[i] = None;
                    }
                }
            }
        }
    }

    fn s1_base(&self, sid: StreamId, iova: u64) -> Option<u64> {
        let ste = self.ste(sid)?;
        let cd_i = self.cd_slot_of(sid)?;
        let cd = ste.cds[cd_i].as_ref()?;
        Self::pte_hit(&cd.s1, iova).map(|p| p.va)
    }

    fn s1_hit_any(&self, iova: u64) -> bool {
        self.stes.iter().flatten().any(|ste| {
            ste.cds
                .iter()
                .flatten()
                .any(|cd| Self::pte_hit(&cd.s1, iova).is_some())
        })
    }

    fn drop_atc(&mut self, pred: impl Fn(&AtsLine) -> bool) {
        for l in self.atc.iter_mut() {
            if l.as_ref().is_some_and(&pred) {
                *l = None;
            }
        }
    }

    fn invalidate_ste_key(&mut self, key: u32) {
        self.drop_atc(|l| StreamId(l.sid).stream_key() == key);
    }

    fn invalidate_ats_range(&mut self, sid: StreamId, iova: PhysAddr, len: u64) {
        let end = iova.0.saturating_add(len.max(1));
        self.drop_atc(|l| {
            l.sid == sid.raw()
                && Self::ranges_overlap(iova.0, end.saturating_sub(iova.0), l.iova, l.len)
        });
    }

    fn atc_lookup(&self, sid: u32, iova: u64) -> Option<PhysAddr> {
        self.atc.iter().flatten().find_map(|l| {
            if l.sid == sid && iova >= l.iova && iova < l.iova + l.len {
                Some(PhysAddr(l.pa + (iova - l.iova)))
            } else {
                None
            }
        })
    }

    fn atc_insert(&mut self, sid: u32, iova: u64, pa: u64, len: u64) {
        if len == 0 {
            return;
        }
        if self.atc_lookup(sid, iova).is_some() {
            return;
        }
        let slot = match self.atc.iter().position(|l| l.is_none()) {
            Some(s) => s,
            None => 0,
        };
        self.atc[slot] = Some(AtsLine { sid, iova, pa, len });
    }

    fn offset_in(region: &MappedRegion, addr: PhysAddr, use_iova: bool) -> Option<u64> {
        let start = if use_iova {
            region.iova.0
        } else {
            region.guest_pa.0
        };
        if addr.0 >= start && addr.0 < start + region.len {
            Some(addr.0 - start)
        } else {
            None
        }
    }

    /// Translate guest PA → IOVA on [`DEFAULT_STREAM`].
    pub fn translate(&self, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.translate_stream(DEFAULT_STREAM, guest_pa)
    }

    /// Translate guest PA → IOVA on `stream_id`. `None` if unbound, unmapped,
    /// or pinned only on another SID.
    pub fn translate_stream(&self, stream_id: u32, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.translate_result(stream_id, guest_pa, None).ok()
    }

    /// Translate with explicit errors (abort / unmapped / wrong stream / tenant).
    /// Reverse lookup of the pin ledger — not a hardware SMMU operation.
    pub fn translate_result(
        &self,
        stream_id: u32,
        guest_pa: PhysAddr,
        tenant: Option<TenantId>,
    ) -> Result<PhysAddr, MapError> {
        let sid = StreamId(stream_id);
        self.require_bound(sid)?;
        if let Some(r) = self.region_at_stream(stream_id, guest_pa) {
            if let Some(t) = tenant {
                if r.tenant != t {
                    return Err(MapError::CrossTenant);
                }
            }
            let off = guest_pa.0 - r.guest_pa.0;
            return Ok(PhysAddr(r.iova.0 + off));
        }
        if self.region_at_any_guest(guest_pa).is_some() {
            return Err(MapError::WrongStream);
        }
        Err(MapError::NotMapped)
    }

    /// IOVA → guest PA on any bound stream (windows are disjoint).
    pub fn resolve(&self, iova: PhysAddr) -> Option<PhysAddr> {
        self.region_at_iova(iova)
            .and_then(|r| self.walk(StreamId(r.stream_id), iova).ok().map(|w| w.pa))
    }

    /// IOVA → guest PA on `stream_id`.
    pub fn resolve_stream(&self, stream_id: u32, iova: PhysAddr) -> Option<PhysAddr> {
        self.resolve_result(stream_id, iova, None).ok()
    }

    /// IOVA → guest PA via the STE → CD → S1 → S2 walk.
    pub fn resolve_result(
        &self,
        stream_id: u32,
        iova: PhysAddr,
        tenant: Option<TenantId>,
    ) -> Result<PhysAddr, MapError> {
        let sid = StreamId(stream_id);
        self.require_bound(sid)?;
        if let Some(t) = tenant {
            if self.ste(sid).is_some_and(|s| s.tenant != t) {
                return Err(MapError::CrossTenant);
            }
        }
        match self.walk(sid, iova) {
            Ok(w) => Ok(w.pa),
            Err(MapError::NotMapped) => {
                if self.s1_hit_any(iova.0) {
                    Err(MapError::WrongStream)
                } else {
                    Err(MapError::NotMapped)
                }
            }
            Err(e) => Err(e),
        }
    }

    pub fn covers(&self, pa: PhysAddr, len: u64) -> bool {
        self.covers_stream(DEFAULT_STREAM, pa, len)
    }

    pub fn covers_stream(&self, stream_id: u32, pa: PhysAddr, len: u64) -> bool {
        if !self.is_bound(StreamId(stream_id)) {
            return false;
        }
        self.covers_in(stream_id, pa, len, false)
    }

    /// True if every byte of `[iova, iova+len)` is pinned on `stream_id`.
    pub fn covers_iova(&self, stream_id: u32, iova: PhysAddr, len: u64) -> bool {
        if !self.is_bound(StreamId(stream_id)) {
            return false;
        }
        self.covers_in(stream_id, iova, len, true)
    }

    /// Guest-PA or IOVA coverage on any **bound** stream (device DMA after rewrite).
    pub fn covers_iova_any(&self, iova: PhysAddr, len: u64) -> bool {
        if len == 0 {
            return false;
        }
        let end = match iova.0.checked_add(len) {
            Some(e) => e,
            None => return false,
        };
        let mut cursor = iova.0;
        while cursor < end {
            let Some(r) = self.region_at_iova(PhysAddr(cursor)) else {
                return false;
            };
            if self.walk(StreamId(r.stream_id), PhysAddr(cursor)).is_err() {
                return false;
            }
            let re = r.iova.0 + r.len;
            if re <= cursor {
                return false;
            }
            cursor = re;
        }
        true
    }

    fn covers_in(&self, stream_id: u32, addr: PhysAddr, len: u64, use_iova: bool) -> bool {
        if len == 0 {
            return false;
        }
        let end = match addr.0.checked_add(len) {
            Some(e) => e,
            None => return false,
        };
        let mut cursor = addr.0;
        while cursor < end {
            let hit = self.regions.iter().flatten().find(|r| {
                if r.stream_id != stream_id {
                    return false;
                }
                Self::offset_in(r, PhysAddr(cursor), use_iova).is_some()
            });
            let Some(r) = hit else {
                return false;
            };
            if use_iova && self.walk(StreamId(stream_id), PhysAddr(cursor)).is_err() {
                return false;
            }
            let re = if use_iova {
                r.iova.0 + r.len
            } else {
                r.guest_pa.0 + r.len
            };
            if re <= cursor {
                return false;
            }
            cursor = re;
        }
        true
    }

    pub fn region_at(&self, pa: PhysAddr) -> Option<&MappedRegion> {
        self.region_at_stream(DEFAULT_STREAM, pa)
    }

    pub fn region_at_stream(&self, stream_id: u32, pa: PhysAddr) -> Option<&MappedRegion> {
        self.regions
            .iter()
            .flatten()
            .find(|r| r.stream_id == stream_id && Self::offset_in(r, pa, false).is_some())
    }

    pub fn region_at_any_guest(&self, pa: PhysAddr) -> Option<&MappedRegion> {
        self.regions
            .iter()
            .flatten()
            .find(|r| Self::offset_in(r, pa, false).is_some())
    }

    pub fn region_at_iova(&self, iova: PhysAddr) -> Option<&MappedRegion> {
        self.regions
            .iter()
            .flatten()
            .find(|r| Self::offset_in(r, iova, true).is_some())
    }

    /// Unmap by IOVA start (or any byte in the window). Stream is implied
    /// by the disjoint IOVA windows.
    pub fn unmap(&mut self, iova: PhysAddr) -> Result<MappedRegion, MapError> {
        let pos = self
            .regions
            .iter()
            .position(|r| {
                r.as_ref()
                    .is_some_and(|x| Self::offset_in(x, iova, true).is_some())
            })
            .ok_or(MapError::NotMapped)?;
        let region = self.regions[pos].take().unwrap();
        self.drop_pin_tables(&region);
        Ok(region)
    }

    /// Unmap an IOVA only if it belongs to `stream_id`.
    pub fn unmap_stream(
        &mut self,
        stream_id: u32,
        iova: PhysAddr,
    ) -> Result<MappedRegion, MapError> {
        let pos = match self.regions.iter().position(|r| {
            r.as_ref()
                .is_some_and(|x| Self::offset_in(x, iova, true).is_some())
        }) {
            Some(p) => p,
            None => return Err(MapError::NotMapped),
        };
        let sid = self.regions[pos].as_ref().unwrap().stream_id;
        if sid != stream_id {
            return Err(MapError::WrongStream);
        }
        let region = self.regions[pos].take().unwrap();
        self.drop_pin_tables(&region);
        Ok(region)
    }

    /// Unmap authorized by a Memory+MAP cap. Wrong tenant is `CrossTenant`.
    pub fn unmap_for(
        &mut self,
        cap: &Capability,
        iova: PhysAddr,
    ) -> Result<MappedRegion, MapError> {
        Self::check_cap(cap)?;
        let pos = match self.regions.iter().position(|r| {
            r.as_ref()
                .is_some_and(|x| Self::offset_in(x, iova, true).is_some())
        }) {
            Some(p) => p,
            None => return Err(MapError::NotMapped),
        };
        if self.regions[pos].as_ref().unwrap().tenant != cap.tenant {
            return Err(MapError::CrossTenant);
        }
        let region = self.regions[pos].take().unwrap();
        self.drop_pin_tables(&region);
        Ok(region)
    }

    fn drop_pin_tables(&mut self, region: &MappedRegion) {
        let sid = StreamId(region.stream_id);
        let pte = self.remove_s1(sid, region.iova.0);
        if let Some(ste_i) = self.ste_index(sid) {
            let (ipa, ilen) = pte
                .map(|p| (p.out, p.len))
                .unwrap_or((region.guest_pa.0, region.len));
            self.remove_s2_if_unused(ste_i, ipa, ilen);
        }
        self.invalidate_ats_range(sid, region.iova, region.len);
    }

    pub fn iter(&self) -> impl Iterator<Item = MappedRegion> + '_ {
        self.regions.iter().flatten().copied()
    }
}

impl Default for IommuMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::CapRights;
    use crate::types::TenantId;

    fn mem_cap(obj: u32, rights: u16, tenant: TenantId) -> Capability {
        Capability::new(CapKind::Memory, CapRights(rights), obj, tenant).with_generation(1)
    }

    #[test]
    fn stream_id_is_chiplet_not_bdf() {
        let sid = StreamId::accel(ChipletId(1), TileId(2), 3);
        assert_eq!(sid.chiplet(), ChipletId(1));
        assert_eq!(sid.tile(), TileId(2));
        assert_eq!(sid.ssid(), 3);
        // Packed as chiplet<<24 | tile<<8 | ssid — not PCI bus:dev.fn.
        assert_eq!(sid.raw(), 0x0100_0203);
        assert_ne!(sid.raw() & 0xff, ((2u32) << 3));
        assert_eq!(
            StreamId::from_raw(DEFAULT_STREAM),
            StreamId::accel(ChipletId(0), TileId(0), 0)
        );
    }

    #[test]
    fn abort_until_bound_capture_then_bind() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let sid = StreamId::from_raw(DEFAULT_STREAM);
        assert_eq!(iommu.stream_state(sid), StreamState::Unbound);
        assert_eq!(
            iommu.translate_result(DEFAULT_STREAM, PhysAddr(0x1000), None),
            Err(MapError::StreamAbort)
        );
        assert_eq!(iommu.capture(sid).unwrap(), StreamState::Captured);
        assert_eq!(iommu.stream_state(sid), StreamState::Captured);
        assert_eq!(iommu.ste_config(sid), SteConfig::Abort);
        assert_eq!(
            iommu.translate_result(DEFAULT_STREAM, PhysAddr(0x1000), None),
            Err(MapError::StreamAbort)
        );
        let no_map = mem_cap(1, CapRights::READ | CapRights::WRITE, TenantId(1));
        assert_eq!(iommu.bind_stream(&no_map, sid), Err(MapError::NoMemoryCap));
        assert_eq!(iommu.bind_stream(&cap, sid).unwrap(), StreamState::Bound);
        assert!(iommu.is_bound(sid));
        assert_eq!(iommu.ste_config(sid), SteConfig::Nested);
        assert_eq!(
            iommu.translate_result(DEFAULT_STREAM, PhysAddr(0x1000), None),
            Err(MapError::NotMapped)
        );
    }

    #[test]
    fn soft_smmu_non_identity_iova() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(7, CapRights::MEM_FULL.0, TenantId(1));
        let r = iommu
            .map(&cap, MapRequest::pin(PhysAddr(0x1000), 0x2000))
            .unwrap();
        assert_ne!(r.iova.0, 0x1000, "Soft SMMU must not identity-map");
        assert_eq!(r.iova.0, IommuMap::window_base(0, 0));
        assert_eq!(r.object, 7);
        assert_eq!(
            iommu.translate(PhysAddr(0x1800)).unwrap().0,
            r.iova.0 + 0x800
        );
        assert_eq!(iommu.resolve(PhysAddr(r.iova.0 + 0x800)).unwrap().0, 0x1800);
        let w = iommu
            .walk(
                StreamId::from_raw(DEFAULT_STREAM),
                PhysAddr(r.iova.0 + 0x800),
            )
            .unwrap();
        assert_eq!(w.pa.0, 0x1800);
        assert_eq!(w.ipa.0, 0x1800, "default Nested uses identity Stage-2");
        assert_eq!(w.config, SteConfig::Nested);
        assert!(iommu.covers(PhysAddr(0x1000), 0x2000));
        assert!(!iommu.covers(PhysAddr(0x1000), 0x2001));
        assert!(iommu.covers_iova(DEFAULT_STREAM, r.iova, 0x2000));
        assert!(iommu.is_bound(StreamId::from_raw(DEFAULT_STREAM)));
    }

    #[test]
    fn stream_a_and_b_pin_same_guest_pa() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let a = iommu
            .map(&cap, MapRequest::pin_stream(PhysAddr(0x2000), 0x1000, 1))
            .unwrap();
        let b = iommu
            .map(&cap, MapRequest::pin_stream(PhysAddr(0x2000), 0x1000, 2))
            .unwrap();
        assert_ne!(a.iova, b.iova);
        assert_ne!(a.iova.0, 0x2000);
        assert_ne!(b.iova.0, 0x2000);
        assert_eq!(
            iommu.translate_stream(1, PhysAddr(0x2400)).unwrap(),
            PhysAddr(a.iova.0 + 0x400)
        );
        assert_eq!(
            iommu.translate_stream(2, PhysAddr(0x2400)).unwrap(),
            PhysAddr(b.iova.0 + 0x400)
        );
        assert!(iommu.translate_stream(1, PhysAddr(0x2000)).is_some());
        // Same STE (chiplet0/tile0), SSID 3 has no CD → abort, not a hit.
        assert_eq!(
            iommu.stream_state(StreamId::from_raw(3)),
            StreamState::Captured
        );
        assert_eq!(
            iommu.translate_result(3, PhysAddr(0x2000), None),
            Err(MapError::StreamAbort)
        );
        assert_eq!(
            iommu.translate_result(1, PhysAddr(0x9000), None),
            Err(MapError::NotMapped)
        );
    }

    #[test]
    fn chiplet_stream_ids_are_distinct_stes() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let a = StreamId::accel(ChipletId(0), TileId(2), 0);
        let b = StreamId::accel(ChipletId(1), TileId(2), 0);
        let ra = iommu
            .map(&cap, MapRequest::pin_accel(PhysAddr(0x8000), 0x1000, a))
            .unwrap();
        let rb = iommu
            .map(&cap, MapRequest::pin_accel(PhysAddr(0x8000), 0x1000, b))
            .unwrap();
        assert_ne!(ra.iova, rb.iova);
        assert_eq!(iommu.ste_count(), 2);
        assert_eq!(
            iommu.translate_stream(a.raw(), PhysAddr(0x8400)).unwrap(),
            PhysAddr(ra.iova.0 + 0x400)
        );
        assert_eq!(
            iommu.translate_result(b.raw(), PhysAddr(0x9000), None),
            Err(MapError::NotMapped)
        );
        assert_eq!(
            iommu.translate_result(b.raw(), PhysAddr(0x8000), Some(TenantId(2))),
            Err(MapError::CrossTenant)
        );
        iommu.unbind_stream(a).unwrap();
        assert_eq!(
            iommu.translate_result(a.raw(), PhysAddr(0x8000), None),
            Err(MapError::StreamAbort)
        );
        assert!(iommu.translate_stream(b.raw(), PhysAddr(0x8000)).is_some());
    }

    #[test]
    fn translate_hit_miss_and_unmap() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let r = iommu
            .map(&cap, MapRequest::pin(PhysAddr(0x2000), 0x1000))
            .unwrap();
        assert!(iommu.translate(PhysAddr(0x2000)).is_some());
        assert!(iommu.translate(PhysAddr(0x2FFF)).is_some());
        assert!(iommu.translate(PhysAddr(0x3000)).is_none());
        iommu.unmap(r.iova).unwrap();
        assert!(iommu.translate(PhysAddr(0x2000)).is_none());
        assert_eq!(iommu.resolve(r.iova), None);
        assert_eq!(iommu.len(), 0);
        assert_eq!(iommu.unmap(r.iova), Err(MapError::NotMapped));
        // STE stays Bound after unmap; next pin does not re-abort.
        let r2 = iommu
            .map(&cap, MapRequest::pin(PhysAddr(0x4000), 0x1000))
            .unwrap();
        assert_ne!(r2.iova.0, 0x4000);
    }

    #[test]
    fn refuse_without_memory_cap() {
        let mut iommu = IommuMap::new();
        let ep = Capability::new(CapKind::Endpoint, CapRights::EP_FULL, 1, TenantId(1))
            .with_generation(1);
        assert_eq!(
            iommu
                .map(&ep, MapRequest::pin(PhysAddr(0x1000), 0x1000))
                .unwrap_err(),
            MapError::NoMemoryCap
        );
        let no_map = mem_cap(1, CapRights::READ | CapRights::WRITE, TenantId(1));
        assert_eq!(
            iommu
                .map(&no_map, MapRequest::pin(PhysAddr(0x1000), 0x1000))
                .unwrap_err(),
            MapError::NoMemoryCap
        );
        assert_eq!(
            iommu.stream_state(StreamId::from_raw(0)),
            StreamState::Unbound
        );
    }

    #[test]
    fn refuse_bad_range_and_same_stream_overlap() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        assert_eq!(
            iommu
                .map(&cap, MapRequest::pin(PhysAddr(0x1000), 0))
                .unwrap_err(),
            MapError::BadRange
        );
        iommu
            .map(&cap, MapRequest::pin(PhysAddr(0x1000), 0x1000))
            .unwrap();
        assert_eq!(
            iommu
                .map(&cap, MapRequest::pin(PhysAddr(0x1800), 0x1000))
                .unwrap_err(),
            MapError::Overlap
        );
    }

    #[test]
    fn refuse_cross_tenant_and_wrong_stream_unmap() {
        let mut iommu = IommuMap::new();
        let a = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let b = mem_cap(2, CapRights::MEM_FULL.0, TenantId(2));
        let r = iommu
            .map(&a, MapRequest::pin_stream(PhysAddr(0x4000), 0x1000, 4))
            .unwrap();
        assert_eq!(
            iommu
                .map(&b, MapRequest::pin_stream(PhysAddr(0x4000), 0x1000, 4))
                .unwrap_err(),
            MapError::CrossTenant
        );
        assert_eq!(
            iommu.translate_result(4, PhysAddr(0x4000), Some(TenantId(2))),
            Err(MapError::CrossTenant)
        );
        assert_eq!(iommu.unmap_stream(5, r.iova), Err(MapError::WrongStream));
        assert_eq!(iommu.unmap_for(&b, r.iova), Err(MapError::CrossTenant));
        let gone = iommu.unmap_for(&a, r.iova).unwrap();
        assert_eq!(gone.stream_id, 4);
        assert!(iommu.is_empty());
    }

    #[test]
    fn ssid_over_s1cdmax_aborts() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let bad = StreamId::accel(ChipletId(0), TileId(0), MAX_CDS as u8);
        assert_eq!(iommu.bind_stream(&cap, bad), Err(MapError::StreamAbort));
        assert_eq!(
            iommu.map(&cap, MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, bad)),
            Err(MapError::StreamAbort)
        );
        assert_eq!(iommu.stream_state(bad), StreamState::Unbound);
        assert_eq!(
            iommu.walk(bad, PhysAddr(SOFT_SMMU_IOVA_BASE)),
            Err(MapError::StreamAbort)
        );
    }

    #[test]
    fn nested_s1_s2_walk_and_s2_fault() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let sid = StreamId::accel(ChipletId(0), TileId(3), 1);
        assert_eq!(iommu.bind_nested(&cap, sid).unwrap(), StreamState::Bound);
        assert!(iommu.ste(sid).unwrap().distinct_ipa);
        let r = iommu
            .map(&cap, MapRequest::pin_accel(PhysAddr(0x5000), 0x1000, sid))
            .unwrap();
        assert_ne!(r.iova.0, 0x5000);
        let w = iommu.walk(sid, r.iova).unwrap();
        assert_eq!(w.config, SteConfig::Nested);
        assert_eq!(w.pa.0, 0x5000);
        assert_ne!(w.ipa.0, 0x5000);
        assert_ne!(w.ipa.0, r.iova.0);
        assert!(w.ipa.0 >= SOFT_SMMU_IPA_BASE);
        assert_eq!(iommu.resolve_stream(sid.raw(), r.iova).unwrap().0, 0x5000);
        iommu.unbind_stage2(sid).unwrap();
        assert_eq!(iommu.walk(sid, r.iova), Err(MapError::Stage2Fault));
        assert_eq!(
            iommu.resolve_result(sid.raw(), r.iova, None),
            Err(MapError::Stage2Fault)
        );
        assert!(iommu.is_bound(sid));
    }

    #[test]
    fn ats_invalidate_unbind_cd_and_flr() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let a = StreamId::accel(ChipletId(2), TileId(1), 0);
        let b = StreamId::accel(ChipletId(2), TileId(1), 1);
        let ra = iommu
            .map(&cap, MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, a))
            .unwrap();
        let rb = iommu
            .map(&cap, MapRequest::pin_accel(PhysAddr(0x3000), 0x1000, b))
            .unwrap();
        assert_eq!(iommu.resolve_ats(a.raw(), ra.iova).unwrap().0, 0x1000);
        assert_eq!(iommu.atc_misses(), 1);
        assert_eq!(iommu.atc_hits(), 0);
        assert_eq!(
            iommu
                .resolve_ats(a.raw(), PhysAddr(ra.iova.0 + 0x10))
                .unwrap()
                .0,
            0x1010
        );
        assert_eq!(iommu.atc_hits(), 1);
        assert_eq!(iommu.atc_len(), 1);
        let dropped = iommu
            .invalidate(InvCmd::Ats {
                sid: a,
                iova: Some(ra.iova),
                len: 0x1000,
            })
            .unwrap();
        assert_eq!(dropped, 1);
        assert_eq!(iommu.atc_len(), 0);
        assert_eq!(iommu.resolve_ats(a.raw(), ra.iova).unwrap().0, 0x1000);
        assert_eq!(iommu.atc_misses(), 2);
        let _ = iommu.invalidate(InvCmd::All).unwrap();
        iommu.unbind_cd(a).unwrap();
        assert_eq!(iommu.stream_state(a), StreamState::Captured);
        assert!(iommu.is_bound(b));
        assert_eq!(
            iommu.resolve_result(a.raw(), ra.iova, None),
            Err(MapError::StreamAbort)
        );
        assert_eq!(iommu.resolve_stream(b.raw(), rb.iova).unwrap().0, 0x3000);
        iommu.flr(b).unwrap();
        assert_eq!(iommu.stream_state(b), StreamState::Unbound);
        assert_eq!(iommu.walk(b, rb.iova), Err(MapError::StreamAbort));
        assert!(iommu.is_empty());
    }

    #[test]
    fn invalidate_does_not_drop_page_tables() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let r = iommu
            .map(&cap, MapRequest::pin(PhysAddr(0x8000), 0x1000))
            .unwrap();
        let _ = iommu.resolve_ats(DEFAULT_STREAM, r.iova).unwrap();
        iommu.invalidate(InvCmd::Tlbi { sid: None }).unwrap();
        iommu
            .invalidate(InvCmd::CfgSte {
                sid: StreamId::from_raw(DEFAULT_STREAM),
            })
            .unwrap();
        assert_eq!(iommu.resolve(r.iova).unwrap().0, 0x8000);
        assert_eq!(
            iommu
                .walk(StreamId::from_raw(DEFAULT_STREAM), r.iova)
                .unwrap()
                .pa
                .0,
            0x8000
        );
    }
}
