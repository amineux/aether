//! Bounded authority for pins created by SYS_MAP, separate from boot pins.
use aether_core::accel::DmaView;
use aether_core::caps::Capability;
use aether_core::iommu::{MapRequest, MAX_MAPS};
use aether_core::types::PhysAddr;
use aether_drivers::SoftNpuDevice;
use aether_hal::HalError;

#[derive(Clone, Copy)]
struct Pin {
    iova: PhysAddr,
    cap: Capability,
}

pub struct DmaPins<const N: usize = MAX_MAPS> {
    entries: [Option<Pin>; N],
}

impl<const N: usize> DmaPins<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(Option::is_none)
    }

    pub fn map<M: DmaView>(
        &mut self,
        npu: &mut SoftNpuDevice<M>,
        cap: &Capability,
        base: PhysAddr,
        len: u64,
    ) -> Result<PhysAddr, HalError> {
        // Reserve tracking capacity before changing the device. A failed map
        // leaves both this slot and all earlier authorities unchanged.
        let slot = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(HalError::Busy)?;
        let iova = npu.map_with_cap(cap, MapRequest::pin(base, len))?;
        self.entries[slot] = Some(Pin { iova, cap: *cap });
        Ok(iova)
    }

    pub fn unmap<M: DmaView>(
        &mut self,
        npu: &mut SoftNpuDevice<M>,
        iova: PhysAddr,
    ) -> Result<(), HalError> {
        // Exact starts only: never authorize an interior address or a boot pin.
        let slot = self
            .entries
            .iter()
            .position(|p| p.is_some_and(|p| p.iova == iova))
            .ok_or(HalError::Fault)?;
        let pin = self.entries[slot].unwrap();
        npu.unmap_with_cap(&pin.cap, pin.iova)?;
        self.entries[slot] = None;
        Ok(())
    }
}
