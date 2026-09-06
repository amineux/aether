//! In-tree drivers. A silicon partner replaces `SoftNpuBackend` with MMIO
//! to their command queue and keeps `VirtioAccelQueue` as the control plane
//! if they want a virtio-compatible QEMU device later.

#![cfg_attr(not(test), no_std)]

pub mod softnpu;
pub mod virtio_accel;

pub use softnpu::SoftNpuDevice;
pub use virtio_accel::{VirtioAccelQueue, VIRTIO_ACCEL_MAGIC, VIRTIO_ACCEL_VERSION};
