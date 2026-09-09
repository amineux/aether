//! Path A Soft-SMMU IOVA contract (host).
//!
//! **Why this is path A, not path B.** Path B (`SoftNpuDevice`) is the
//! in-kernel `AccelMmio` array stock `make qemu` boots: the kernel CPU
//! takes the avail ring and SoftNPU loads guest PA on SoftNPU ssid 0
//! (`aether_core::iommu::DEFAULT_STREAM`). Path A is the QEMU `-device aether-accel`
//! PCI BAR (`qemu/aether_accel.c`, vendor `0xAE7E` / device `0xACC1`).
//! The **device** is the DMA initiator: it DMA-reads tensor addresses
//! from the frozen 88-byte [`AccelJobWire`]. This module proves those
//! fields are Soft-SMMU IOVAs (not identity GPAs), DMA walks
//! STE→CD→S1→S2 on a path-A [`StreamId`], and a wrong SID aborts.
//!
//! Soft SMMU stays software. This is not a QEMU IOMMU and not a guest
//! PCI bind — a kernel `VirtioAccelMmio` that talks BAR0 is still
//! larger than this gated digest. Stock `make qemu` is unchanged
//! (path B). CI does not rebuild QEMU. `make qemu-accel` stays optional.
//!
//! No new syscall. No NVIDIA / FLOP claim.

use aether_core::accel::{AccelJobDesc, AccelOp, Completion, DmaView, SoftNpu};
use aether_core::caps::Capability;
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId, StreamState};
use aether_core::types::{ChipletId, PhysAddr, TileId};
use aether_hal::{AccelDevice, AccelInfo, HalError, ACCEL_BACKEND_VIRTIO_SOFTNPU};

use crate::mmio::{AccelJobWire, AccelMmio};

/// Path-A PCI BAR substream. Distinct from SoftNPU (`ssid` 0), Soft-CP (1),
/// and IreeShapedCp (2). Same STE as stream 0 (chiplet 0 / tile 0).
pub const PATH_A_SSID: u8 = 4;

/// Packed path-A SID: chiplet 0, tile 0, [`PATH_A_SSID`]. Not a PCIe BDF.
pub const fn path_a_sid() -> StreamId {
    StreamId::accel(ChipletId(0), TileId(0), PATH_A_SSID)
}

/// PCI device id published by `qemu/aether_accel_pci.c` (`AETHER_PCI_DEVICE`).
pub const PATH_A_PCI_DEVICE: u32 = 0xACC1;

/// Path-A BAR + Soft-SMMU DMA. Host contract for the portable device model.
///
/// Stock `make qemu` does not attach this. SoftNPU /init stays path B.
pub struct PathABar<M: DmaView> {
    pub info: AccelInfo,
    pub mmio: AccelMmio,
    pub npu: SoftNpu,
    pub mem: M,
    pub iommu: IommuMap,
    /// SID this PCI function is bound to (path-A stream).
    sid: StreamId,
    /// SID used on the DMA walk. Tests may restamp to inject a wrong SID.
    dma_sid: StreamId,
}

impl<M: DmaView> PathABar<M> {
    pub fn new(mem: M) -> Self {
        let sid = path_a_sid();
        Self {
            info: AccelInfo {
                vendor: 0xAE7E,
                device: PATH_A_PCI_DEVICE,
                n_queues: 1,
                max_wave: 64,
                backend: ACCEL_BACKEND_VIRTIO_SOFTNPU,
                sm_count: 0,
                wq_count: 0,
            },
            mmio: AccelMmio::new(),
            npu: SoftNpu::new(),
            mem,
            iommu: IommuMap::new(),
            sid,
            dma_sid: sid,
        }
    }

    pub fn stream_id(&self) -> StreamId {
        self.sid
    }

    /// Bind the path-A PCI stream. Capture alone leaves DMA aborting.
    pub fn bind_stream(&mut self, cap: &Capability) -> Result<StreamState, HalError> {
        self.iommu.bind_stream(cap, self.sid).map_err(map_hal_error)
    }

    /// Authorize + pin on the path-A SID. IOVA is not identity.
    pub fn map_with_cap(
        &mut self,
        cap: &Capability,
        req: MapRequest,
    ) -> Result<PhysAddr, HalError> {
        let mut req = req;
        req.stream_id = self.sid.raw();
        let region = self.iommu.map(cap, req).map_err(map_hal_error)?;
        Ok(region.iova)
    }

    /// Fault injection: next `service` DMA walks `sid` instead of the
    /// bound path-A SID. Not a public submit path.
    pub fn inject_dma_sid(&mut self, sid: StreamId) {
        self.dma_sid = sid;
    }

    pub fn doorbell_pending(&self) -> bool {
        self.mmio.doorbell_pending()
    }

    pub fn irq_pending(&self) -> bool {
        self.mmio.irq_pending()
    }

    /// Packed avail-ring job the path-A device would DMA-read.
    pub fn peek_job_wire(&self, slot: usize) -> AccelJobWire {
        self.mmio.peek_job_wire(slot)
    }

    /// Device-side: drain one kicked job. Avail-ring addresses are
    /// Soft-SMMU IOVAs on [`Self::stream_id`]; resolve to guest PA for DMA.
    /// Wrong SID / unbound / identity GPA → used-ring status `-2`.
    pub fn service(&mut self) -> Option<Completion> {
        let (token, job) = self.mmio.device_take_avail()?;
        if job.op != AccelOp::Nop && !self.buffers_mapped(&job) {
            return Some(self.complete_dma_fault(token));
        }
        let job = match self.job_from_iova(job) {
            Ok(j) => j,
            Err(_) => return Some(self.complete_dma_fault(token)),
        };
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

    fn complete_dma_fault(&mut self, token: u32) -> Completion {
        let cpl = Completion {
            job_seq: self.npu.seq,
            status: -2,
            cycles: 0,
        };
        self.mmio.device_complete(token, cpl);
        cpl
    }

    fn job_to_iova(&self, job: &AccelJobDesc) -> Result<AccelJobDesc, MapError> {
        if self.iommu.is_empty() {
            return Err(MapError::StreamAbort);
        }
        let sid = self.sid.raw();
        let mut wired = *job;
        wired.flags = sid;
        wired.a = self.iommu.translate_result(sid, job.a, None)?;
        wired.b = self.iommu.translate_result(sid, job.b, None)?;
        wired.c = self.iommu.translate_result(sid, job.c, None)?;
        if job.bias.0 != 0 {
            wired.bias = self.iommu.translate_result(sid, job.bias, None)?;
        }
        Ok(wired)
    }

    fn job_from_iova(&self, job: AccelJobDesc) -> Result<AccelJobDesc, MapError> {
        if job.op == AccelOp::Nop {
            return Ok(job);
        }
        let sid = self.dma_sid.raw();
        let mut pa = job;
        pa.a = self.iommu.resolve_result(sid, job.a, None)?;
        pa.b = self.iommu.resolve_result(sid, job.b, None)?;
        pa.c = self.iommu.resolve_result(sid, job.c, None)?;
        if job.bias.0 != 0 {
            pa.bias = self.iommu.resolve_result(sid, job.bias, None)?;
        }
        Ok(pa)
    }

    fn buffers_mapped(&self, job: &AccelJobDesc) -> bool {
        if self.iommu.is_empty() {
            return false;
        }
        let es = job.elem_bytes().max(1);
        let sid = self.dma_sid.raw();
        self.iommu.covers_iova(sid, job.a, job.bytes_a().max(es))
            && self.iommu.covers_iova(sid, job.b, job.bytes_b().max(es))
            && self.iommu.covers_iova(sid, job.c, job.bytes_c().max(es))
    }
}

fn map_hal_error(e: MapError) -> HalError {
    match e {
        MapError::NoMemoryCap => HalError::NoMemoryCap,
        MapError::BadRange | MapError::Overlap => HalError::BadArg,
        MapError::TableFull | MapError::SidBudget => HalError::Busy,
        MapError::NotMapped
        | MapError::CrossTenant
        | MapError::WrongStream
        | MapError::StreamAbort
        | MapError::Stage2Fault
        | MapError::SubmitSid => HalError::Fault,
    }
}

impl<M: DmaView> AccelDevice for PathABar<M> {
    fn probe(&mut self) -> Result<AccelInfo, HalError> {
        if !self.mmio.negotiate() {
            return Err(HalError::NotFound);
        }
        Ok(self.info)
    }

    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError> {
        let wired = self.job_to_iova(job).map_err(map_hal_error)?;
        self.mmio.driver_submit(&wired).map_err(|_| HalError::Busy)
    }

    fn poll(&mut self) -> Option<Completion> {
        self.mmio.driver_poll_used()
    }

    fn map(&mut self, req: MapRequest) -> Result<PhysAddr, HalError> {
        let _ = req;
        Err(HalError::NoMemoryCap)
    }

    fn unmap(&mut self, iova: PhysAddr) -> Result<(), HalError> {
        self.iommu
            .unmap_stream(self.sid.raw(), iova)
            .map(|_| ())
            .map_err(map_hal_error)
    }

    fn translate(&self, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.iommu.translate_stream(self.sid.raw(), guest_pa)
    }

    fn translate_stream(&self, stream_id: u32, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.iommu.translate_stream(stream_id, guest_pa)
    }

    fn name(&self) -> &'static str {
        "path-a-pci-bar+soft-smmu"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::accel::SliceMem;
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::iommu::{DEFAULT_STREAM, SOFT_SMMU_IOVA_BASE};
    use aether_core::types::TenantId;
    use aether_hal::AccelDevice;

    fn mem_cap() -> Capability {
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1)).with_generation(1)
    }

    fn fill_2x2(backing: &mut [u8]) {
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            backing[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    fn path_a_dev(backing: &mut [u8]) -> PathABar<SliceMem<'_>> {
        fill_2x2(backing);
        PathABar::new(SliceMem {
            base: PhysAddr(0),
            bytes: backing,
        })
    }

    #[test]
    fn why_this_is_path_a_not_path_b() {
        let mut backing = [0u8; 256];
        let d = path_a_dev(&mut backing);
        assert_eq!(
            d.name(),
            "path-a-pci-bar+soft-smmu",
            "path A is the PCI BAR DMA initiator, not in-kernel SoftNpuDevice"
        );
        assert_eq!(d.info.device, PATH_A_PCI_DEVICE);
        assert_ne!(
            d.info.device, 0x0001,
            "path B SoftNpuDevice publishes 0x0001"
        );
        assert_eq!(d.stream_id().ssid(), PATH_A_SSID);
        assert_ne!(
            d.stream_id().raw(),
            DEFAULT_STREAM,
            "path A is not SoftNPU ssid 0"
        );
        assert_eq!(d.info.backend, ACCEL_BACKEND_VIRTIO_SOFTNPU);
        assert_eq!(d.info.vendor, 0xAE7E);
    }

    #[test]
    fn path_a_job_wire_carries_iova_not_gpa() {
        let mut backing = [0u8; 256];
        let mut d = path_a_dev(&mut backing);
        assert!(d.probe().is_ok());
        assert_eq!(
            d.map(MapRequest::pin(PhysAddr(0), 256)).unwrap_err(),
            HalError::NoMemoryCap
        );
        let iova = d
            .map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        assert_ne!(iova.0, 0, "Soft SMMU IOVA is not identity");
        assert!(iova.0 >= SOFT_SMMU_IOVA_BASE);
        assert_ne!(iova.0, 0x0);
        assert_eq!(d.translate(PhysAddr(16)).unwrap().0, iova.0 + 16);

        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        d.submit(&job).unwrap();
        let wire = d.peek_job_wire(0);
        assert_eq!(wire.a, iova.0, "path-A job wire carries IOVA A, not GPA");
        assert_eq!(wire.b, iova.0 + 16);
        assert_eq!(wire.c, iova.0 + 32);
        assert_ne!(wire.a, 0);
        assert_ne!(wire.b, 16);
        assert_ne!(wire.c, 32);
        assert_eq!(
            wire.flags,
            path_a_sid().raw(),
            "flags stamp the path-A stream_id without growing the frozen 88-byte wire"
        );
        assert!(d.doorbell_pending());
        assert!(d.poll().is_none(), "completions come from used ring / IRQ");
    }

    #[test]
    fn path_a_bind_translate_matmul_and_wrong_sid_abort() {
        let mut backing = [0u8; 256];
        let mut d = path_a_dev(&mut backing);
        assert!(d.probe().is_ok());
        let cap = mem_cap();
        assert_eq!(d.bind_stream(&cap).unwrap(), StreamState::Bound);
        let iova = d
            .map_with_cap(&cap, MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        let sid = path_a_sid();
        assert_eq!(d.iommu.walk(sid, iova).unwrap().pa.0, 0);
        assert_eq!(
            d.iommu
                .translate_result(sid.raw(), PhysAddr(0), None)
                .unwrap()
                .0,
            iova.0
        );

        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        d.submit(&job).unwrap();
        let serviced = d.service().unwrap();
        assert_eq!(serviced.status, 0);
        assert!(d.irq_pending());
        let c = d.poll().unwrap();
        assert_eq!(c.status, 0);
        let out0 = d.mem.load_i32(PhysAddr(32)).unwrap();
        assert_eq!(out0, 19);

        // Same IOVA on the in-kernel path-B stream (ssid 0) is WrongStream
        // once that SSID is Bound — this is not identity DMA.
        let path_b = StreamId::from_raw(DEFAULT_STREAM);
        d.iommu.bind_stream(&cap, path_b).unwrap();
        assert_eq!(
            d.iommu.resolve_result(path_b.raw(), iova, None),
            Err(MapError::WrongStream)
        );
        assert_eq!(
            d.iommu.translate_result(path_b.raw(), PhysAddr(0), None),
            Err(MapError::WrongStream)
        );
    }

    #[test]
    fn path_a_unbound_sid_aborts_dma() {
        let mut backing = [0u8; 256];
        let mut d = path_a_dev(&mut backing);
        let cap = mem_cap();
        let sid = path_a_sid();
        assert_eq!(d.iommu.stream_state(sid), StreamState::Unbound);
        assert_eq!(
            d.iommu.translate_result(sid.raw(), PhysAddr(0), None),
            Err(MapError::StreamAbort)
        );
        assert_eq!(d.iommu.capture(sid).unwrap(), StreamState::Captured);
        assert_eq!(
            d.iommu.translate_result(sid.raw(), PhysAddr(0), None),
            Err(MapError::StreamAbort),
            "capture does not authorize path-A DMA"
        );
        assert_eq!(d.bind_stream(&cap).unwrap(), StreamState::Bound);
        assert_eq!(
            d.iommu.translate_result(sid.raw(), PhysAddr(0), None),
            Err(MapError::NotMapped)
        );
    }

    #[test]
    fn path_a_wrong_sid_dma_aborts_on_service() {
        let mut backing = [0u8; 256];
        let mut d = path_a_dev(&mut backing);
        assert!(d.probe().is_ok());
        let cap = mem_cap();
        let _iova = d
            .map_with_cap(&cap, MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        d.submit(&job).unwrap();
        // Bind the in-kernel path-B SID so DMA is WrongStream, not Unbound.
        d.iommu
            .bind_stream(&cap, StreamId::from_raw(DEFAULT_STREAM))
            .unwrap();
        d.inject_dma_sid(StreamId::from_raw(DEFAULT_STREAM));
        let serviced = d.service().unwrap();
        assert_eq!(serviced.status, -2, "wrong-SID path-A DMA must abort");
        let out0 = d.mem.load_i32(PhysAddr(32)).unwrap();
        assert_eq!(out0, 0, "wrong SID must not write C");
    }

    #[test]
    fn path_a_identity_gpa_in_wire_refused() {
        let mut backing = [0u8; 256];
        let mut d = path_a_dev(&mut backing);
        assert!(d.probe().is_ok());
        let _iova = d
            .map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        d.submit(&job).unwrap();
        // Guest PA in the avail ring is not a Soft-SMMU IOVA.
        d.mmio.poke_avail_a(0, 0);
        let serviced = d.service().unwrap();
        assert_eq!(serviced.status, -2);
    }
}
