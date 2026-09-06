//! Aether core: capability fabric, tile scheduler, tensor arenas, accel jobs.
//!
//! This crate is `no_std` and allocation-free so the same invariants run on the
//! host (`cargo test`) and inside the freestanding kernel.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod accel;
pub mod arena;
pub mod caps;
pub mod cut;
pub mod demo;
pub mod fabric;
pub mod hodge;
pub mod observe;
pub mod sched;
pub mod types;

pub use accel::{AccelJobDesc, AccelOp, Completion, DType, SoftNpu};
pub use arena::{ArenaAllocator, ArenaError, ArenaId, ArenaRequest};
pub use caps::{CapError, CapKind, CapRights, CapTable, Capability, CPtr};
pub use cut::{AffinityGraph, CutError, SpectralCut};
pub use demo::{run_boot_demo, DemoReport};
pub use fabric::{ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags};
pub use hodge::{FlowClass, HodgeError, HodgeQuota};
pub use observe::{EventKind, EventRing, KernelEvent};
pub use sched::{Job, JobKind, TileKind, TileScheduler};
pub use types::{BankId, PhysAddr, TenantId, TileId};

/// Research-prototype version string printed by the boot demo.
pub const VERSION: &str = "0.1.0";
pub const NAME: &str = "Aether";
