//! Software command processor (`SoftCommandProcessor`).
//!
//! Honest model of what a silicon CP would ingest: a fixed 64-byte
//! packet packed from [`AccelJobDesc`] (opcode / dtype / place / IOVAs /
//! shape / packed [`StreamId`] / fence). Not SoftNPU's virtqueue BAR, not
//! [`crate::PartnerNpuStub`]'s no-op complete-on-submit, and not a
//! vendor partnership.
//!
//! Uses the post-#7 Soft SMMU APIs:
//! ```text
//! StreamId::accel(chiplet, tile, CP_SSID)
//! bind_stream / map (Memory+MAP)     // DMA aborts until Bound
//! submit → pack CpCmd with per-SID IOVAs, doorbell (does not execute)
//! service (IRQ / kthread poll) → resolve_stream + SoftNPU math
//! poll → completion; caller retires the fence
//! ```

use aether_core::accel::{AccelJobDesc, AccelOp, Completion, DmaView, SoftNpu};
use aether_core::caps::Capability;
use aether_core::fence::{Fence, FenceId, Timeline};
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::partition::PartitionError;
use aether_core::types::{PhysAddr, TileId};
use aether_hal::{AccelDevice, AccelInfo, HalError, ACCEL_BACKEND_SOFT_CP};

/// Packet magic a CP mailbox would DMA (`AE7E` + command-processor `0C01`).
pub const CP_PKT_MAGIC: u32 = 0xAE7E_0C01;
pub const CP_CMD_SIZE: usize = 64;
pub const CP_FLAG_HAS_BIAS: u16 = 1 << 0;
/// Soft-CP substream. Distinct from SoftNPU's `DEFAULT_STREAM` (ssid 0).
pub const CP_SSID: u8 = 1;

/// Pack the CP stream from a job's fabric place.
pub fn stream_for_job(job: &AccelJobDesc) -> StreamId {
    StreamId::accel(
        job.place.chiplet,
        TileId(job.place.tile.unwrap_or(0)),
        CP_SSID,
    )
}

fn map_hal_error(e: MapError) -> HalError {
    match e {
        MapError::NoMemoryCap => HalError::NoMemoryCap,
        MapError::BadRange | MapError::Overlap => HalError::BadArg,
        MapError::TableFull => HalError::Busy,
        MapError::NotMapped
        | MapError::CrossTenant
        | MapError::WrongStream
        | MapError::StreamAbort => HalError::Fault,
    }
}

/// 64-byte command packet. Layout is the architectural contract; see
/// `docs/ACCEL.md`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpCmd {
    pub magic: u32,
    pub opcode: u8,
    pub dtype: u8,
    pub space: u8,
    pub phase: u8,
    pub m: u16,
    pub n: u16,
    pub k: u16,
    pub flags: u16,
    /// Packed [`StreamId`]: `[31:24] chiplet | [23:8] tile | [7:0] ssid`.
    pub stream_id: u32,
    pub chiplet: u16,
    pub tile: u16,
    pub iova_a: u64,
    pub iova_b: u64,
    pub iova_c: u64,
    pub iova_bias: u64,
    pub fence_id: u64,
}

const _: [(); CP_CMD_SIZE] = [(); core::mem::size_of::<CpCmd>()];

impl CpCmd {
    /// Translate an Aether job through Soft SMMU into a CP packet.
    ///
    /// `Nop` is a doorbell / latency probe and does not require pins.
    /// Every other op refuses unless the job's packed SID is Bound and
    /// A/B/C (and bias, if set) translate on that SID.
    pub fn pack(job: &AccelJobDesc, iommu: &IommuMap) -> Result<Self, HalError> {
        if job.m > u16::MAX as u32 || job.n > u16::MAX as u32 || job.k > u16::MAX as u32 {
            return Err(HalError::BadArg);
        }
        if job.op == AccelOp::Nop {
            return Ok(Self::empty_from(job));
        }
        let sid = stream_for_job(job);
        let a = iommu
            .translate_result(sid.raw(), job.a, None)
            .map_err(map_hal_error)?;
        let b = iommu
            .translate_result(sid.raw(), job.b, None)
            .map_err(map_hal_error)?;
        let c = iommu
            .translate_result(sid.raw(), job.c, None)
            .map_err(map_hal_error)?;
        let bias = if job.bias.0 != 0 {
            iommu
                .translate_result(sid.raw(), job.bias, None)
                .map_err(map_hal_error)?
        } else {
            PhysAddr(0)
        };
        let es = job.elem_bytes().max(1);
        let a_bytes = job.bytes_a().max(es);
        let b_bytes = job.bytes_b().max(es);
        let c_bytes = job.bytes_c().max(es);
        if !iommu.covers_stream(sid.raw(), job.a, a_bytes)
            || !iommu.covers_stream(sid.raw(), job.b, b_bytes)
            || !iommu.covers_stream(sid.raw(), job.c, c_bytes)
        {
            return Err(HalError::Fault);
        }
        if job.bias.0 != 0
            && !iommu.covers_stream(
                sid.raw(),
                job.bias,
                es.saturating_mul(job.n as u64).max(es),
            )
        {
            return Err(HalError::Fault);
        }
        let mut flags = 0u16;
        if job.bias.0 != 0 {
            flags |= CP_FLAG_HAS_BIAS;
        }
        Ok(Self {
            magic: CP_PKT_MAGIC,
            opcode: job.op as u32 as u8,
            dtype: job.dtype as u8,
            space: job.space as u8,
            phase: job.phase as u8,
            m: job.m as u16,
            n: job.n as u16,
            k: job.k as u16,
            flags,
            stream_id: sid.raw(),
            chiplet: job.place.chiplet.0 as u16,
            tile: job.place.tile.unwrap_or(0),
            iova_a: a.0,
            iova_b: b.0,
            iova_c: c.0,
            iova_bias: bias.0,
            fence_id: job.fence_id,
        })
    }

    fn empty_from(job: &AccelJobDesc) -> Self {
        let sid = stream_for_job(job);
        Self {
            magic: CP_PKT_MAGIC,
            opcode: AccelOp::Nop as u32 as u8,
            dtype: job.dtype as u8,
            space: job.space as u8,
            phase: job.phase as u8,
            m: 0,
            n: 0,
            k: 0,
            flags: 0,
            stream_id: sid.raw(),
            chiplet: job.place.chiplet.0 as u16,
            tile: job.place.tile.unwrap_or(0),
            iova_a: 0,
            iova_b: 0,
            iova_c: 0,
            iova_bias: 0,
            fence_id: job.fence_id,
        }
    }

    /// Little-endian wire image a silicon CP would DMA from the mailbox.
    pub fn to_le_bytes(self) -> [u8; CP_CMD_SIZE] {
        let mut b = [0u8; CP_CMD_SIZE];
        b[0..4].copy_from_slice(&self.magic.to_le_bytes());
        b[4] = self.opcode;
        b[5] = self.dtype;
        b[6] = self.space;
        b[7] = self.phase;
        b[8..10].copy_from_slice(&self.m.to_le_bytes());
        b[10..12].copy_from_slice(&self.n.to_le_bytes());
        b[12..14].copy_from_slice(&self.k.to_le_bytes());
        b[14..16].copy_from_slice(&self.flags.to_le_bytes());
        b[16..20].copy_from_slice(&self.stream_id.to_le_bytes());
        b[20..22].copy_from_slice(&self.chiplet.to_le_bytes());
        b[22..24].copy_from_slice(&self.tile.to_le_bytes());
        b[24..32].copy_from_slice(&self.iova_a.to_le_bytes());
        b[32..40].copy_from_slice(&self.iova_b.to_le_bytes());
        b[40..48].copy_from_slice(&self.iova_c.to_le_bytes());
        b[48..56].copy_from_slice(&self.iova_bias.to_le_bytes());
        b[56..64].copy_from_slice(&self.fence_id.to_le_bytes());
        b
    }
}

/// Software command processor. Completions arrive on the IRQ/poll path,
/// never inside [`AccelDevice::submit`].
pub struct SoftCommandProcessor<M: DmaView> {
    pub info: AccelInfo,
    pub iommu: IommuMap,
    pub mem: M,
    npu: SoftNpu,
    mailbox: Option<CpCmd>,
    pending_job: Option<AccelJobDesc>,
    doorbell: bool,
    irq: bool,
    last_cmd: Option<CpCmd>,
    last_cpl: Option<Completion>,
    last_fence: Option<u64>,
    fence_done: bool,
}

impl<M: DmaView> SoftCommandProcessor<M> {
    pub fn new(mem: M) -> Self {
        Self {
            info: AccelInfo {
                vendor: 0xAE7E,
                device: 0x0003,
                n_queues: 1,
                max_wave: 64,
                backend: ACCEL_BACKEND_SOFT_CP,
            },
            iommu: IommuMap::new(),
            mem,
            npu: SoftNpu::new(),
            mailbox: None,
            pending_job: None,
            doorbell: false,
            irq: false,
            last_cmd: None,
            last_cpl: None,
            last_fence: None,
            fence_done: false,
        }
    }

    pub fn bind_stream(
        &mut self,
        cap: &Capability,
        sid: StreamId,
    ) -> Result<aether_core::iommu::StreamState, HalError> {
        self.iommu.bind_stream(cap, sid).map_err(map_hal_error)
    }

    pub fn map_with_cap(
        &mut self,
        cap: &Capability,
        req: MapRequest,
    ) -> Result<PhysAddr, HalError> {
        let region = self.iommu.map(cap, req).map_err(map_hal_error)?;
        Ok(region.iova)
    }

    pub fn last_cmd(&self) -> Option<CpCmd> {
        self.last_cmd
    }

    pub fn doorbell_pending(&self) -> bool {
        self.doorbell
    }

    pub fn irq_pending(&self) -> bool {
        self.irq
    }

    /// Fence id retired by the last IRQ, if the job named one.
    pub fn completed_fence(&self) -> Option<u64> {
        if self.fence_done {
            self.last_fence
        } else {
            None
        }
    }

    /// Retire the IRQ seq into a CP-shaped [`Timeline`].
    ///
    /// The device names the seq; the timeline owns credit + watermark.
    /// No fence on the job is `Ok(None)`.
    pub fn retire_into(&self, timeline: &mut Timeline) -> Result<Option<Fence>, PartitionError> {
        match self.completed_fence() {
            Some(id) => timeline.complete(FenceId(id)).map(Some),
            None => Ok(None),
        }
    }

    /// Device-side: consume the mailbox, resolve Soft-SMMU IOVAs, execute, raise IRQ.
    pub fn service(&mut self) -> Option<Completion> {
        let cmd = self.mailbox.take()?;
        let job = self.pending_job.take()?;
        self.doorbell = false;
        if job.op != AccelOp::Nop && self.smmu_walk(&cmd).is_err() {
            let cpl = Completion {
                job_seq: self.npu.seq,
                status: -2,
                cycles: 0,
            };
            return Some(self.complete(cmd.fence_id, cpl));
        }
        match self.npu.execute(&job, &mut self.mem) {
            Ok(cpl) => Some(self.complete(cmd.fence_id, cpl)),
            Err(_) => {
                let cpl = Completion {
                    job_seq: self.npu.seq,
                    status: -1,
                    cycles: 0,
                };
                Some(self.complete(cmd.fence_id, cpl))
            }
        }
    }

    fn smmu_walk(&self, cmd: &CpCmd) -> Result<(), HalError> {
        let _ = self
            .iommu
            .resolve_result(cmd.stream_id, PhysAddr(cmd.iova_a), None)
            .map_err(map_hal_error)?;
        let _ = self
            .iommu
            .resolve_result(cmd.stream_id, PhysAddr(cmd.iova_b), None)
            .map_err(map_hal_error)?;
        let _ = self
            .iommu
            .resolve_result(cmd.stream_id, PhysAddr(cmd.iova_c), None)
            .map_err(map_hal_error)?;
        if cmd.flags & CP_FLAG_HAS_BIAS != 0 {
            let _ = self
                .iommu
                .resolve_result(cmd.stream_id, PhysAddr(cmd.iova_bias), None)
                .map_err(map_hal_error)?;
        }
        Ok(())
    }

    fn complete(&mut self, fence_id: u64, cpl: Completion) -> Completion {
        self.last_cpl = Some(cpl);
        self.irq = true;
        if fence_id != 0 {
            self.last_fence = Some(fence_id);
            self.fence_done = true;
        }
        cpl
    }
}

impl<M: DmaView> AccelDevice for SoftCommandProcessor<M> {
    fn probe(&mut self) -> Result<AccelInfo, HalError> {
        Ok(self.info)
    }

    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError> {
        if self.doorbell || self.mailbox.is_some() {
            return Err(HalError::Busy);
        }
        let cmd = CpCmd::pack(job, &self.iommu)?;
        self.last_cmd = Some(cmd);
        self.mailbox = Some(cmd);
        self.pending_job = Some(*job);
        self.doorbell = true;
        self.irq = false;
        self.last_cpl = None;
        self.fence_done = false;
        self.last_fence = if job.fence_id != 0 {
            Some(job.fence_id)
        } else {
            None
        };
        Ok(self.npu.seq)
    }

    fn poll(&mut self) -> Option<Completion> {
        if !self.irq {
            return None;
        }
        let cpl = self.last_cpl.take()?;
        self.irq = false;
        Some(cpl)
    }

    fn map(&mut self, _req: MapRequest) -> Result<PhysAddr, HalError> {
        Err(HalError::NoMemoryCap)
    }

    fn unmap(&mut self, iova: PhysAddr) -> Result<(), HalError> {
        self.iommu.unmap(iova).map(|_| ()).map_err(map_hal_error)
    }

    fn translate(&self, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.iommu.translate(guest_pa)
    }

    fn translate_stream(&self, stream_id: u32, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.iommu.translate_stream(stream_id, guest_pa)
    }

    fn name(&self) -> &'static str {
        "soft-cp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::accel::{DType, SliceMem};
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::fence::{FenceId, Timeline};
    use aether_core::iommu::{StreamState, SOFT_SMMU_IOVA_BASE};
    use aether_core::partition::{
        BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
    };
    use aether_core::types::{ChipletId, TenantId};
    use aether_hal::{
        AccelDevice, ACCEL_BACKEND_PARTNER_STUB, ACCEL_BACKEND_SOFTNPU,
        ACCEL_BACKEND_VIRTIO_SOFTNPU,
    };

    fn mem_cap() -> Capability {
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 3, TenantId(1)).with_generation(1)
    }

    fn matmul_backing() -> ([u8; 256], AccelJobDesc) {
        let mut backing = [0u8; 256];
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            backing[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        (backing, job)
    }

    fn pin_job(dev: &mut SoftCommandProcessor<SliceMem<'_>>, job: &AccelJobDesc) -> PhysAddr {
        let sid = stream_for_job(job);
        dev.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 256, sid))
            .unwrap()
    }

    #[test]
    fn probe_is_distinct_backend() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let info = d.probe().unwrap();
        assert_eq!(info.backend, ACCEL_BACKEND_SOFT_CP);
        assert_ne!(info.backend, ACCEL_BACKEND_SOFTNPU);
        assert_ne!(info.backend, ACCEL_BACKEND_VIRTIO_SOFTNPU);
        assert_ne!(info.backend, ACCEL_BACKEND_PARTNER_STUB);
        assert_eq!(info.device, 0x0003);
        assert_eq!(d.name(), "soft-cp");
    }

    #[test]
    fn map_requires_memory_cap_and_relocates() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let sid = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        assert_eq!(
            d.map(MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, sid))
                .unwrap_err(),
            HalError::NoMemoryCap
        );
        let no_map = Capability::new(
            CapKind::Memory,
            CapRights(CapRights::READ | CapRights::WRITE),
            1,
            TenantId(1),
        )
        .with_generation(1);
        assert_eq!(
            d.map_with_cap(
                &no_map,
                MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, sid)
            )
            .unwrap_err(),
            HalError::NoMemoryCap
        );
        let iova = d
            .map_with_cap(
                &mem_cap(),
                MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, sid),
            )
            .unwrap();
        assert!(iova.0 >= SOFT_SMMU_IOVA_BASE);
        assert_ne!(iova.0, 0x1000);
        assert_eq!(
            d.translate_stream(sid.raw(), PhysAddr(0x1400)).unwrap().0,
            iova.0 + 0x400
        );
        assert_eq!(
            d.iommu
                .resolve_stream(sid.raw(), PhysAddr(iova.0 + 0x400))
                .unwrap()
                .0,
            0x1400
        );
    }

    #[test]
    fn capture_without_bind_aborts_translate() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let sid = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        assert_eq!(d.iommu.capture(sid).unwrap(), StreamState::Captured);
        assert!(!d.iommu.is_bound(sid));
        assert_eq!(
            d.iommu
                .translate_result(sid.raw(), PhysAddr(0x1000), None)
                .unwrap_err(),
            MapError::StreamAbort
        );
        let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job.place = job.place.with_tile(2);
        assert_eq!(d.submit(&job).unwrap_err(), HalError::Fault);
        assert_eq!(d.bind_stream(&mem_cap(), sid).unwrap(), StreamState::Bound);
    }

    #[test]
    fn submit_packs_packet_poll_after_irq() {
        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        job.fence_id = 9;
        let sid = stream_for_job(&job);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        assert!(d.probe().is_ok());
        let iova = pin_job(&mut d, &job);
        assert!(iova.0 >= SOFT_SMMU_IOVA_BASE);
        assert_ne!(iova.0, 0);

        d.submit(&job).unwrap();
        assert!(d.doorbell_pending());
        assert!(d.poll().is_none(), "completions come from IRQ, not submit");

        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.magic, CP_PKT_MAGIC);
        assert_eq!(cmd.opcode, AccelOp::MatMul as u32 as u8);
        assert_eq!(cmd.dtype, 0);
        assert_eq!(cmd.m, 2);
        assert_eq!(cmd.n, 2);
        assert_eq!(cmd.k, 2);
        assert_eq!(cmd.stream_id, sid.raw());
        assert_eq!(StreamId::from_raw(cmd.stream_id).chiplet().0, 0);
        assert_eq!(StreamId::from_raw(cmd.stream_id).tile().0, 2);
        assert_eq!(StreamId::from_raw(cmd.stream_id).ssid(), CP_SSID);
        assert_eq!(cmd.tile, 2);
        assert_eq!(cmd.fence_id, 9);
        assert_eq!(cmd.iova_a, iova.0);
        assert_eq!(cmd.iova_b, iova.0 + 16);
        assert_eq!(cmd.iova_c, iova.0 + 32);
        assert_ne!(cmd.iova_a, job.a.0);
        let wire = cmd.to_le_bytes();
        assert_eq!(&wire[0..4], &CP_PKT_MAGIC.to_le_bytes());
        assert_eq!(wire.len(), CP_CMD_SIZE);

        let serviced = d.service().unwrap();
        assert_eq!(serviced.status, 0);
        assert!(d.irq_pending());
        assert_eq!(d.completed_fence(), Some(9));
        let c = d.poll().unwrap();
        assert_eq!(c.status, 0);
        assert!(!d.irq_pending());
        let out0 = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(out0, 19);
    }

    #[test]
    fn submit_packs_f32_dtype_and_executes() {
        let mut backing = [0u8; 256];
        for (i, v) in [1.0f32, 2.0, 3.0, 4.0].iter().enumerate() {
            backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_bits().to_le_bytes());
        }
        for (i, v) in [5.0f32, 6.0, 7.0, 8.0].iter().enumerate() {
            backing[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_bits().to_le_bytes());
        }
        let mut job = AccelJobDesc::matmul_f32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job.place = job.place.with_tile(2);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.dtype, DType::F32 as u8);
        assert_eq!(cmd.to_le_bytes()[5], 2);
        assert_eq!(d.service().unwrap().status, 0);
        let out0 = f32::from_bits(u32::from_le_bytes(backing[32..36].try_into().unwrap()));
        assert_eq!(out0, 19.0);
    }

    #[test]
    fn missing_map_faults() {
        let (mut backing, job) = matmul_backing();
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        assert_eq!(d.submit(&job).unwrap_err(), HalError::Fault);
    }

    #[test]
    fn partial_map_faults() {
        let (mut backing, job) = matmul_backing();
        let sid = stream_for_job(&job);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 16, sid))
            .unwrap();
        assert_eq!(d.submit(&job).unwrap_err(), HalError::Fault);
    }

    #[test]
    fn wrong_stream_faults() {
        let (mut backing, job) = matmul_backing();
        let other = StreamId::accel(ChipletId(0), TileId(0), 7);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 256, other))
            .unwrap();
        // Job packs chiplet0/tile0/ssid=1; pins live on ssid 7.
        assert_eq!(d.submit(&job).unwrap_err(), HalError::Fault);
    }

    #[test]
    fn softnpu_default_stream_is_wrong_sid_for_cp() {
        let (mut backing, job) = matmul_backing();
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        // SoftNPU stream 0 binds on first pin; Soft-CP compute uses ssid 1.
        d.map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        assert_eq!(d.submit(&job).unwrap_err(), HalError::Fault);
    }

    #[test]
    fn nop_doorbell_without_maps() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let mut job = AccelJobDesc::matmul_i32(0, 0, 0, PhysAddr(0), PhysAddr(0), PhysAddr(0), 1);
        job.op = AccelOp::Nop;
        d.submit(&job).unwrap();
        let cpl = d.service().unwrap();
        assert_eq!(cpl.status, 0);
        assert_eq!(d.poll().unwrap().status, 0);
    }

    #[test]
    fn irq_retires_partition_fence() {
        let (mut backing, mut job) = matmul_backing();
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
        job.fence_id = fence.id.0;

        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        assert!(d.completed_fence().is_none());
        d.service().unwrap();
        let cpl = d.poll().unwrap();
        assert_eq!(cpl.status, 0);
        assert_eq!(
            timeline.wait(FenceId(d.completed_fence().unwrap())),
            Err(PartitionError::FenceNotReady)
        );
        let done = d.retire_into(&mut timeline).unwrap().unwrap();
        assert!(done.completed);
        assert!(timeline.wait(done.id).unwrap().completed);
        assert_eq!(timeline.in_flight(), 0);
        assert_eq!(timeline.retired(), fence.id.0);
    }
}
