//! In-tree drivers. SoftNPU services the virtqueue MMIO window.
//! Path A (`path_a`) is a host Soft-SMMU IOVA contract on the same frozen
//! BAR the optional QEMU `-device aether-accel` exposes. Stock `make qemu`
//! stays path B.
//! [`SoftCommandProcessor`] is the host-tested Aether-native CP path
//! (packed `CpCmd` + SET_SID-at-submit + two software XQueues +
//! SoftGreenCtx SM/WQ partitions + SoftChipletSync scoped timelines +
//! SoftCCT elision + SoftNoI-IS admit +
//! SoftCmdFirewall copy-then-validate + IRQ/fence).
//! SoftSFI (`softsfi`) is a toy bytecode sandbox beside the CP.
//! PASID/SVA (`sva`) binds process VA ↔ Soft-SMMU SSID on this CP.
//! OperatorInject (`opinject`) is the resident worker + versioned op table
//! (memcpy / saxpy + hot-add scale; GPUOS / Mirage MPK inspiration).
//! SoftNoI-IS (`noi`) is Interference Score admit on a fake shared NoI
//! plus a software fabric-class tag (Curl reserved ring).
//! [`IreeShapedCp`] is the partner-shaped HAL spine: an IREE HAL dispatch
//! packet, not Soft-CP 2.0 (still a single mailbox; optional scoped
//! timelines). [`PartnerNpuStub`] remains a documented no-op sketch.

#![cfg_attr(not(test), no_std)]

pub mod fakecp;
pub mod firewall;
pub mod ireecp;
pub mod mmio;
pub mod noi;
pub mod opinject;
pub mod partner;
pub mod path_a;
pub mod softnpu;
pub mod softsfi;
pub mod sva;
pub mod virtio_accel;

pub use fakecp::{
    CpCmd, SoftCommandProcessor, XQueue, XQueueState, CP_CMD_SIZE, CP_FLAG_SET_SID, CP_PKT_MAGIC,
    CP_SSID, SOFT_CP_XQUEUES, XQUEUE_DEPTH,
};
pub use firewall::{
    run_firewall_demo, FirewallMode, FirewallReport, FirewallSim, SoftCmdFirewall, FIREWALL_SLOTS,
};
pub use ireecp::{IreeHalCmd, IreeShapedCp, IREE_HAL_CMD_SIZE, IREE_HAL_PKT_MAGIC, IREE_SSID};
pub use mmio::{
    AccelMmio, MmioTraceEntry, ACCEL_MMIO_SIZE, GOLDEN_CFG_OFFS, REG_DOORBELL, REG_IRQ_STATUS,
    REG_MAGIC, REG_QSIZE, REG_STATUS, REG_USED_IDX, REG_VERSION,
};
pub use partner::{PartnerCmd, PartnerNpuStub};
pub use path_a::{path_a_sid, PathABar, PATH_A_PCI_DEVICE, PATH_A_SSID};
pub use softnpu::{IdentityDma, KernelDma, SoftNpuDevice};
pub use virtio_accel::{VirtioAccelQueue, VIRTIO_ACCEL_MAGIC, VIRTIO_ACCEL_VERSION};
