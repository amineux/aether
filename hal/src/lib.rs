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
use aether_core::space::{map_place, FabricAddr, Place, SpaceError};
use aether_core::types::PhysAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccelInfo {
    pub vendor: u32,
    pub device: u32,
    pub n_queues: u16,
    pub max_wave: u16,
    /// 0 = software model, 1 = virtio, 2 = silicon.
    pub backend: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HalError {
    NotFound,
    Busy,
    BadArg,
    Fault,
    Unsupported,
}

/// Accelerator doorbell / IRQ contract.
pub trait AccelDevice {
    fn probe(&mut self) -> Result<AccelInfo, HalError>;
    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError>;
    fn poll(&mut self) -> Option<Completion>;
    /// Pin a physical range the device may DMA. Ownership must already
    /// have been transferred via the fabric (arena + cap).
    ///
    /// This is a *local* pin. Remote `(place, local)` addresses must go
    /// through [`map_fabric`] — never a silent coherent load.
    fn map(&mut self, base: PhysAddr, size: u64) -> Result<(), HalError>;
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

    struct Dummy;
    impl AccelDevice for Dummy {
        fn probe(&mut self) -> Result<AccelInfo, HalError> {
            Ok(AccelInfo {
                vendor: 0xAE7E,
                device: 1,
                n_queues: 1,
                max_wave: 8,
                backend: 0,
            })
        }
        fn submit(&mut self, _job: &AccelJobDesc) -> Result<u32, HalError> {
            Err(HalError::Unsupported)
        }
        fn poll(&mut self) -> Option<Completion> {
            None
        }
        fn map(&mut self, _base: PhysAddr, _size: u64) -> Result<(), HalError> {
            Ok(())
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
