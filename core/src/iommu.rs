//! Soft SMMU: a software stream-ID page-table for accelerator DMA.
//!
//! Shape matches a future hardware SMMU / IOMMU: a Memory cap authorizes a
//! guest PA range, each stream is its own IOVA namespace, and `translate`
//! / `resolve` walk a software STE → CD block table. This is **not** a
//! hardware SMMU and **not** a full SMMUv3 emulator (no command queue,
//! event queue, stage-2, or MMIO register file).
//!
//! StreamIDs are **accelerator / chiplet** identities
//! (`chiplet | tile | ssid`), not PCIe BDF. That is the intentional
//! hole versus CPU IOMMU emulators (QEMU / OpenVMM), which index the
//! stream table by guest RequesterID.
//!
//! Lifecycle (OpenVMM / smmuv3-accel shaped, software only):
//! - A SID starts **Unbound**. DMA translate **aborts** until it is Bound.
//! - **Capture** on first sighting (map or an explicit `capture`) records
//!   the STE; it does not authorize translation.
//! - **Bind** (Memory+MAP, or the first authorized `map`) installs a
//!   context descriptor (SSID → CD) and enables the stream.
//! - `unbind_stream` is the FLR analogue: STE drops to Unbound and its
//!   pins are forgotten.
//!
//! Policy:
//! - Overlap is **per full SID** (STE + SSID) on guest PA. Two SIDs may
//!   pin the same guest PA to different IOVAs.
//! - IOVAs are allocated from per-(STE, CD) windows above 4 GiB, so a
//!   typical QEMU guest PA is never identity-mapped (`iova != guest_pa`).
//! - Entries are block descriptors (one pin = one range), not 4K PTEs.
//! - `CrossTenant` / `WrongStream` / `StreamAbort` / `NotMapped` as below.
//!
//! Bank QoS / bandwidth coloring is out of scope here.

use crate::caps::{CapKind, CapRights, Capability};
use crate::types::{ChipletId, PhysAddr, TenantId, TileId, PAGE_4K};

pub const MAX_MAPS: usize = 16;
/// Software stream-table size (STE slots). Not a hardware stream-table.
pub const MAX_STES: usize = 8;
/// Context descriptors per STE (SSID → CD). Not a hardware CD table.
pub const MAX_CDS: usize = 8;

/// SoftNPU / default virtqueue stream: chiplet 0, tile 0, ssid 0.
pub const DEFAULT_STREAM: u32 = 0;

/// Soft-SMMU IOVA window base (4 GiB). Guest RAM in the QEMU 128 MiB
/// demo lives well below this, so allocated IOVAs are non-identity.
pub const SOFT_SMMU_IOVA_BASE: u64 = 0x0001_0000_0000;

/// 256 MiB of IOVA per STE slot.
pub const SOFT_SMMU_STE_SHIFT: u32 = 28;
/// 4 MiB of IOVA per CD slot inside an STE window.
pub const SOFT_SMMU_CD_SHIFT: u32 = 22;

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

/// One pinned Soft-SMMU block descriptor.
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
    /// SID is Unbound or Captured, or its CD is missing. DMA must abort.
    StreamAbort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContextDesc {
    ssid: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StreamTableEntry {
    key: u32,
    state: StreamState,
    tenant: TenantId,
    cds: [Option<ContextDesc>; MAX_CDS],
}

/// Software IOMMU: STE → CD → guest PA ↔ IOVA block table.
#[derive(Clone, Debug)]
pub struct IommuMap {
    stes: [Option<StreamTableEntry>; MAX_STES],
    regions: [Option<MappedRegion>; MAX_MAPS],
}

impl IommuMap {
    pub const fn new() -> Self {
        Self {
            stes: [None; MAX_STES],
            regions: [None; MAX_MAPS],
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

    /// IOVA window for software STE slot `ste_idx` and CD slot `cd_slot`.
    pub const fn window_base(ste_idx: usize, cd_slot: usize) -> u64 {
        SOFT_SMMU_IOVA_BASE
            .wrapping_add((ste_idx as u64) << SOFT_SMMU_STE_SHIFT)
            .wrapping_add((cd_slot as u64) << SOFT_SMMU_CD_SHIFT)
    }

    /// Authorize + pin. Refuses anything that is not a Memory cap with MAP.
    /// First authorized use **captures and binds** the SID, then allocates
    /// a non-identity IOVA in that stream namespace.
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
        let _ = self.capture(sid)?;
        let ste_i = self.ste_index(sid).ok_or(MapError::TableFull)?;
        {
            let ste = self.stes[ste_i].as_mut().unwrap();
            if ste.state == StreamState::Bound && ste.tenant != cap.tenant {
                return Err(MapError::CrossTenant);
            }
            ste.tenant = cap.tenant;
            ste.state = StreamState::Bound;
        }
        self.ensure_cd(ste_i, sid.ssid())?;
        Ok(StreamState::Bound)
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
        self.stes[ste_i] = None;
        Ok(())
    }

    pub fn stream_state(&self, sid: StreamId) -> StreamState {
        self.ste(sid)
            .map(|s| {
                if s.state != StreamState::Bound {
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
                .position(|c| c.as_ref().is_some_and(|d| d.ssid == sid.ssid()))
        })
    }

    fn ensure_cd(&mut self, ste_i: usize, ssid: u8) -> Result<usize, MapError> {
        if let Some(i) = self.stes[ste_i].as_ref().and_then(|e| {
            e.cds
                .iter()
                .position(|c| c.as_ref().is_some_and(|d| d.ssid == ssid))
        }) {
            return Ok(i);
        }
        let slot = self.stes[ste_i]
            .as_ref()
            .unwrap()
            .cds
            .iter()
            .position(|c| c.is_none())
            .ok_or(MapError::TableFull)?;
        self.stes[ste_i].as_mut().unwrap().cds[slot] = Some(ContextDesc { ssid });
        Ok(slot)
    }

    fn require_bound(&self, sid: StreamId) -> Result<(), MapError> {
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

    fn alloc_iova(
        &self,
        sid: StreamId,
        guest_pa: PhysAddr,
        len: u64,
    ) -> Result<PhysAddr, MapError> {
        let span = Self::page_align_up(len).ok_or(MapError::BadRange)?;
        let ste_i = self.ste_index(sid).ok_or(MapError::StreamAbort)?;
        let cd_i = self.cd_slot_of(sid).ok_or(MapError::StreamAbort)?;
        let base = Self::window_base(ste_i, cd_i);
        let window = 1u64 << SOFT_SMMU_CD_SHIFT;
        let mut next = base;
        for r in self.regions.iter().flatten() {
            if r.stream_id != sid.raw() {
                continue;
            }
            let rspan = Self::page_align_up(r.len).ok_or(MapError::BadRange)?;
            let end = r.iova.0.checked_add(rspan).ok_or(MapError::BadRange)?;
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
        Ok(PhysAddr(next))
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
        self.region_at_iova(iova).and_then(|r| {
            if !self.is_bound(StreamId(r.stream_id)) {
                return None;
            }
            let off = iova.0 - r.iova.0;
            Some(PhysAddr(r.guest_pa.0 + off))
        })
    }

    /// IOVA → guest PA on `stream_id`.
    pub fn resolve_stream(&self, stream_id: u32, iova: PhysAddr) -> Option<PhysAddr> {
        self.resolve_result(stream_id, iova, None).ok()
    }

    /// IOVA → guest PA with explicit errors.
    pub fn resolve_result(
        &self,
        stream_id: u32,
        iova: PhysAddr,
        tenant: Option<TenantId>,
    ) -> Result<PhysAddr, MapError> {
        let sid = StreamId(stream_id);
        self.require_bound(sid)?;
        match self.region_at_iova(iova) {
            Some(r) if r.stream_id == stream_id => {
                if let Some(t) = tenant {
                    if r.tenant != t {
                        return Err(MapError::CrossTenant);
                    }
                }
                let off = iova.0 - r.iova.0;
                Ok(PhysAddr(r.guest_pa.0 + off))
            }
            Some(_) => Err(MapError::WrongStream),
            None => Err(MapError::NotMapped),
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
            if !self.is_bound(StreamId(r.stream_id)) {
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
        Ok(self.regions[pos].take().unwrap())
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
        Ok(self.regions[pos].take().unwrap())
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
        Ok(self.regions[pos].take().unwrap())
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
        assert_eq!(
            iommu.translate_result(DEFAULT_STREAM, PhysAddr(0x1000), None),
            Err(MapError::StreamAbort)
        );
        let no_map = mem_cap(1, CapRights::READ | CapRights::WRITE, TenantId(1));
        assert_eq!(iommu.bind_stream(&no_map, sid), Err(MapError::NoMemoryCap));
        assert_eq!(iommu.bind_stream(&cap, sid).unwrap(), StreamState::Bound);
        assert!(iommu.is_bound(sid));
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
}
