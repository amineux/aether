//! Software NPU tile bound to the VirtIO-Accel MMIO virtqueue.
//!
//! SpecForge Y1H1 path B: this is the canonical demo behind the
//! in-kernel BAR. The kernel driver talks the MMIO + virtqueue ABI
//! (`submit` kicks the doorbell; `poll` reads the used ring). SoftNPU
//! is the device-side executor: [`SoftNpuDevice::service`] drains the
//! avail ring on poll/IRQ. No custom QEMU device is required.

use aether_core::accel::{AccelError, AccelJobDesc, Completion, DmaView, SoftNpu};
use aether_core::caps::Capability;
use aether_core::fence::{Fence, FenceId, Timeline};
use aether_core::iommu::{IommuMap, MapError, MapRequest, DEFAULT_STREAM};
use aether_core::partition::PartitionError;
use aether_core::types::PhysAddr;
use aether_hal::{AccelDevice, AccelInfo, HalError, ACCEL_BACKEND_VIRTIO_SOFTNPU};

use crate::mmio::AccelMmio;

/// Kernel CPU view of guest RAM after Soft SMMU resolve.
///
/// On x86 this is the higher-half alias (`KERNEL_VMA + PA`), not the
/// trampoline identity map. RISC-V / aarch64 still use PA=VA (their
/// kernel map *is* identity). SoftNPU must resolve IOVAs first —
/// this view never treats a Soft-SMMU IOVA as a load address.
pub struct KernelDma;

/// PA=VA DMA. Host tests and RISC-V / aarch64 kernel maps.
/// x86 SoftNPU uses [`KernelDma`] (HH) so identity 4 GiB can come down.
pub struct IdentityDma;

fn dma_kva(pa: u64) -> Result<u64, AccelError> {
    #[cfg(target_arch = "x86_64")]
    {
        aether_core::phys_to_kva(pa).ok_or(AccelError::Overflow)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Ok(pa)
    }
}

macro_rules! impl_pa_dma {
    ($ty:ty, $va:ident) => {
        impl DmaView for $ty {
            fn load_i32(&self, addr: PhysAddr) -> Result<i32, AccelError> {
                let p = $va(addr.0)? as *const i32;
                // SAFETY: caller mapped + owns the arena (cap + Soft SMMU).
                Ok(unsafe { core::ptr::read_volatile(p) })
            }
            fn store_i32(&mut self, addr: PhysAddr, val: i32) -> Result<(), AccelError> {
                let p = $va(addr.0)? as *mut i32;
                unsafe { core::ptr::write_volatile(p, val) };
                Ok(())
            }
            fn load_u16(&self, addr: PhysAddr) -> Result<u16, AccelError> {
                let p = $va(addr.0)? as *const u16;
                Ok(unsafe { core::ptr::read_volatile(p) })
            }
            fn store_u16(&mut self, addr: PhysAddr, val: u16) -> Result<(), AccelError> {
                let p = $va(addr.0)? as *mut u16;
                unsafe { core::ptr::write_volatile(p, val) };
                Ok(())
            }
        }
    };
}

fn identity_va(pa: u64) -> Result<u64, AccelError> {
    Ok(pa)
}

impl_pa_dma!(KernelDma, dma_kva);
impl_pa_dma!(IdentityDma, identity_va);

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
    last_fence: Option<u64>,
    fence_done: bool,
}

impl<M: DmaView> SoftNpuDevice<M> {
    pub fn new(mem: M) -> Self {
        Self {
            info: AccelInfo {
                vendor: 0xAE7E,
                device: 0x0001,
                n_queues: 1,
                max_wave: 64,
                backend: ACCEL_BACKEND_VIRTIO_SOFTNPU,
            },
            mmio: AccelMmio::new(),
            npu: SoftNpu::new(),
            mem,
            iommu: IommuMap::new(),
            last_fence: None,
            fence_done: false,
        }
    }

    /// Fence id retired by the last used-ring IRQ, if the job named one.
    pub fn completed_fence(&self) -> Option<u64> {
        if self.fence_done {
            self.last_fence
        } else {
            None
        }
    }

    /// Retire the IRQ seq into a CP-shaped [`Timeline`].
    pub fn retire_into(&self, timeline: &mut Timeline) -> Result<Option<Fence>, PartitionError> {
        match self.completed_fence() {
            Some(id) => timeline.complete(FenceId(id)).map(Some),
            None => Ok(None),
        }
    }

    /// Authorize + pin through the software IOMMU, then program the device.
    pub fn map_with_cap(
        &mut self,
        cap: &Capability,
        req: MapRequest,
    ) -> Result<PhysAddr, HalError> {
        let region = self.iommu.map(cap, req).map_err(map_hal_error)?;
        Ok(region.iova)
    }

    /// IOVA → guest PA only. No guest-PA identity shortcut when Soft SMMU
    /// is populated — tensors must arrive as Soft-SMMU IOVAs.
    fn dma_guest_pa(&self, addr: PhysAddr) -> Option<PhysAddr> {
        if self.iommu.is_empty() {
            return Some(addr);
        }
        self.iommu
            .resolve_stream(DEFAULT_STREAM, addr)
            .or_else(|| self.iommu.resolve(addr))
    }

    fn job_to_iova(&self, job: &AccelJobDesc) -> AccelJobDesc {
        let mut wired = *job;
        if self.iommu.is_empty() {
            return wired;
        }
        if let Some(a) = self.iommu.translate_stream(DEFAULT_STREAM, job.a) {
            wired.a = a;
        }
        if let Some(b) = self.iommu.translate_stream(DEFAULT_STREAM, job.b) {
            wired.b = b;
        }
        if let Some(c) = self.iommu.translate_stream(DEFAULT_STREAM, job.c) {
            wired.c = c;
        }
        if job.bias.0 != 0 {
            if let Some(bias) = self.iommu.translate_stream(DEFAULT_STREAM, job.bias) {
                wired.bias = bias;
            }
        }
        wired
    }

    fn job_from_iova(&self, job: AccelJobDesc) -> Option<AccelJobDesc> {
        if self.iommu.is_empty() {
            return Some(job);
        }
        let mut pa = job;
        pa.a = self.dma_guest_pa(job.a)?;
        pa.b = self.dma_guest_pa(job.b)?;
        pa.c = self.dma_guest_pa(job.c)?;
        if job.bias.0 != 0 {
            pa.bias = self.dma_guest_pa(job.bias)?;
        }
        Some(pa)
    }

    pub fn doorbell_pending(&self) -> bool {
        self.mmio.doorbell_pending()
    }

    pub fn irq_pending(&self) -> bool {
        self.mmio.irq_pending()
    }

    /// Device-side: drain one kicked job into SoftNPU and raise used-ring IRQ.
    /// Avail-ring addresses are Soft-SMMU IOVAs; resolve to guest PA for DMA.
    pub fn service(&mut self) -> Option<Completion> {
        let (token, job) = self.mmio.device_take_avail()?;
        let fence_id = job.fence_id;
        self.fence_done = false;
        self.last_fence = if fence_id != 0 { Some(fence_id) } else { None };
        if !self.buffers_mapped(&job) {
            let cpl = Completion {
                job_seq: self.npu.seq,
                status: -2,
                cycles: 0,
            };
            self.mmio.device_complete(token, cpl);
            return Some(self.note_fence(fence_id, cpl));
        }
        let Some(job) = self.job_from_iova(job) else {
            let cpl = Completion {
                job_seq: self.npu.seq,
                status: -2,
                cycles: 0,
            };
            self.mmio.device_complete(token, cpl);
            return Some(self.note_fence(fence_id, cpl));
        };
        match self.npu.execute(&job, &mut self.mem) {
            Ok(cpl) => {
                self.mmio.device_complete(token, cpl);
                Some(self.note_fence(job.fence_id, cpl))
            }
            Err(_) => {
                let cpl = Completion {
                    job_seq: self.npu.seq,
                    status: -1,
                    cycles: 0,
                };
                self.mmio.device_complete(token, cpl);
                Some(self.note_fence(job.fence_id, cpl))
            }
        }
    }

    fn note_fence(&mut self, fence_id: u64, cpl: Completion) -> Completion {
        if fence_id != 0 {
            self.last_fence = Some(fence_id);
            self.fence_done = true;
        }
        cpl
    }

    fn buffers_mapped(&self, job: &AccelJobDesc) -> bool {
        if self.iommu.is_empty() {
            // Host unit tests that only exercise the ring may skip pin.
            return true;
        }
        let es = job.elem_bytes().max(1);
        let a_bytes = job.bytes_a();
        let b_bytes = job.bytes_b();
        let c_bytes = job.bytes_c();
        self.dma_range_ok(job.a, a_bytes.max(es))
            && self.dma_range_ok(job.b, b_bytes.max(es))
            && self.dma_range_ok(job.c, c_bytes.max(es))
    }

    fn dma_range_ok(&self, addr: PhysAddr, len: u64) -> bool {
        self.iommu.covers_iova(DEFAULT_STREAM, addr, len) || self.iommu.covers_iova_any(addr, len)
    }
}

fn map_hal_error(e: MapError) -> HalError {
    match e {
        MapError::NoMemoryCap => HalError::NoMemoryCap,
        MapError::BadRange | MapError::Overlap => HalError::BadArg,
        MapError::TableFull => HalError::Busy,
        MapError::NotMapped
        | MapError::CrossTenant
        | MapError::WrongStream
        | MapError::StreamAbort
        | MapError::Stage2Fault => HalError::Fault,
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
        let wired = self.job_to_iova(job);
        self.fence_done = false;
        self.last_fence = if job.fence_id != 0 {
            Some(job.fence_id)
        } else {
            None
        };
        self.mmio.driver_submit(&wired).map_err(|_| HalError::Busy)
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
        self.iommu
            .unmap(iova)
            .map(|_| ())
            .map_err(|_| HalError::Fault)
    }

    fn translate(&self, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.iommu.translate(guest_pa)
    }

    fn translate_stream(&self, stream_id: u32, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.iommu.translate_stream(stream_id, guest_pa)
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
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1)).with_generation(1)
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
        assert_eq!(
            dev.map(MapRequest::pin(PhysAddr(0), 256)).unwrap_err(),
            HalError::NoMemoryCap
        );
        let iova = dev
            .map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        assert_ne!(iova.0, 0, "Soft SMMU IOVA is not identity");
        assert_eq!(dev.translate(PhysAddr(16)).unwrap().0, iova.0 + 16);
        assert_eq!(dev.iommu.resolve(PhysAddr(iova.0 + 16)).unwrap().0, 16);

        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        dev.submit(&job).unwrap();
        // Avail ring carries Soft-SMMU IOVAs, not guest PAs.
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&dev.mmio.as_bytes()[0x80 + 24..0x80 + 32]);
        assert_eq!(u64::from_le_bytes(raw), iova.0);
        assert!(dev.doorbell_pending());
        assert!(
            dev.poll().is_none(),
            "completions come from used ring / IRQ"
        );

        let serviced = dev.service().unwrap();
        assert_eq!(serviced.status, 0);
        assert!(dev.irq_pending());
        let c = dev.poll().unwrap();
        assert_eq!(c.status, 0);
        assert!(!dev.irq_pending());
        let out0 = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(out0, 19);
    }

    #[test]
    fn service_refuses_guest_pa_when_smmu_bound() {
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
        let _ = dev
            .map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        dev.submit(&job).unwrap();
        // Guest PA in the avail ring is not a Soft-SMMU IOVA.
        dev.mmio.poke_avail_a(0, 0);
        let serviced = dev.service().unwrap();
        assert_eq!(serviced.status, -2);
    }

    #[test]
    fn golden_mmio_trace_softnpu_submit_complete() {
        use crate::mmio::GOLDEN_SOFTNPU_SUBMIT_COMPLETE;

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
        // Map does not poke the BAR; leave extra observers out of the
        // golden so the sequence is probe → submit → service → poll
        // against the frozen layout.
        assert!(dev.probe().is_ok());
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        dev.submit(&job).unwrap();
        let serviced = dev.service().unwrap();
        assert_eq!(serviced.status, 0);
        assert_eq!(serviced.job_seq, 1);
        let c = dev.poll().unwrap();
        assert_eq!(c.status, 0);
        assert_eq!(c.cycles, 8);
        assert_eq!(
            dev.mmio.golden_cfg_trace().as_slice(),
            GOLDEN_SOFTNPU_SUBMIT_COMPLETE
        );
        let out0 = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(out0, 19);
    }

    #[test]
    fn used_ring_retires_partition_fence() {
        use aether_core::partition::{
            BlastRadius, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
        };
        use aether_core::types::ChipletId;

        let mut backing = [0u8; 256];
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            backing[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let part = PartitionProfile::new(
            PartitionId(1),
            SpatialSlice::single_chiplet(ChipletId(0), 0b1, 0b1),
            QosBudget {
                bw_mbps: 100,
                credits: 2,
            },
            BlastRadius {
                max_nodes: 2,
                max_hops: 1,
            },
        );
        let mut timeline = Timeline::new(PartitionId(1));
        let fence = timeline.submit(&part, None).unwrap();
        let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job.fence_id = fence.id.0;

        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut dev = SoftNpuDevice::new(mem);
        dev.map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        dev.submit(&job).unwrap();
        assert!(dev.completed_fence().is_none());
        dev.service().unwrap();
        assert_eq!(dev.completed_fence(), Some(fence.id.0));
        let done = dev.retire_into(&mut timeline).unwrap().unwrap();
        assert!(done.completed);
        assert!(timeline.wait(fence.id).unwrap().completed);
        assert_eq!(timeline.in_flight(), 0);
    }

    #[test]
    fn invalidate_does_not_break_softnpu_resolve() {
        use aether_core::iommu::InvCmd;

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
        let iova = dev
            .map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        let _ = dev.iommu.invalidate(InvCmd::All).unwrap();
        assert_eq!(dev.iommu.resolve(iova).unwrap().0, 0);
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        dev.submit(&job).unwrap();
        let serviced = dev.service().unwrap();
        assert_eq!(serviced.status, 0);
        let out0 = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(out0, 19);
    }
}
