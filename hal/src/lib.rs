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
use aether_core::greenctx::SmWqBudget;
use aether_core::iommu::MapRequest;
use aether_core::space::{map_place, FabricAddr, Place, SpaceError};
use aether_core::types::PhysAddr;
use aether_core::window::TypedWindow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccelInfo {
    pub vendor: u32,
    pub device: u32,
    pub n_queues: u16,
    pub max_wave: u16,
    /// Backend discriminator. See [`ACCEL_BACKEND_SOFTNPU`] and siblings.
    pub backend: u8,
    /// Fake SM pool advertised by Soft-CP. Zero = no SoftGreenCtx.
    pub sm_count: u16,
    /// Fake work-queue pool advertised by Soft-CP. Zero = no SoftGreenCtx.
    pub wq_count: u16,
}

/// In-process SoftNPU software model (Dummy / reference execute).
pub const ACCEL_BACKEND_SOFTNPU: u8 = 0;
/// SoftNPU behind the in-kernel virtqueue MMIO BAR (QEMU demo).
pub const ACCEL_BACKEND_VIRTIO_SOFTNPU: u8 = 1;
/// Documented no-op partner sketch. Not a working command processor.
pub const ACCEL_BACKEND_PARTNER_STUB: u8 = 2;
/// Software command processor: packed packet + Soft SMMU + IRQ/fence.
pub const ACCEL_BACKEND_SOFT_CP: u8 = 3;
/// IREE HAL-shaped command processor: public Device/Buffer/Executable/Event
/// nouns packed into a frozen dispatch packet. Not a vendor, not Soft-CP 2.0.
pub const ACCEL_BACKEND_IREE_SHAPED: u8 = 4;

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

/// Honest preemption grain for software XQueues.
///
/// Soft-CP implements [`PreemptionLevel::QueueBoundary`] only: `suspend`
/// refuses to start the next packed command on that queue. A command
/// already inside `service()` / the integer engine runs to completion.
/// [`PreemptionLevel::MidOp`] is named so callers do not overclaim; no
/// in-tree backend returns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreemptionLevel {
    QueueBoundary = 0,
    MidOp = 1,
}

/// Accelerator doorbell / IRQ contract.
pub trait AccelDevice {
    fn probe(&mut self) -> Result<AccelInfo, HalError>;
    /// Write a job into the avail ring and kick the doorbell. Does not
    /// execute the job; completions arrive on the used ring / IRQ.
    ///
    /// Device-shaped default: queue 0. Soft-CP XQueue submit is
    /// [`Self::submit_queue`].
    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError>;
    /// Create / stamp a software XQueue (id, sticky Soft-SMMU SID, priority).
    ///
    /// Default: unsupported. Soft-CP implements two queues. Not a silicon
    /// queuing unit; not an XSched LD_PRELOAD shim.
    fn create_queue(&mut self, queue: u16, stream_id: u32, priority: u8) -> Result<(), HalError> {
        let _ = (queue, stream_id, priority);
        Err(HalError::Unsupported)
    }
    /// Submit onto a software XQueue. Default: queue 0 == [`submit`].
    fn submit_queue(&mut self, queue: u16, job: &AccelJobDesc) -> Result<u32, HalError> {
        if queue != 0 {
            return Err(HalError::Unsupported);
        }
        self.submit(job)
    }
    /// Park a queue at the next command boundary. Default: unsupported.
    fn suspend_queue(&mut self, queue: u16) -> Result<PreemptionLevel, HalError> {
        let _ = queue;
        Err(HalError::Unsupported)
    }
    /// Unpark a suspended XQueue. Default: unsupported.
    fn resume_queue(&mut self, queue: u16) -> Result<(), HalError> {
        let _ = queue;
        Err(HalError::Unsupported)
    }
    /// Soft-CP SM/WQ budget. Default: none (no SoftGreenCtx).
    ///
    /// Soft partition, not MIG / BAR firewall. CUDA Green Contexts /
    /// DetShare are inspiration only.
    fn sm_wq_budget(&self) -> Option<SmWqBudget> {
        None
    }
    /// SoftNoI-IS estimate for a tenant. Default: none (no fake NoI).
    ///
    /// PARL / NoI inspiration. Admit control, not topology synthesis.
    fn noi_is_milli(&self, tenant: u32) -> Option<u32> {
        let _ = tenant;
        None
    }
    /// Admit a tenant onto the fake NoI. Default: unsupported.
    ///
    /// Returns the projected IS in milli (1000 = 1.0×). Refuse with
    /// [`HalError::Busy`] when IS > budget.
    fn admit_noi(&mut self, tenant: u32, demand: u32) -> Result<u32, HalError> {
        let _ = (tenant, demand);
        Err(HalError::Unsupported)
    }
    /// Create a SoftGreenCtx with an exclusive SM/WQ slice.
    ///
    /// Default: unsupported. Soft-CP implements a fake 10-SM / 10-WQ pool.
    fn create_green_ctx(&mut self, sm: u16, wq: u16) -> Result<u16, HalError> {
        let _ = (sm, wq);
        Err(HalError::Unsupported)
    }
    /// Bind an XQueue to a SoftGreenCtx. Default: unsupported.
    fn bind_queue_ctx(&mut self, queue: u16, ctx: u16) -> Result<(), HalError> {
        let _ = (queue, ctx);
        Err(HalError::Unsupported)
    }
    /// Migrate-to-yield: rebind a yielded XQueue to another SoftGreenCtx.
    /// Soft-SMMU SID must stay put. Default: unsupported.
    fn migrate_queue_ctx(&mut self, queue: u16, dest_ctx: u16) -> Result<(), HalError> {
        let _ = (queue, dest_ctx);
        Err(HalError::Unsupported)
    }
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
    /// Pin a [`TypedWindow`] (HBM / CXL.mem stub / DRAM) on its SID.
    ///
    /// Default forwards to [`Self::map`] with the window's SID. Cap-gated
    /// drivers still refuse until `map_window_with_cap`. This is not a
    /// CXL.mem HDM decoder.
    fn map_window(&mut self, win: TypedWindow) -> Result<PhysAddr, HalError> {
        self.map(win.request())
    }
    fn unmap(&mut self, iova: PhysAddr) -> Result<(), HalError> {
        let _ = iova;
        Ok(())
    }
    /// Bind process mm ↔ this device's Soft-SMMU SSID (PASID space).
    ///
    /// Default: unsupported. Soft-CP implements it with Memory+MAP
    /// (`bind_sva_with_cap`). Linux SVA inspiration; not ARM SVA / PCIe
    /// PASID / CUDA UVA.
    fn bind_sva(&mut self, mm: u16, stream_id: u32) -> Result<u8, HalError> {
        let _ = (mm, stream_id);
        Err(HalError::Unsupported)
    }
    /// Pin process VA → guest PA on a SVA-bound SSID. Returns the VA
    /// (the DMA address). Default: unsupported.
    fn map_va(
        &mut self,
        stream_id: u32,
        va: PhysAddr,
        guest_pa: PhysAddr,
        len: u64,
    ) -> Result<PhysAddr, HalError> {
        let _ = (stream_id, va, guest_pa, len);
        Err(HalError::Unsupported)
    }
    /// Unmap a process VA and invalidate that SSID's software TLB.
    /// Default: unsupported.
    fn unmap_va(&mut self, stream_id: u32, va: PhysAddr) -> Result<(), HalError> {
        let _ = (stream_id, va);
        Err(HalError::Unsupported)
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
                sm_count: 0,
                wq_count: 0,
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
        assert_eq!(d.probe().unwrap().sm_count, 0);
        assert_eq!(d.probe().unwrap().wq_count, 0);
        assert_eq!(d.sm_wq_budget(), None);
        assert_eq!(d.create_green_ctx(7, 7).unwrap_err(), HalError::Unsupported);
        assert_eq!(d.name(), "dummy");
        let iova = d
            .map(MapRequest::pin(
                aether_core::types::PhysAddr(0x1000),
                0x1000,
            ))
            .unwrap();
        assert_eq!(iova.0, 0x1000);
        assert_eq!(d.bind_sva(0x100, 0).unwrap_err(), HalError::Unsupported);
        assert_eq!(
            d.map_va(0, PhysAddr(0x40_0000), PhysAddr(0x1000), 0x1000)
                .unwrap_err(),
            HalError::Unsupported
        );
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
