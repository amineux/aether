//! Host ABI sketch shaped like PJRT / IREE HAL.
//!
//! The kernel is a **submission shim + resource solver**. Compilers own the
//! ISA, graph IR, and fusion. This module names the objects a runtime
//! presents to the fabric — it does not parse or rewrite ML graphs.
//!
//! Host-facing nouns: Device, MemorySpace, Buffer, Executable, Event.
//! Event is a **research noun over existing fences** (partition
//! [`crate::fence::Timeline`], or SoftChipletSync chiplet/package
//! scope). The working host session is `aether-pjrt` (`host/aether-pjrt`):
//! it maps these nouns onto a frozen `IreeHalCmd` and submits into
//! IreeShapedCp. SoftNPU remains the path-B qemu demo. That crate is
//! not a PJRT plugin, not `GetPjRtApi`, not XLA, and not an IREE HAL
//! driver. `examples/accel-client` is a second tiny consumer of the
//! same image (doorbell sketch).

use crate::activity::ActivityId;
use crate::chipsync::SyncScope;
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

/// Completion event: a research noun over an existing fence.
///
/// Job Events (`scope = None`) are a [`FenceId`] on the partition
/// CP-shaped [`crate::fence::Timeline`]. Chiplet / package Events name a
/// [`crate::chipsync::SoftChipletSync`] scoped timeline that already
/// exists. This is **not** a new IR, not a CUDA stream, not
/// `GetPjRtApi`, and not XLA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub fence: FenceId,
    pub partition: PartitionId,
    /// `None` = partition timeline (job Events from execute).
    /// `Some(Chiplet | Package)` = SoftChipletSync visibility.
    pub scope: Option<SyncScope>,
}

impl Event {
    /// Partition-timeline Event (the execute / `signal_payload` path).
    pub const fn on_timeline(fence: FenceId, partition: PartitionId) -> Self {
        Self {
            fence,
            partition,
            scope: None,
        }
    }

    pub const fn with_scope(mut self, scope: SyncScope) -> Self {
        self.scope = Some(scope);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chipsync::SyncScope;
    use crate::fence::FenceId;
    use crate::partition::PartitionId;
    use crate::space::Place;
    use crate::types::ChipletId;

    #[test]
    fn buffer_is_not_unified_by_default() {
        let place = Place::new(ChipletId(0), MemorySpace::TileSram);
        let b = Buffer::new(MemorySpace::TileSram, FabricAddr::new(place, 0), 4096);
        assert!(!b.unified);
        assert_eq!(b.space, MemorySpace::TileSram);
    }

    #[test]
    fn event_is_a_fence_not_a_plugin() {
        let e = Event::on_timeline(FenceId(3), PartitionId(1));
        assert_eq!(e.fence, FenceId(3));
        assert_eq!(e.partition, PartitionId(1));
        assert!(
            e.scope.is_none(),
            "job Events stay on the partition timeline"
        );
        let scoped = e.with_scope(SyncScope::Chiplet);
        assert_eq!(scoped.scope, Some(SyncScope::Chiplet));
        assert_eq!(scoped.fence, FenceId(3));
    }
}
