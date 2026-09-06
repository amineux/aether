//! In-tree drivers. SoftNPU services the virtqueue MMIO window. A silicon
//! partner starts from [`PartnerNpuStub`] and replaces the no-op backend
//! with their command packet + IRQ.

#![cfg_attr(not(test), no_std)]

pub mod mmio;
pub mod partner;
pub mod softnpu;
pub mod virtio_accel;

pub use mmio::{AccelMmio, ACCEL_MMIO_SIZE, REG_DOORBELL, REG_IRQ_STATUS, REG_MAGIC};
pub use partner::{PartnerCmd, PartnerNpuStub};
pub use softnpu::SoftNpuDevice;
pub use virtio_accel::{VirtioAccelQueue, VIRTIO_ACCEL_MAGIC, VIRTIO_ACCEL_VERSION};
