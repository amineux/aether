//! Pin / translate API for accelerator DMA.
//!
//! Shape matches a future SMMU / IOMMU: a Memory cap authorizes a guest
//! PA range, the map table records it, and `translate` returns an IOVA.
//! QEMU UP identity-maps (`iova == guest_pa`). Hardware stream IDs and
//! page-table walks are not programmed here.

use crate::caps::{CapKind, CapRights, Capability};
use crate::types::{PhysAddr, TenantId};

pub const MAX_MAPS: usize = 16;

/// Pin request. `stream_id` is the future SMMU SID (0 on SoftNPU / QEMU).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapRequest {
    pub guest_pa: PhysAddr,
    pub len: u64,
    pub stream_id: u32,
    pub writable: bool,
}

impl MapRequest {
    pub const fn pin(guest_pa: PhysAddr, len: u64) -> Self {
        Self {
            guest_pa,
            len,
            stream_id: 0,
            writable: true,
        }
    }
}

/// One pinned window. Identity IOVA on QEMU UP.
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
    Overlap,
    TableFull,
    NotMapped,
    CrossTenant,
}

/// Software IOMMU: tracks pinned guest PA → IOVA translations.
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
    pub fn map(
        &mut self,
        cap: &Capability,
        req: MapRequest,
    ) -> Result<MappedRegion, MapError> {
        Self::check_cap(cap)?;
        if req.len == 0 {
            return Err(MapError::BadRange);
        }
        if req.guest_pa.0.checked_add(req.len).is_none() {
            return Err(MapError::BadRange);
        }
        if self.overlaps(req.guest_pa, req.len) {
            return Err(MapError::Overlap);
        }
        let slot = self
            .regions
            .iter()
            .position(|r| r.is_none())
            .ok_or(MapError::TableFull)?;
        let region = MappedRegion {
            guest_pa: req.guest_pa,
            iova: req.guest_pa, // identity — future SMMU would allocate an IOVA
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

    fn overlaps(&self, pa: PhysAddr, len: u64) -> bool {
        let start = pa.0;
        let end = start + len;
        self.regions.iter().flatten().any(|r| {
            let rs = r.guest_pa.0;
            let re = rs + r.len;
            start < re && rs < end
        })
    }

    /// Identity translate. `None` if the PA was never pinned.
    pub fn translate(&self, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.region_at(guest_pa).map(|r| {
            let off = guest_pa.0 - r.guest_pa.0;
            PhysAddr(r.iova.0 + off)
        })
    }

    pub fn covers(&self, pa: PhysAddr, len: u64) -> bool {
        if len == 0 {
            return false;
        }
        let end = match pa.0.checked_add(len) {
            Some(e) => e,
            None => return false,
        };
        // A range may span adjacent regions; require every byte covered.
        let mut cursor = pa.0;
        while cursor < end {
            let Some(r) = self.region_at(PhysAddr(cursor)) else {
                return false;
            };
            let re = r.guest_pa.0 + r.len;
            if re <= cursor {
                return false;
            }
            cursor = re;
        }
        true
    }

    pub fn region_at(&self, pa: PhysAddr) -> Option<&MappedRegion> {
        self.regions.iter().flatten().find(|r| {
            let start = r.guest_pa.0;
            pa.0 >= start && pa.0 < start + r.len
        })
    }

    pub fn unmap(&mut self, iova: PhysAddr) -> Result<MappedRegion, MapError> {
        let pos = self
            .regions
            .iter()
            .position(|r| r.as_ref().is_some_and(|x| x.iova.0 == iova.0))
            .ok_or(MapError::NotMapped)?;
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
    fn identity_map_with_memory_cap() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(7, CapRights::MEM_FULL.0, TenantId(1));
        let r = iommu
            .map(&cap, MapRequest::pin(PhysAddr(0x1000), 0x2000))
            .unwrap();
        assert_eq!(r.iova.0, 0x1000);
        assert_eq!(r.object, 7);
        assert_eq!(iommu.translate(PhysAddr(0x1800)).unwrap().0, 0x1800);
        assert!(iommu.covers(PhysAddr(0x1000), 0x2000));
        assert!(!iommu.covers(PhysAddr(0x1000), 0x2001));
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
    fn refuse_bad_range_and_overlap() {
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
    fn unmap_forgets_region() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, CapRights::MEM_FULL.0, TenantId(1));
        let r = iommu
            .map(&cap, MapRequest::pin(PhysAddr(0x2000), 0x1000))
            .unwrap();
        iommu.unmap(r.iova).unwrap();
        assert!(iommu.translate(PhysAddr(0x2000)).is_none());
        assert_eq!(iommu.len(), 0);
    }
}
