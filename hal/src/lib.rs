//! Hardware abstraction for Aether.
//!
//! Arch crates and drivers implement these traits. The kernel talks to
//! tiles through `AccelDevice` so a real NPU, a VirtIO device, and the
//! software reference model share one submit/complete/map contract.
//!
//! Porting to RISC-V / aarch64 means implementing `Console` + `Timer` +
//! interrupt ack; the fabric, caps, arenas, and scheduler stay unchanged.

#![cfg_attr(not(test), no_std)]

use aether_core::accel::{AccelJobDesc, Completion};
use aether_core::iommu::MapRequest;
use aether_core::space::{map_place, FabricAddr, Place, SpaceError};
use aether_core::types::PhysAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccelInfo {
    pub vendor: u32,
    pub device: u32,
    pub n_queues: u16,
    pub max_wave: u16,
    /// Backend discriminator. See [`ACCEL_BACKEND_SOFTNPU`] and siblings.
    pub backend: u8,
}

/// In-process SoftNPU software model (Dummy / reference execute).
pub const ACCEL_BACKEND_SOFTNPU: u8 = 0;
/// SoftNPU behind the in-kernel virtqueue MMIO BAR (QEMU demo).
pub const ACCEL_BACKEND_VIRTIO_SOFTNPU: u8 = 1;
/// Documented no-op partner sketch. Not a working command processor.
pub const ACCEL_BACKEND_PARTNER_STUB: u8 = 2;
/// Software command processor: packed packet + Soft SMMU + IRQ/fence.
pub const ACCEL_BACKEND_SOFT_CP: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HalError {
    NotFound,
    Busy,
    BadArg,
    Fault,
    Unsupported,
    /// Map refused: no Memory cap walk, or the pin was never authorized.
    NoMemoryCap,
}

/// Accelerator doorbell / IRQ contract.
pub trait AccelDevice {
    fn probe(&mut self) -> Result<AccelInfo, HalError>;
    /// Write a job into the avail ring and kick the doorbell. Does not
    /// execute the job; completions arrive on the used ring / IRQ.
    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError>;
    /// Driver-side used-ring read. Does not service the device.
    fn poll(&mut self) -> Option<Completion>;
    /// Pin a guest PA range the device may DMA. Returns the Soft-SMMU IOVA.
    ///
    /// Callers must have already walked a Memory cap with MAP (see
    /// [`aether_core::iommu::IommuMap::map`]). Implementations may still
    /// refuse an unauthorized pin. IOVA is not identity; `stream_id` selects
    /// a software context. This is not a hardware SMMU.
    ///
    /// This is a *local* pin. Remote `(place, local)` addresses must go
    /// through [`map_fabric`] — never a silent coherent load.
    fn map(&mut self, req: MapRequest) -> Result<PhysAddr, HalError>;
    fn unmap(&mut self, iova: PhysAddr) -> Result<(), HalError> {
        let _ = iova;
        Ok(())
    }
    /// Translate a guest PA through the device's Soft-SMMU table (stream 0).
    fn translate(&self, guest_pa: PhysAddr) -> Option<PhysAddr> {
        let _ = guest_pa;
        None
    }
    /// Translate on an explicit software stream ID.
    fn translate_stream(&self, stream_id: u32, guest_pa: PhysAddr) -> Option<PhysAddr> {
        let _ = stream_id;
        self.translate(guest_pa)
    }
    fn name(&self) -> &'static str;
}

/// Refuse a silent remote load. Callers that want a remote byte use an
/// explicit DMA/NoC Exchange job, not this helper.
pub fn map_fabric(here: Place, addr: FabricAddr) -> Result<PhysAddr, SpaceError> {
    map_place(here, addr)
}

pub trait Console {
    fn write_bytes(&mut self, bytes: &[u8]);
}

pub trait Timer {
    fn ticks(&self) -> u64;
    fn set_hz(&mut self, hz: u32);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::iommu::MapRequest;

    struct Dummy;
    impl AccelDevice for Dummy {
        fn probe(&mut self) -> Result<AccelInfo, HalError> {
            Ok(AccelInfo {
                vendor: 0xAE7E,
                device: 1,
                n_queues: 1,
                max_wave: 8,
                backend: ACCEL_BACKEND_SOFTNPU,
            })
        }
        fn submit(&mut self, _job: &AccelJobDesc) -> Result<u32, HalError> {
            Err(HalError::Unsupported)
        }
        fn poll(&mut self) -> Option<Completion> {
            None
        }
        fn map(&mut self, req: MapRequest) -> Result<PhysAddr, HalError> {
            if req.len == 0 {
                return Err(HalError::BadArg);
            }
            Ok(req.guest_pa)
        }
        fn name(&self) -> &'static str {
            "dummy"
        }
    }

    #[test]
    fn probe_dummy() {
        let mut d = Dummy;
        assert_eq!(d.probe().unwrap().vendor, 0xAE7E);
        assert_eq!(d.name(), "dummy");
        let iova = d
            .map(MapRequest::pin(
                aether_core::types::PhysAddr(0x1000),
                0x1000,
            ))
            .unwrap();
        assert_eq!(iova.0, 0x1000);
    }

    #[test]
    fn map_fabric_refuses_silent_remote() {
        use aether_core::space::MemorySpace;
        use aether_core::types::ChipletId;
        let here = Place::new(ChipletId(0), MemorySpace::TileSram);
        let there = FabricAddr::new(Place::new(ChipletId(1), MemorySpace::CxlRegion), 0x80);
        assert_eq!(map_fabric(here, there), Err(SpaceError::SilentRemoteLoad));
        let local = FabricAddr::new(here, 0x40);
        assert_eq!(map_fabric(here, local).unwrap().0, 0x40);
    }
}
