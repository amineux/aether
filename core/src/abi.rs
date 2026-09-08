//! Host ABI sketch shaped like PJRT / IREE HAL.
//!
//! The kernel is a **submission shim + resource solver**. Compilers own the
//! ISA, graph IR, and fusion. This module names the objects a runtime
//! presents to the fabric — it does not parse or rewrite ML graphs.
//!
//! Host-facing nouns: Device, MemorySpace, Buffer, Executable, Event.
//! The working host session is `aether-pjrt` (`host/aether-pjrt`): it
//! maps these nouns onto a frozen `IreeHalCmd` and submits into
//! IreeShapedCp. SoftNPU remains the path-B qemu demo. That crate is
//! not a PJRT plugin and not an IREE HAL driver. `examples/accel-client`
//! is a second tiny consumer of the same image (doorbell sketch).

use crate::activity::ActivityId;
use crate::fence::FenceId;
use crate::partition::PartitionId;
use crate::space::{FabricAddr, MemorySpace};

/// A published device / activity the runtime can bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub activity: ActivityId,
    pub partition: PartitionId,
}

/// A buffer bound to exactly one typed memory space.
/// `unified` is true only when [`crate::caps::CapRights::UNIFIED`] was granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffer {
    pub space: MemorySpace,
    pub addr: FabricAddr,
    pub len: u64,
    pub unified: bool,
}

impl Buffer {
    pub const fn new(space: MemorySpace, addr: FabricAddr, len: u64) -> Self {
        Self {
            space,
            addr,
            len,
            unified: false,
        }
    }
}

/// Compiler-owned executable. The kernel stores a handle; it does not
/// interpret the ISA blob or fuse ops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Executable {
    pub activity: ActivityId,
    /// Opaque compiler artifact id (ELF / PJRT executable / IREE HAL module).
    pub isa_blob_id: u32,
}

/// Completion event: a fence on a partition timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub fence: FenceId,
    pub partition: PartitionId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::Place;
    use crate::types::ChipletId;

    #[test]
    fn buffer_is_not_unified_by_default() {
        let place = Place::new(ChipletId(0), MemorySpace::TileSram);
        let b = Buffer::new(MemorySpace::TileSram, FabricAddr::new(place, 0), 4096);
        assert!(!b.unified);
        assert_eq!(b.space, MemorySpace::TileSram);
    }
}
