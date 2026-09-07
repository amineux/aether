//! Typed memory spaces and fabric addresses.
//!
//! Memory is a typed place — never a single address space by default.
//! `UNIFIED_MEMORY` is an explicit capability bit ([`crate::cap::CapRights::UNIFIED`]),
//! never implied by [`crate::cap::CapRights::MEM_FULL`].
//!
//! A [`FabricAddr`] is a `(place, local)` tuple. Remote access is an explicit
//! DMA/NoC op, not a silent coherent load.

use crate::caps::CapRights;
use crate::types::ChipletId;

/// Host-visible memory space kind. Buffers bind to exactly one space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MemorySpace {
    /// Ordinary host DRAM (CPU tile local).
    Host = 0,
    /// Device high-bandwidth memory (off-tile, on-package).
    DeviceHbm = 1,
    /// Per-tile scratch SRAM — first-class, not a cache of HBM.
    TileSram = 2,
    /// CXL.mem-inspired typed place. Coherence is not assumed across the
    /// link. [`crate::window::TypedWindow`] (`CxlMemStub`) is the pin/map
    /// stub — not a CXL.mem HDM decoder and not QEMU CXL silicon.
    CxlRegion = 3,
    /// Transient scratch (software-managed, not cacheable).
    Scratch = 4,
    /// Streaming / FIFO / on-chip fabric buffer (no random load).
    Streaming = 5,
}

impl MemorySpace {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Host => "HOST",
            Self::DeviceHbm => "DEVICE_HBM",
            Self::TileSram => "TILE_SRAM",
            Self::CxlRegion => "CXL_REGION",
            Self::Scratch => "SCRATCH",
            Self::Streaming => "STREAMING",
        }
    }

    /// Random load/store from a host CPU is only legal in Host (and
    /// UNIFIED-mapped regions, which are a capability, not a space).
    pub const fn allows_host_load(self) -> bool {
        matches!(self, Self::Host)
    }

    pub const fn is_on_tile(self) -> bool {
        matches!(self, Self::TileSram | Self::Scratch | Self::Streaming)
    }
}

/// A place in the fabric: chiplet + optional tile + space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub chiplet: ChipletId,
    pub tile: Option<u16>,
    pub space: MemorySpace,
}

impl Place {
    pub const fn new(chiplet: ChipletId, space: MemorySpace) -> Self {
        Self {
            chiplet,
            tile: None,
            space,
        }
    }

    pub const fn with_tile(mut self, tile: u16) -> Self {
        self.tile = Some(tile);
        self
    }

    pub const fn same_chiplet(self, other: Place) -> bool {
        self.chiplet.0 == other.chiplet.0
    }
}

/// Fabric address: `(place, local)`. The local field is an offset *inside*
/// that place, not a globally unique virtual address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricAddr {
    pub place: Place,
    pub local: u64,
}

impl FabricAddr {
    pub const fn new(place: Place, local: u64) -> Self {
        Self { place, local }
    }

    pub const fn is_remote(self, here: Place) -> bool {
        if self.place.chiplet.0 != here.chiplet.0 {
            return true;
        }
        if self.place.space as u8 != here.space as u8 {
            return true;
        }
        match (self.place.tile, here.tile) {
            (Some(a), Some(b)) => a != b,
            (None, None) => false,
            _ => true,
        }
    }
}

/// Why a map / load / bind was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceError {
    /// Caller tried to treat a remote `(place, local)` as a coherent load.
    SilentRemoteLoad,
    /// Buffer space does not match the job / arena binding.
    SpaceMismatch,
    /// UNIFIED_MEMORY bit required and not present.
    UnifiedNotGranted,
    /// Streaming / scratch space cannot be randomly mapped.
    NotMappable,
}

/// Map a fabric address as if it were a local load. Remote places and
/// non-mappable spaces are refused — use an explicit DMA/NoC Exchange.
pub fn map_place(here: Place, addr: FabricAddr) -> Result<crate::types::PhysAddr, SpaceError> {
    if matches!(addr.place.space, MemorySpace::Streaming | MemorySpace::Scratch) {
        return Err(SpaceError::NotMappable);
    }
    if addr.is_remote(here) {
        return Err(SpaceError::SilentRemoteLoad);
    }
    Ok(crate::types::PhysAddr(addr.local))
}

/// Coherent load across spaces requires an explicit UNIFIED grant.
pub fn coherent_load(rights: CapRights, here: Place, addr: FabricAddr) -> Result<(), SpaceError> {
    if !addr.is_remote(here) && addr.place.space.allows_host_load() {
        return Ok(());
    }
    if rights.contains(CapRights::UNIFIED) {
        return Ok(());
    }
    Err(SpaceError::UnifiedNotGranted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_is_not_a_space() {
        // Six named spaces; UNIFIED is a capability bit, not a seventh space.
        assert_eq!(MemorySpace::Host as u8, 0);
        assert_eq!(MemorySpace::Streaming as u8, 5);
        assert!(!MemorySpace::DeviceHbm.allows_host_load());
        assert!(MemorySpace::TileSram.is_on_tile());
    }

    #[test]
    fn remote_addr_is_explicit() {
        let here = Place::new(ChipletId(0), MemorySpace::TileSram).with_tile(0);
        let there = FabricAddr::new(
            Place::new(ChipletId(1), MemorySpace::DeviceHbm),
            0x1000,
        );
        assert!(there.is_remote(here));
        let local = FabricAddr::new(here, 0x40);
        assert!(!local.is_remote(here));
        assert_eq!(
            map_place(here, there),
            Err(SpaceError::SilentRemoteLoad)
        );
        assert_eq!(
            coherent_load(CapRights::MEM_FULL, here, there),
            Err(SpaceError::UnifiedNotGranted)
        );
    }
}
