//! In-tree drivers. SoftNPU services the virtqueue MMIO window.
//! [`SoftCommandProcessor`] is the host-tested command-processor path
//! (packed packet + Soft SMMU + IRQ/fence). [`PartnerNpuStub`] remains
//! a documented no-op sketch.

#![cfg_attr(not(test), no_std)]

pub mod fakecp;
pub mod mmio;
pub mod partner;
pub mod softnpu;
pub mod virtio_accel;

pub use fakecp::{CpCmd, SoftCommandProcessor, CP_CMD_SIZE, CP_PKT_MAGIC};
pub use mmio::{
    AccelMmio, MmioTraceEntry, ACCEL_MMIO_SIZE, GOLDEN_CFG_OFFS, REG_DOORBELL, REG_IRQ_STATUS,
    REG_MAGIC, REG_QSIZE, REG_STATUS, REG_USED_IDX, REG_VERSION,
};
pub use partner::{PartnerCmd, PartnerNpuStub};
pub use softnpu::SoftNpuDevice;
pub use virtio_accel::{VirtioAccelQueue, VIRTIO_ACCEL_MAGIC, VIRTIO_ACCEL_VERSION};
