//! Typed memory windows — CXL.mem-inspired nouns, **software stub only**.
//!
//! [`TypedWindow`] is a host/kernel range `{ base, len, kind, sid }` that
//! Soft SMMU can pin with Memory+MAP rights. Kinds are [`WindowKind::Hbm`],
//! [`WindowKind::CxlMemStub`], and [`WindowKind::Dram`].
//!
//! This is **Exploration E**, not a SpecForge Y2H1 calendar item:
//! - **Not** a CXL.mem HDM decoder, not QEMU `cxl-type3` / CXL.host, not
//!   silicon, not cache-coherent fabric memory.
//! - [`WindowKind::CxlMemStub`] borrows CXL.mem vocabulary for a typed
//!   fabric window. [`crate::space::MemorySpace::CxlRegion`] stays a
//!   typed place; this module is the pin/map stub that place lacked.
//! - Coherent remote load is still refused without `UNIFIED`.
//!
//! Soft SMMU: [`IommuMap::map_window`] / [`IommuMap::unmap_window`].
//! Wrong SID is [`MapError::WrongStream`]. A foreign-tenant window is
//! [`MapError::CrossTenant`] on the pin path and [`CutError::CrossCut`]
//! at SpectralCut bind ([`crate::cut::bind_window`]).

use crate::caps::Capability;
use crate::iommu::{IommuMap, MapError, MapRequest, MappedRegion, StreamId};
use crate::space::MemorySpace;
use crate::types::{PhysAddr, TenantId};

/// Kind of a typed window. CXL.mem is inspiration for the stub variant
/// only — never a claim of a Host-managed Device Memory decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WindowKind {
    /// On-package HBM stack (device-local). Not CXL.
    Hbm = 0,
    /// CXL.mem-inspired typed fabric window. Software stub only.
    CxlMemStub = 1,
    /// Host DRAM.
    Dram = 2,
}

impl WindowKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Hbm => "HBM",
            Self::CxlMemStub => "CXL_MEM_STUB",
            Self::Dram => "DRAM",
        }
    }

    /// Matching typed place. `CxlMemStub` maps to `CxlRegion` — still not
    /// a CXL.mem window in silicon.
    pub const fn space(self) -> MemorySpace {
        match self {
            Self::Hbm => MemorySpace::DeviceHbm,
            Self::CxlMemStub => MemorySpace::CxlRegion,
            Self::Dram => MemorySpace::Host,
        }
    }
}

/// A typed memory window Soft SMMU / AccelDevice can pin.
///
/// `sid` is a software [`StreamId`] (chiplet | tile | ssid), not a PCIe
/// BDF and not a CXL.mem decoder index. `tenant` is the owner; a foreign
/// tenant pin is `CrossTenant` / SpectralCut `CrossCut`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedWindow {
    pub base: PhysAddr,
    pub len: u64,
    pub kind: WindowKind,
    pub sid: StreamId,
    pub tenant: TenantId,
}

impl TypedWindow {
    pub const fn new(
        base: PhysAddr,
        len: u64,
        kind: WindowKind,
        sid: StreamId,
        tenant: TenantId,
    ) -> Self {
        Self {
            base,
            len,
            kind,
            sid,
            tenant,
        }
    }

    /// Soft-SMMU pin request on this window's SID.
    pub const fn request(self) -> MapRequest {
        MapRequest::pin_accel(self.base, self.len, self.sid)
    }
}

/// Result of [`IommuMap::map_window`]: the request plus the non-identity pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappedWindow {
    pub window: TypedWindow,
    pub region: MappedRegion,
}

impl IommuMap {
    /// Pin a typed window through Soft SMMU.
    ///
    /// Requires Memory+MAP. The window's tenant must match the cap.
    /// IOVA is non-identity on the window's SID. This does **not**
    /// program a CXL.mem HDM decoder.
    pub fn map_window(
        &mut self,
        cap: &Capability,
        win: TypedWindow,
    ) -> Result<MappedWindow, MapError> {
        Self::check_cap(cap)?;
        if cap.tenant != win.tenant {
            return Err(MapError::CrossTenant);
        }
        if win.len == 0 {
            return Err(MapError::BadRange);
        }
        let region = self.map(cap, win.request())?;
        Ok(MappedWindow {
            window: win,
            region,
        })
    }

    /// Pin `win` only if `sid` equals `win.sid`. Mismatch is `WrongStream`.
    pub fn map_window_sid(
        &mut self,
        cap: &Capability,
        win: TypedWindow,
        sid: StreamId,
    ) -> Result<MappedWindow, MapError> {
        if sid != win.sid {
            return Err(MapError::WrongStream);
        }
        self.map_window(cap, win)
    }

    /// Unmap a window IOVA on `sid`. Wrong SID is `WrongStream`; wrong
    /// tenant is `CrossTenant`. Memory+MAP is required.
    pub fn unmap_window(
        &mut self,
        cap: &Capability,
        sid: StreamId,
        iova: PhysAddr,
    ) -> Result<MappedRegion, MapError> {
        Self::check_cap(cap)?;
        let Some(r) = self.region_at_iova(iova) else {
            return Err(MapError::NotMapped);
        };
        if r.stream_id != sid.raw() {
            return Err(MapError::WrongStream);
        }
        if r.tenant != cap.tenant {
            return Err(MapError::CrossTenant);
        }
        self.unmap_for(cap, iova)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{CapKind, CapRights, CapTable};
    use crate::cut::{bind_window, CutError, SpectralCut};
    use crate::space::{map_place, FabricAddr, Place, SpaceError};
    use crate::types::{ChipletId, TileId};

    fn mem_cap(obj: u32, tenant: TenantId) -> Capability {
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, obj, tenant).with_generation(1)
    }

    fn win(kind: WindowKind, sid: StreamId, tenant: TenantId) -> TypedWindow {
        TypedWindow::new(PhysAddr(0xB000), 0x1000, kind, sid, tenant)
    }

    #[test]
    fn kinds_are_honest_nouns() {
        assert_eq!(WindowKind::Hbm.name(), "HBM");
        assert_eq!(WindowKind::CxlMemStub.name(), "CXL_MEM_STUB");
        assert_eq!(WindowKind::Dram.name(), "DRAM");
        assert_eq!(WindowKind::Hbm.space(), MemorySpace::DeviceHbm);
        assert_eq!(WindowKind::CxlMemStub.space(), MemorySpace::CxlRegion);
        assert_eq!(WindowKind::Dram.space(), MemorySpace::Host);
        assert!(!WindowKind::CxlMemStub.space().allows_host_load());
    }

    #[test]
    fn map_window_non_identity_and_unmap() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, TenantId(1));
        let sid = StreamId::accel(ChipletId(0), TileId(2), 2);
        let w = win(WindowKind::CxlMemStub, sid, TenantId(1));
        let mapped = iommu.map_window(&cap, w).unwrap();
        assert_eq!(mapped.window.kind, WindowKind::CxlMemStub);
        assert_ne!(mapped.region.iova.0, w.base.0);
        assert_eq!(mapped.region.stream_id, sid.raw());
        assert_eq!(
            iommu.resolve_stream(sid.raw(), mapped.region.iova).unwrap(),
            w.base
        );
        let gone = iommu.unmap_window(&cap, sid, mapped.region.iova).unwrap();
        assert_eq!(gone.stream_id, sid.raw());
        assert!(iommu
            .resolve_stream(sid.raw(), mapped.region.iova)
            .is_none());
    }

    #[test]
    fn map_window_wrong_sid_and_rights() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap(1, TenantId(1));
        let sid = StreamId::accel(ChipletId(0), TileId(2), 2);
        let other = StreamId::accel(ChipletId(0), TileId(2), 3);
        let w = win(WindowKind::Hbm, sid, TenantId(1));
        assert_eq!(
            iommu.map_window_sid(&cap, w, other),
            Err(MapError::WrongStream)
        );
        let mapped = iommu.map_window_sid(&cap, w, sid).unwrap();
        assert_eq!(
            iommu.unmap_window(&cap, other, mapped.region.iova),
            Err(MapError::WrongStream)
        );
        assert_eq!(
            iommu.resolve_result(other.raw(), mapped.region.iova, None),
            Err(MapError::StreamAbort)
        );
        let no_map = Capability::new(
            CapKind::Memory,
            CapRights(CapRights::READ | CapRights::WRITE),
            1,
            TenantId(1),
        )
        .with_generation(1);
        assert_eq!(
            iommu.map_window(&no_map, win(WindowKind::Dram, sid, TenantId(1))),
            Err(MapError::NoMemoryCap)
        );
        assert!(iommu.region_at_iova(mapped.region.iova).is_some());
    }

    #[test]
    fn foreign_tenant_pin_and_crosscut() {
        let mut iommu = IommuMap::new();
        let a = mem_cap(1, TenantId(1));
        let b = mem_cap(2, TenantId(2));
        let sid = StreamId::accel(ChipletId(0), TileId(2), 2);
        let win_a = win(WindowKind::CxlMemStub, sid, TenantId(1));
        let win_b = TypedWindow::new(
            PhysAddr(0xC000),
            0x1000,
            WindowKind::Dram,
            StreamId::accel(ChipletId(1), TileId(1), 0),
            TenantId(2),
        );
        assert_eq!(iommu.map_window(&b, win_a), Err(MapError::CrossTenant));
        let mapped = iommu.map_window(&a, win_a).unwrap();
        assert_eq!(
            iommu.unmap_window(&b, sid, mapped.region.iova),
            Err(MapError::CrossTenant)
        );

        let mut caps = CapTable::new(TenantId(1));
        let (_g, cut) = SpectralCut::qemu_chiplet_cut(400).unwrap();
        let cptr = caps
            .mint(crate::caps::Capability::new(
                CapKind::SpectralCut,
                CapRights::CUT_FULL,
                cut.id.0,
                TenantId(1),
            ))
            .unwrap();
        assert_eq!(
            bind_window(&caps, cptr, &cut, &win_b, TenantId(1)),
            Err(CutError::CrossCut)
        );
        assert!(bind_window(&caps, cptr, &cut, &win_a, TenantId(1)).is_ok());
        assert_eq!(
            cut.allow_window(&win_b, TenantId(1)),
            Err(CutError::CrossCut)
        );
    }

    #[test]
    fn cxl_stub_is_not_a_silent_load() {
        let here = Place::new(ChipletId(0), MemorySpace::TileSram);
        let there = FabricAddr::new(
            Place::new(ChipletId(1), WindowKind::CxlMemStub.space()),
            0x80,
        );
        assert_eq!(map_place(here, there), Err(SpaceError::SilentRemoteLoad));
    }
}
