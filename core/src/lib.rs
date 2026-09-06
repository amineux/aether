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
pub mod aspace;
pub mod bootfs;
pub mod caps;
pub mod color;
pub mod cut;
pub mod demo;
pub mod elf;
pub mod fabric;
pub mod fence;
pub mod hodge;
pub mod iommu;
pub mod laplacian;
pub mod mmap;
pub mod observe;
pub mod opkernel;
pub mod partition;
pub mod phase;
pub mod preempt;
pub mod ramfs;
pub mod reloc;
pub mod sched;
pub mod softfloat;
pub mod space;
pub mod sparsify;
pub mod sysnr;
pub mod types;

pub use abi::{Buffer, Device, Event, Executable};
pub use accel::{demo_f16_f32_ok, AccelJobDesc, AccelOp, Completion, DType, SoftNpu};
pub use activity::{Activity, ActivityId, ActivityKind};
pub use arena::{ArenaAllocator, ArenaError, ArenaId, ArenaRequest};
pub use bootfs::{
    parse_bootfs, pack_bootfs, BootFs, BootFsError, BootFsFile, BOOTFS_ENT, BOOTFS_HDR,
    BOOTFS_MAGIC, BOOTFS_MAX_FILES, BOOTFS_NAME, BOOTFS_SECTOR,
};
pub use aspace::{
    cr3_pa, cr3_pcid, cr3_tagged, hh_to_phys, identity_keep_2m, identity_keep_pa, kaslr_slide,
    kaslr_slide_valid, kernel_text_va_slid, parse_kaslr_cmdline, phys_to_hh, phys_to_hh_slid,
    phys_to_kva, IdentityAs, PcidAlloc, Sv39As, Ttbr0As, APIC_MMIO_BASE, APIC_MMIO_END, CR3_NOFLUSH,
    CR3_PCID_MASK, CR4_PCIDE, CR4_SMAP, CR4_SMEP, IDENTITY_KEEP_LOW, INVPCID_ALL,
    INVPCID_ALL_GLOBAL, INVPCID_INDIV, INVPCID_SINGLE, KASLR_HH_PD0, KASLR_HH_PD1, KASLR_KERNEL_SPAN,
    KASLR_MAILBOX, KASLR_MAILBOX_RELOCS, KASLR_MAILBOX_SLIDE, KASLR_SLIDE_COUNT, KASLR_SLIDE_STRIDE,
    KERNEL_HH_SPAN, KERNEL_LMA, KERNEL_TEXT_VA, KERNEL_VMA, KPTI_SLOT_BASE, KPTI_TRAMP_IDT,
    KPTI_TRAMP_PAS, KPTI_TRAMP_STACK, KPTI_TRAMP_STACK_TOP, KPTI_TRAMP_VA, PCID_KERNEL,
    PCID_USER_BASE,
};
pub use caps::{CPtr, CapError, CapKind, CapRights, CapTable, Capability, CdtNode};
pub use color::{admit_wave, BankColor, ColorError};
pub use cut::{AffinityGraph, CutError, CutId, SpectralCut};
pub use demo::{run_boot_demo, DemoReport};
pub use elf::{parse_elf64, ElfError, ElfImage};
pub use fabric::{ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags};
pub use fence::{Fence, FenceId, Timeline, TimelineId, MAX_IN_FLIGHT};
pub use hodge::{FlowClass, HodgeError, HodgeQuota};
pub use iommu::{
    IommuMap, MapError, MapRequest, MappedRegion, StreamId, StreamState, DEFAULT_STREAM,
    SOFT_SMMU_IOVA_BASE,
};
pub use laplacian::AffinityLaplacian;
pub use mmap::{
    parse_boot_mmap, plan_frames, span, MapSource, MemoryMap, MmapError, PhysRegion,
    BOOT_RESERVE_FLOOR, FRAME_CAP_BYTES, MB1_BOOT_MAGIC, MB2_BOOT_MAGIC,
};
pub use observe::{EventKind, EventRing, KernelEvent};
pub use opkernel::{CollectiveKind, OpKernelError, OpKernelId, OperatorKernelHandle};
pub use partition::{BlastRadius, PartitionId, PartitionProfile, QosBudget, SpatialSlice};
pub use phase::Phase;
pub use preempt::{CpuQueue, ThreadState, WaitWhy};
pub use ramfs::{RamFd, RamFs, RamFsError, RamHandle, INIT_PATH, PROBE_PATH};
pub use reloc::{
    apply_pie_image, apply_rela_bytes, apply_rela_dyn, parse_pie_trailer, parse_rela64, Rela64,
    RelocError, PIE_RELOC_MAGIC, PIE_TRAILER_SIZE, RELA64_SIZE, R_X86_64_RELATIVE,
};
pub use sched::{Job, JobKind, TileKind, TileScheduler};
pub use softfloat::{add_f16, add_f32, f16_to_f32, f32_to_f16, mul_f16, mul_f32};
pub use space::{FabricAddr, MemorySpace, Place, SpaceError};
pub use sparsify::{decide_header, SparsifiedCollective, SparsifyAction, DEFAULT_THRESHOLD_MILLI};
pub use sysnr::{
    UserAccelJob, UserCompletion, UserIpcMsg, INIT_EP_CPTR, INIT_QUEUE_CPTR, SYS_ACCEL_SUBMIT,
    SYS_ACCEL_WAIT, SYS_ARENA_ALLOC, SYS_CLONE, SYS_DEBUG_PRINT, SYS_EXIT, SYS_MAP, SYS_RECV,
    SYS_SEND, SYS_UNMAP, SYS_YIELD, USER_AA_IMAGE_BASE, USER_AA_IMAGE_END, USER_AA_STACK_TOP,
    USER_COW_BASE, USER_COW_END, USER_IMAGE_BASE, USER_IMAGE_END, USER_PROBE_BASE, USER_PROBE_END,
    USER_PROBE_STACK_TOP, USER_RV_IMAGE_BASE, USER_RV_IMAGE_END, USER_RV_STACK_TOP, USER_STACK_TOP,
    BLK_WINDOW_BASE, BLK_WINDOW_END, COW_PRIVATE_WORD, COW_TEMPLATE_WORD,
};
pub use types::{BankId, ChipletId, PhysAddr, TenantId, TileId};

/// Research-prototype version string printed by the boot demo.
pub const VERSION: &str = "0.1.0";
pub const NAME: &str = "Aether";
