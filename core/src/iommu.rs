//! Soft SMMU: a software stream-ID page-table for accelerator DMA.
//!
//! Shape matches a future hardware SMMU / IOMMU: a Memory cap authorizes a
//! guest PA range, each `stream_id` is its own IOVA namespace, and
//! `translate` / `resolve` walk the software page table. This is **not**
//! a hardware SMMU — QEMU never programs a real SID or PT walk.
//!
//! Policy (documented so a later hardware cut can match it):
//! - Overlap is **per stream** on guest PA. Two streams may pin the same
//!   guest PA to different IOVAs.
//! - IOVAs are allocated from a per-stream window above 4 GiB, so a
//!   typical QEMU guest PA is never identity-mapped (`iova != guest_pa`).
//! - Entries are block descriptors (one pin = one range), not 4K PTEs.
//! - `CrossTenant` if another tenant already occupies an overlapping
//!   window on the same stream, or if unmap/translate is authorized by
//!   the wrong tenant.
//! - `WrongStream` if the IOVA / PA exists only on a different stream.

use crate::caps::{CapKind, CapRights, Capability};
use crate::types::{PhysAddr, TenantId, PAGE_4K};

pub const MAX_MAPS: usize = 16;

/// SoftNPU / default virtqueue stream. Hardware SIDs are not programmed.
pub const DEFAULT_STREAM: u32 = 0;

/// Soft-SMMU IOVA window base (4 GiB). Guest RAM in the QEMU 128 MiB
/// demo lives well below this, so allocated IOVAs are non-identity.
pub const SOFT_SMMU_IOVA_BASE: u64 = 0x0001_0000_0000;

/// 256 MiB of IOVA per `stream_id`. Lookup still uses the full u32 SID;
/// this shift only places allocator windows.
pub const SOFT_SMMU_STREAM_SHIFT: u32 = 28;

/// Pin request. `stream_id` selects a Soft-SMMU context (software SID).
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
    /// Guest-PA overlap on the **same** stream (same tenant).
    Overlap,
    TableFull,
    NotMapped,
    /// Another tenant owns the window, or the authorizing cap's tenant
    /// does not match the pinned region.
    CrossTenant,
    /// The PA / IOVA is mapped, but not on the requested stream.
    WrongStream,
}

/// Software IOMMU: per-stream guest PA ↔ IOVA block table.
#[derive(Clone, Debug)]
pub struct IommuMap {
    regions: [Option<MappedRegion>; MAX_MAPS],
}

impl IommuMap {
    pub const fn new() -> Self {
        Self {
            regions: [None; MAX_MAPS],
        }
    }

    pub fn len(&self) -> usize {
        self.regions.iter().filter(|r| r.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Authorize + pin. Refuses anything that is not a Memory cap with MAP.
    /// Allocates a non-identity IOVA in the request's stream namespace.
    pub fn map(&mut self, cap: &Capability, req: MapRequest) -> Result<MappedRegion, MapError> {
        Self::check_cap(cap)?;
        if req.len == 0 {
            return Err(MapError::BadRange);
        }
        if req.guest_pa.0.checked_add(req.len).is_none() {
            return Err(MapError::BadRange);
        }
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
        let iova = self.alloc_iova(req.stream_id, req.guest_pa, req.len)?;
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

    /// Soft-SMMU context base for `stream_id`.
    pub const fn stream_iova_base(stream_id: u32) -> u64 {
        SOFT_SMMU_IOVA_BASE.wrapping_add((stream_id as u64) << SOFT_SMMU_STREAM_SHIFT)
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
        stream_id: u32,
        guest_pa: PhysAddr,
        len: u64,
    ) -> Result<PhysAddr, MapError> {
        let span = Self::page_align_up(len).ok_or(MapError::BadRange)?;
        let base = Self::stream_iova_base(stream_id);
        let window = 1u64 << SOFT_SMMU_STREAM_SHIFT;
        let mut next = base;
        for r in self.regions.iter().flatten() {
            if r.stream_id != stream_id {
                continue;
            }
            let rspan = Self::page_align_up(r.len).ok_or(MapError::BadRange)?;
            let end = r.iova.0.checked_add(rspan).ok_or(MapError::BadRange)?;
            if end > next {
                next = end;
            }
        }
        // Never emit identity IOVA, even if a guest PA lands in the window.
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

    /// Translate guest PA → IOVA on `stream_id`. Miss if unmapped or
    /// pinned only on another stream.
    pub fn translate_stream(&self, stream_id: u32, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.region_at_stream(stream_id, guest_pa).map(|r| {
            let off = guest_pa.0 - r.guest_pa.0;
            PhysAddr(r.iova.0 + off)
        })
    }

    /// Translate with explicit errors (unmapped / wrong stream / tenant).
    pub fn translate_result(
        &self,
        stream_id: u32,
        guest_pa: PhysAddr,
        tenant: Option<TenantId>,
    ) -> Result<PhysAddr, MapError> {
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

    /// IOVA → guest PA on any stream (windows are disjoint by construction).
    pub fn resolve(&self, iova: PhysAddr) -> Option<PhysAddr> {
        self.region_at_iova(iova).map(|r| {
            let off = iova.0 - r.iova.0;
            PhysAddr(r.guest_pa.0 + off)
        })
    }

    /// IOVA → guest PA on `stream_id`.
    pub fn resolve_stream(&self, stream_id: u32, iova: PhysAddr) -> Option<PhysAddr> {
        self.region_at_iova(iova).and_then(|r| {
            if r.stream_id != stream_id {
                return None;
            }
            let off = iova.0 - r.iova.0;
            Some(PhysAddr(r.guest_pa.0 + off))
        })
    }

    /// IOVA → guest PA with explicit errors.
    pub fn resolve_result(
        &self,
        stream_id: u32,
        iova: PhysAddr,
        tenant: Option<TenantId>,
    ) -> Result<PhysAddr, MapError> {
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
        self.covers_in(stream_id, pa, len, false)
    }

    /// True if every byte of `[iova, iova+len)` is pinned on `stream_id`.
    pub fn covers_iova(&self, stream_id: u32, iova: PhysAddr, len: u64) -> bool {
        self.covers_in(stream_id, iova, len, true)
    }

    /// Guest-PA or IOVA coverage on any stream (device DMA after rewrite).
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
        Capability {
            kind: CapKind::Memory,
            rights: CapRights(rights),
            object: obj,
            badge: 0,
            generation: 1,
            tenant,
        }
    }

    #[test]
    fn soft_smmu_non_identity_iova() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(7, CapRights::MEM_FULL.0, TenantId(1));
        let r = iommu
            .map(&cap, MapRequest::pin(PhysAddr(0x1000), 0x2000))
            .unwrap();
        assert_ne!(r.iova.0, 0x1000, "Soft SMMU must not identity-map");
        assert_eq!(r.iova.0, IommuMap::stream_iova_base(DEFAULT_STREAM));
        assert_eq!(r.object, 7);
        assert_eq!(
            iommu.translate(PhysAddr(0x1800)).unwrap().0,
            r.iova.0 + 0x800
        );
        assert_eq!(iommu.resolve(PhysAddr(r.iova.0 + 0x800)).unwrap().0, 0x1800);
        assert!(iommu.covers(PhysAddr(0x1000), 0x2000));
        assert!(!iommu.covers(PhysAddr(0x1000), 0x2001));
        assert!(iommu.covers_iova(DEFAULT_STREAM, r.iova, 0x2000));
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
        assert!(iommu.translate_stream(3, PhysAddr(0x2000)).is_none());
        assert_eq!(
            iommu.translate_result(3, PhysAddr(0x2000), None),
            Err(MapError::WrongStream)
        );
        assert_eq!(
            iommu.translate_result(1, PhysAddr(0x9000), None),
            Err(MapError::NotMapped)
        );
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
    }

    #[test]
    fn refuse_without_memory_cap() {
        let mut iommu = IommuMap::new();
        let ep = Capability {
            kind: CapKind::Endpoint,
            rights: CapRights::EP_FULL,
            object: 1,
            badge: 0,
            generation: 1,
            tenant: TenantId(1),
        };
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
