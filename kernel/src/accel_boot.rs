//! Required accelerator preparation before publishing the kernel World.
//!
//! Kept independent of kernel globals so the actual boot path can also be
//! exercised with the host software device and malformed capabilities/MMIO.
use aether_core::accel::DmaView;
use aether_core::caps::Capability;
use aether_core::iommu::{MapRequest, DEFAULT_STREAM};
use aether_core::types::PhysAddr;
use aether_drivers::SoftNpuDevice;
use aether_hal::{AccelDevice, HalError};

/// Negotiate, pin the initial image, and verify its complete DMA translation.
/// Callers must handle failure before making the device available to syscalls.
pub fn prepare<M: DmaView>(
    npu: &mut SoftNpuDevice<M>,
    cap: &Capability,
    base: PhysAddr,
    len: u64,
) -> Result<PhysAddr, HalError> {
    npu.probe()?;
    let iova = npu.map_with_cap(cap, MapRequest::pin(base, len))?;
    if npu
        .iommu
        .resolve_range(DEFAULT_STREAM, iova, len, Some(cap.tenant))
        != Ok(base)
    {
        return Err(HalError::Fault);
    }
    Ok(iova)
}
