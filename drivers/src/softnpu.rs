//! Software NPU tile bound to the VirtIO-Accel MMIO virtqueue.
//!
//! The kernel driver talks the MMIO + virtqueue ABI (`submit` kicks the
//! doorbell; `poll` reads the used ring). SoftNPU is the device-side
//! executor: [`SoftNpuDevice::service`] drains the avail ring on
//! poll/IRQ. No custom QEMU device is required.

use aether_core::accel::{AccelJobDesc, Completion, DmaView, SoftNpu};
use aether_core::caps::Capability;
use aether_core::iommu::{IommuMap, MapRequest};
use aether_core::types::PhysAddr;
use aether_hal::{AccelDevice, AccelInfo, HalError};

use crate::mmio::AccelMmio;

/// Identity-mapped physical memory (kernel). Host tests use `SliceDma`.
pub struct IdentityDma;

impl DmaView for IdentityDma {
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, aether_core::accel::AccelError> {
        let p = addr.0 as *const i32;
        // SAFETY: caller mapped + owns the arena (cap + transfer).
        Ok(unsafe { core::ptr::read_volatile(p) })
    }
    fn store_i32(
        &mut self,
        addr: PhysAddr,
        val: i32,
    ) -> Result<(), aether_core::accel::AccelError> {
        let p = addr.0 as *mut i32;
        unsafe { core::ptr::write_volatile(p, val) };
        Ok(())
    }
}

/// DMA through a caller-provided buffer (demo / tests).
pub struct FnDma<L, S>
where
    L: Fn(PhysAddr) -> i32,
    S: FnMut(PhysAddr, i32),
{
    pub load: L,
    pub store: S,
}

impl<L, S> DmaView for FnDma<L, S>
where
    L: Fn(PhysAddr) -> i32,
    S: FnMut(PhysAddr, i32),
{
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, aether_core::accel::AccelError> {
        Ok((self.load)(addr))
    }
    fn store_i32(
        &mut self,
        addr: PhysAddr,
        val: i32,
    ) -> Result<(), aether_core::accel::AccelError> {
        (self.store)(addr, val);
        Ok(())
    }
}

pub struct SoftNpuDevice<M: DmaView> {
    pub info: AccelInfo,
    pub mmio: AccelMmio,
    pub npu: SoftNpu,
    pub mem: M,
    pub iommu: IommuMap,
}

impl<M: DmaView> SoftNpuDevice<M> {
    pub fn new(mem: M) -> Self {
        Self {
            info: AccelInfo {
                vendor: 0xAE7E,
                device: 0x0001,
                n_queues: 1,
                max_wave: 64,
                backend: 1, // virtio-shaped MMIO, SoftNPU backend
            },
            mmio: AccelMmio::new(),
            npu: SoftNpu::new(),
            mem,
            iommu: IommuMap::new(),
        }
    }

    /// Authorize + pin through the software IOMMU, then program the device.
    pub fn map_with_cap(
        &mut self,
        cap: &Capability,
        req: MapRequest,
    ) -> Result<PhysAddr, HalError> {
        let region = self.iommu.map(cap, req).map_err(|e| match e {
            aether_core::iommu::MapError::NoMemoryCap => HalError::NoMemoryCap,
            aether_core::iommu::MapError::BadRange | aether_core::iommu::MapError::Overlap => {
                HalError::BadArg
            }
            aether_core::iommu::MapError::TableFull => HalError::Busy,
            _ => HalError::Fault,
        })?;
        Ok(region.iova)
    }

    pub fn doorbell_pending(&self) -> bool {
        self.mmio.doorbell_pending()
    }

    pub fn irq_pending(&self) -> bool {
        self.mmio.irq_pending()
    }

    /// Device-side: drain one kicked job into SoftNPU and raise used-ring IRQ.
    pub fn service(&mut self) -> Option<Completion> {
        let (token, job) = self.mmio.device_take_avail()?;
        if !self.buffers_mapped(&job) {
            let cpl = Completion {
                job_seq: self.npu.seq,
                status: -2,
                cycles: 0,
            };
            self.mmio.device_complete(token, cpl);
            return Some(cpl);
        }
        match self.npu.execute(&job, &mut self.mem) {
            Ok(cpl) => {
                self.mmio.device_complete(token, cpl);
                Some(cpl)
            }
            Err(_) => {
                let cpl = Completion {
                    job_seq: self.npu.seq,
                    status: -1,
                    cycles: 0,
                };
                self.mmio.device_complete(token, cpl);
                Some(cpl)
            }
        }
    }

    fn buffers_mapped(&self, job: &AccelJobDesc) -> bool {
        if self.iommu.is_empty() {
            // Host unit tests that only exercise the ring may skip pin.
            return true;
        }
        let a_bytes = 4u64.saturating_mul(job.elems_a() as u64);
        let b_bytes = 4u64.saturating_mul(job.elems_b() as u64);
        let c_bytes = 4u64.saturating_mul(job.elems_c() as u64);
        self.iommu.covers(job.a, a_bytes.max(4))
            && self.iommu.covers(job.b, b_bytes.max(4))
            && self.iommu.covers(job.c, c_bytes.max(4))
    }
}

impl<M: DmaView> AccelDevice for SoftNpuDevice<M> {
    fn probe(&mut self) -> Result<AccelInfo, HalError> {
        if !self.mmio.negotiate() {
            return Err(HalError::NotFound);
        }
        Ok(self.info)
    }

    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError> {
        self.mmio.driver_submit(job).map_err(|_| HalError::Busy)
    }

    fn poll(&mut self) -> Option<Completion> {
        self.mmio.driver_poll_used()
    }

    fn map(&mut self, req: MapRequest) -> Result<PhysAddr, HalError> {
        // Device pin without a cap is refused. Use [`Self::map_with_cap`].
        let _ = req;
        Err(HalError::NoMemoryCap)
    }

    fn unmap(&mut self, iova: PhysAddr) -> Result<(), HalError> {
        self.iommu.unmap(iova).map(|_| ()).map_err(|_| HalError::Fault)
    }

    fn translate(&self, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.iommu.translate(guest_pa)
    }

    fn name(&self) -> &'static str {
        "softnpu+virtio-accel-mmio"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::accel::SliceMem;
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::types::TenantId;
    use aether_hal::AccelDevice;

    fn mem_cap() -> Capability {
        Capability {
            kind: CapKind::Memory,
            rights: CapRights::MEM_FULL,
            object: 1,
            badge: 0,
            generation: 1,
            tenant: TenantId(1),
        }
    }

    #[test]
    fn submit_kicks_doorbell_service_raises_irq() {
        let mut backing = [0u8; 256];
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            backing[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut dev = SoftNpuDevice::new(mem);
        assert!(dev.probe().is_ok());
        assert_eq!(dev.map(MapRequest::pin(PhysAddr(0), 256)).unwrap_err(), HalError::NoMemoryCap);
        let iova = dev
            .map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        assert_eq!(iova.0, 0);
        assert_eq!(dev.translate(PhysAddr(16)).unwrap().0, 16);

        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        dev.submit(&job).unwrap();
        assert!(dev.doorbell_pending());
        assert!(dev.poll().is_none(), "completions come from used ring / IRQ");

        let serviced = dev.service().unwrap();
        assert_eq!(serviced.status, 0);
        assert!(dev.irq_pending());
        let c = dev.poll().unwrap();
        assert_eq!(c.status, 0);
        assert!(!dev.irq_pending());
        let out0 = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(out0, 19);
    }
}
