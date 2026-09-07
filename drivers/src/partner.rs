//! Sketch of a second `AccelDevice` for a silicon partner.
//!
//! This is **not** a partnership, a working NPU, or a performance claim.
//! It shows where opcode / dtype / route fields land in a command
//! packet, and leaves every hardware step as an explicit TODO.

use aether_core::accel::{AccelJobDesc, AccelOp, Completion, DType};
use aether_core::caps::Capability;
use aether_core::iommu::{IommuMap, MapRequest};
use aether_core::space::Place;
use aether_core::types::PhysAddr;
use aether_hal::{AccelDevice, AccelInfo, HalError};

/// Command packet a partner command processor would ingest.
///
/// Fill these from [`AccelJobDesc`] — do not invent a second IR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartnerCmd {
    pub opcode: u32,
    pub dtype: u8,
    pub route_chiplet: u8,
    pub route_tile: u16,
    pub iova_a: u64,
    pub iova_b: u64,
    pub iova_c: u64,
    pub m: u32,
    pub n: u32,
    pub k: u32,
}

impl PartnerCmd {
    /// Translate an Aether job + IOVA map into the partner packet.
    pub fn from_job(job: &AccelJobDesc, iommu: &IommuMap) -> Result<Self, HalError> {
        let a = iommu.translate(job.a).ok_or(HalError::Fault)?;
        let b = iommu.translate(job.b).ok_or(HalError::Fault)?;
        let c = iommu.translate(job.c).ok_or(HalError::Fault)?;
        Ok(Self {
            opcode: job.op as u32,
            dtype: job.dtype as u8,
            route_chiplet: job.place.chiplet.0,
            route_tile: job.place.tile.unwrap_or(0),
            iova_a: a.0,
            iova_b: b.0,
            iova_c: c.0,
            m: job.m,
            n: job.n,
            k: job.k,
        })
    }
}

/// No-op silicon sketch. Probe reports `backend = 2`.
pub struct PartnerNpuStub {
    pub info: AccelInfo,
    pub iommu: IommuMap,
    last_cmd: Option<PartnerCmd>,
    last_cpl: Option<Completion>,
    seq: u32,
}

impl PartnerNpuStub {
    pub fn new() -> Self {
        Self {
            info: AccelInfo {
                vendor: 0xFFFF_0000, // placeholder — not a real vendor id
                device: 0x0001,
                n_queues: 1,
                max_wave: 0, // TODO: read from PCI/MMIO cfg
                backend: 2,
                sm_count: 0,
                wq_count: 0,
            },
            iommu: IommuMap::new(),
            last_cmd: None,
            last_cpl: None,
            seq: 1,
        }
    }

    pub fn last_cmd(&self) -> Option<PartnerCmd> {
        self.last_cmd
    }

    pub fn map_with_cap(
        &mut self,
        cap: &Capability,
        req: MapRequest,
    ) -> Result<PhysAddr, HalError> {
        // Soft SMMU records `req.stream_id`. Hardware SMMU SID / PT is TODO.
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

    fn encode(job: &AccelJobDesc) -> u32 {
        // TODO: pack partner-specific opcode bits. v0.1 just mirrors AccelOp.
        let _ = (DType::I32, Place::new);
        job.op as u32
    }
}

impl Default for PartnerNpuStub {
    fn default() -> Self {
        Self::new()
    }
}

impl AccelDevice for PartnerNpuStub {
    fn probe(&mut self) -> Result<AccelInfo, HalError> {
        // TODO: PCI BAR / platform MMIO probe; check magic / version.
        Ok(self.info)
    }

    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError> {
        // TODO: DMA the command packet; ring the partner doorbell.
        let cmd = PartnerCmd::from_job(job, &self.iommu)?;
        let _opcode = Self::encode(job);
        self.last_cmd = Some(cmd);
        let token = self.seq;
        self.seq += 1;
        // No-op complete: Nop always "succeeds". Real work is TODO.
        let status = if job.op == AccelOp::Nop { 0 } else { 0 };
        self.last_cpl = Some(Completion {
            job_seq: token,
            status,
            cycles: 0,
        });
        Ok(token)
    }

    fn poll(&mut self) -> Option<Completion> {
        // TODO: MSI-X / completion-queue pop; then fabric.send(REPLY).
        self.last_cpl.take()
    }

    fn map(&mut self, _req: MapRequest) -> Result<PhysAddr, HalError> {
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
        "partner-npu-stub"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::caps::{CapKind, CapRights};
    use aether_core::types::TenantId;
    use aether_hal::AccelDevice;

    fn mem_cap() -> Capability {
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 9, TenantId(1)).with_generation(1)
    }

    #[test]
    fn probe_is_silicon_backend() {
        let mut d = PartnerNpuStub::new();
        let info = d.probe().unwrap();
        assert_eq!(info.backend, 2);
        assert_eq!(d.name(), "partner-npu-stub");
    }

    #[test]
    fn map_refuses_without_cap_then_translates() {
        let mut d = PartnerNpuStub::new();
        assert_eq!(
            d.map(MapRequest::pin(PhysAddr(0x1000), 0x1000))
                .unwrap_err(),
            HalError::NoMemoryCap
        );
        let iova = d
            .map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0x1000), 0x1000))
            .unwrap();
        assert_ne!(iova.0, 0x1000);
        assert_eq!(d.translate(PhysAddr(0x1400)).unwrap().0, iova.0 + 0x400);
    }

    #[test]
    fn submit_encodes_opcode_dtype_route() {
        let mut d = PartnerNpuStub::new();
        d.map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job.dtype = DType::I32;
        job.place = job.place.with_tile(4);
        d.submit(&job).unwrap();
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.opcode, AccelOp::MatMul as u32);
        assert_eq!(cmd.dtype, DType::I32 as u8);
        assert_eq!(cmd.route_tile, 4);
        assert_eq!(cmd.iova_a, d.translate(PhysAddr(0)).unwrap().0);
        assert_ne!(cmd.iova_a, 0);
        let cpl = d.poll().unwrap();
        assert_eq!(cpl.status, 0);
    }
}
