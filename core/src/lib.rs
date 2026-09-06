//! Aether core: capability fabric, tile scheduler, tensor arenas, accel jobs.
//!
//! This crate is `no_std` and allocation-free so the same invariants run on the
//! host (`cargo test`) and inside the freestanding kernel.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod abi;
pub mod accel;
pub mod activity;
pub mod arena;
pub mod caps;
pub mod cut;
pub mod demo;
pub mod elf;
pub mod fabric;
pub mod fence;
pub mod hodge;
pub mod observe;
pub mod partition;
pub mod phase;
pub mod preempt;
pub mod sched;
pub mod space;
pub mod sysnr;
pub mod types;

pub use abi::{Buffer, Device, Event, Executable};
pub use accel::{AccelJobDesc, AccelOp, Completion, DType, SoftNpu};
pub use activity::{Activity, ActivityId, ActivityKind};
pub use arena::{ArenaAllocator, ArenaError, ArenaId, ArenaRequest};
pub use caps::{CapError, CapKind, CapRights, CapTable, Capability, CPtr};
pub use cut::{AffinityGraph, CutError, SpectralCut};
pub use demo::{run_boot_demo, DemoReport};
pub use elf::{parse_elf64, ElfError, ElfImage};
pub use fabric::{ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags};
pub use fence::{Fence, FenceId, Timeline};
pub use hodge::{FlowClass, HodgeError, HodgeQuota};
pub use observe::{EventKind, EventRing, KernelEvent};
pub use partition::{BlastRadius, PartitionId, PartitionProfile, QosBudget, SpatialSlice};
pub use phase::Phase;
pub use preempt::{CpuQueue, ThreadState, WaitWhy};
pub use sched::{Job, JobKind, TileKind, TileScheduler};
pub use space::{FabricAddr, MemorySpace, Place, SpaceError};
pub use sysnr::{
    UserAccelJob, UserCompletion, UserIpcMsg, INIT_EP_CPTR, INIT_QUEUE_CPTR, SYS_ACCEL_SUBMIT,
    SYS_ACCEL_WAIT, SYS_ARENA_ALLOC, SYS_DEBUG_PRINT, SYS_EXIT, SYS_MAP, SYS_RECV, SYS_SEND,
    SYS_UNMAP, SYS_YIELD, USER_IMAGE_BASE, USER_IMAGE_END, USER_STACK_TOP,
};
pub use types::{BankId, ChipletId, PhysAddr, TenantId, TileId};

/// Research-prototype version string printed by the boot demo.
pub const VERSION: &str = "0.1.0";
pub const NAME: &str = "Aether";
