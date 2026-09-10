//! MicroPerceptron-shaped **thin** consumer of frozen [`IreeHalCmd`].
//!
//! MicroPerceptron / virtio-accel is an **inspiration name only**. This crate
//! is not a MicroPerceptron port, not a vendor, not a PJRT plugin, and not a
//! second compiler story. [`aether_pjrt`](https://github.com/amineux/aether/blob/main/host/aether-pjrt)
//! stays the compiler-facing shim. This is a research sketch: a small opcode
//! surface (`memcpy` / `matmul` / `wave`) that submits the same 96-byte
//! little-endian image (magic `0xAE7E1EE1`, `executable = 0x0001EE00`)
//! through [`IreeShapedCp`] (doorbell) or Soft-CP (SoftCmdFirewall still
//! applies). Bad `isa_blob_id` and unbound SID are refused.
//!
//! v1 `IreeHalCmd` does not define TRANSFER-only memcpy (`TRANSFER` alone is
//! Fault). `memcpy` is an explicit host copy. Path B / `make qemu` is
//! unchanged. `IreeHalCmd` offsets are unchanged.

#![deny(unsafe_code)]

use aether_core::abi::Event as AbiEvent;
use aether_core::accel::{AccelError, AccelJobDesc, AccelOp, Completion, DmaView};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::fence::Timeline;
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::partition::{
    BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
};
use aether_core::space::MemorySpace;
use aether_core::types::{ChipletId, PhysAddr, TenantId, TileId};
use aether_drivers::fakecp::{SoftCommandProcessor, CP_SSID};
use aether_drivers::firewall::FirewallSim;
use aether_drivers::ireecp::{
    IreeHalCmd, IreeShapedCp, IREE_HAL_CMD_SIZE, IREE_REF_EXECUTABLE, IREE_SSID,
};
use aether_hal::{
    AccelDevice, AccelInfo, HalError, ACCEL_BACKEND_IREE_SHAPED, ACCEL_BACKEND_SOFT_CP,
};

const HEAP_LEN: usize = 64 * 1024;
const HEAP_BASE: u64 = 0x1_0000;
const BUF_ALIGN: u64 = 64;
const MAX_BUFS: usize = 8;

/// Owned host heap the software CP DMA into. Guest PAs are offsets from
/// [`HEAP_BASE`]; Soft SMMU relocates them to IOVAs above 4 GiB.
struct HostDma {
    base: PhysAddr,
    bytes: Vec<u8>,
}

impl HostDma {
    fn new() -> Self {
        Self {
            base: PhysAddr(HEAP_BASE),
            bytes: vec![0u8; HEAP_LEN],
        }
    }

    fn off(&self, addr: PhysAddr) -> Result<usize, AccelError> {
        let off = addr
            .0
            .checked_sub(self.base.0)
            .ok_or(AccelError::Overflow)? as usize;
        Ok(off)
    }
}

impl DmaView for HostDma {
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, AccelError> {
        let off = self.off(addr)?;
        if off + 4 > self.bytes.len() {
            return Err(AccelError::Overflow);
        }
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.bytes[off..off + 4]);
        Ok(i32::from_le_bytes(b))
    }

    fn store_i32(&mut self, addr: PhysAddr, val: i32) -> Result<(), AccelError> {
        let off = self.off(addr)?;
        if off + 4 > self.bytes.len() {
            return Err(AccelError::Overflow);
        }
        self.bytes[off..off + 4].copy_from_slice(&val.to_le_bytes());
        Ok(())
    }

    fn load_u16(&self, addr: PhysAddr) -> Result<u16, AccelError> {
        let off = self.off(addr)?;
        if off + 2 > self.bytes.len() {
            return Err(AccelError::Overflow);
        }
        let mut b = [0u8; 2];
        b.copy_from_slice(&self.bytes[off..off + 2]);
        Ok(u16::from_le_bytes(b))
    }

    fn store_u16(&mut self, addr: PhysAddr, val: u16) -> Result<(), AccelError> {
        let off = self.off(addr)?;
        if off + 2 > self.bytes.len() {
            return Err(AccelError::Overflow);
        }
        self.bytes[off..off + 2].copy_from_slice(&val.to_le_bytes());
        Ok(())
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufId(pub u32);

/// Small opcode surface. Not a second IR. `Memcpy` is a host copy —
/// v1 `TRANSFER` stays reserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    Memcpy,
    MatMul,
    Wave,
}

/// Where the frozen 96-byte image is submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitPath {
    /// `IreeHalCmd` → [`IreeShapedCp::submit_hal`] (`ssid = 2`).
    Doorbell,
    /// Frozen image is packed and checked, then the job is enqueued on
    /// Soft-CP (`ssid = 1`). [`SoftCmdFirewall`] copy-then-validate still
    /// applies. Soft-CP's `CpCmd` is a different packet; offsets of
    /// `IreeHalCmd` are not relocated.
    SoftCp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Hal(HalError),
    Accel(AccelError),
    Partition(PartitionError),
    UnknownBuffer,
    OutOfMemory,
    JobFault(i32),
    NotReady,
    FrozenImage,
}

impl From<HalError> for Error {
    fn from(e: HalError) -> Self {
        Self::Hal(e)
    }
}

impl From<PartitionError> for Error {
    fn from(e: PartitionError) -> Self {
        Self::Partition(e)
    }
}

struct Alloc {
    guest_pa: PhysAddr,
    iova: PhysAddr,
    len: u64,
}

enum Engine {
    Doorbell(IreeShapedCp<HostDma>),
    SoftCp {
        cp: SoftCommandProcessor<HostDma>,
        /// IREE_SSID pins used only to pack the frozen 96-byte image.
        /// Soft-CP DMA uses `CP_SSID` on `cp.iommu`.
        freeze: IommuMap,
    },
}

impl Engine {
    fn mem(&self) -> &HostDma {
        match self {
            Self::Doorbell(d) => &d.mem,
            Self::SoftCp { cp, .. } => &cp.mem,
        }
    }

    fn mem_mut(&mut self) -> &mut HostDma {
        match self {
            Self::Doorbell(d) => &mut d.mem,
            Self::SoftCp { cp, .. } => &mut cp.mem,
        }
    }
}

/// Thin MicroPerceptron-shaped host session over one submit path.
pub struct MpShim {
    engine: Engine,
    info: AccelInfo,
    profile: PartitionProfile,
    timeline: Timeline,
    cap: Capability,
    bump: u64,
    bufs: Vec<Alloc>,
    last_cpl: Option<Completion>,
    last_hal: Option<IreeHalCmd>,
}

impl MpShim {
    /// Default path: frozen image through [`IreeShapedCp`].
    pub fn doorbell() -> Result<Self, Error> {
        Self::new(SubmitPath::Doorbell)
    }

    /// Soft-CP path: same frozen image is packed and checked; enqueue
    /// goes through [`SoftCommandProcessor`] + SoftCmdFirewall.
    pub fn soft_cp() -> Result<Self, Error> {
        Self::new(SubmitPath::SoftCp)
    }

    pub fn new(path: SubmitPath) -> Result<Self, Error> {
        let (engine, info) = match path {
            SubmitPath::Doorbell => {
                let mut cp = IreeShapedCp::new(HostDma::new());
                let info = cp.probe()?;
                if info.backend != ACCEL_BACKEND_IREE_SHAPED {
                    return Err(Error::Hal(HalError::Unsupported));
                }
                (Engine::Doorbell(cp), info)
            }
            SubmitPath::SoftCp => {
                let mut cp = SoftCommandProcessor::new(HostDma::new());
                let info = cp.probe()?;
                if info.backend != ACCEL_BACKEND_SOFT_CP {
                    return Err(Error::Hal(HalError::Unsupported));
                }
                (
                    Engine::SoftCp {
                        cp,
                        freeze: IommuMap::new(),
                    },
                    info,
                )
            }
        };
        Ok(Self {
            engine,
            info,
            profile: PartitionProfile::new(
                PartitionId(1),
                SpatialSlice::single_chiplet(ChipletId(0), 0b1111, 0b1),
                QosBudget {
                    bw_mbps: 100,
                    credits: 8,
                },
                BlastRadius {
                    max_nodes: 4,
                    max_hops: 2,
                },
            ),
            timeline: Timeline::new(PartitionId(1)),
            cap: Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1))
                .with_generation(1),
            bump: HEAP_BASE,
            bufs: Vec::new(),
            last_cpl: None,
            last_hal: None,
        })
    }

    pub fn path(&self) -> SubmitPath {
        match self.engine {
            Engine::Doorbell(_) => SubmitPath::Doorbell,
            Engine::SoftCp { .. } => SubmitPath::SoftCp,
        }
    }

    pub fn info(&self) -> AccelInfo {
        self.info
    }

    pub fn name(&self) -> &'static str {
        match &self.engine {
            Engine::Doorbell(d) => d.name(),
            Engine::SoftCp { cp, .. } => cp.name(),
        }
    }

    fn dma_stream(&self) -> StreamId {
        match self.engine {
            Engine::Doorbell(_) => StreamId::accel(ChipletId(0), TileId(0), IREE_SSID),
            Engine::SoftCp { .. } => StreamId::accel(ChipletId(0), TileId(0), CP_SSID),
        }
    }

    fn iree_stream() -> StreamId {
        StreamId::accel(ChipletId(0), TileId(0), IREE_SSID)
    }

    /// Allocate and pin through Soft SMMU (Memory+MAP).
    /// [`AccelDevice::map`] without a cap walk is [`HalError::NoMemoryCap`].
    pub fn allocate(&mut self, len: u64) -> Result<BufId, Error> {
        if len == 0 {
            return Err(Error::Hal(HalError::BadArg));
        }
        if self.bufs.len() >= MAX_BUFS {
            return Err(Error::OutOfMemory);
        }
        let aligned = (len + BUF_ALIGN - 1) & !(BUF_ALIGN - 1);
        let end = HEAP_BASE + HEAP_LEN as u64;
        if self.bump.saturating_add(aligned) > end {
            return Err(Error::OutOfMemory);
        }
        let guest_pa = PhysAddr(self.bump);
        let req = MapRequest::pin_accel(guest_pa, aligned, self.dma_stream());
        let iova = match &mut self.engine {
            Engine::Doorbell(d) => d.map_with_cap(&self.cap, req)?,
            Engine::SoftCp { cp, freeze } => {
                let iova = cp.map_with_cap(&self.cap, req)?;
                freeze
                    .map(
                        &self.cap,
                        MapRequest::pin_accel(guest_pa, aligned, Self::iree_stream()),
                    )
                    .map_err(map_hal_error)?;
                iova
            }
        };
        self.bump += aligned;
        self.bufs.push(Alloc {
            guest_pa,
            iova,
            len,
        });
        Ok(BufId(self.bufs.len() as u32))
    }

    pub fn buffer_iova(&self, id: BufId) -> Result<PhysAddr, Error> {
        Ok(self.alloc(id)?.iova)
    }

    pub fn buffer_guest_pa(&self, id: BufId) -> Result<PhysAddr, Error> {
        Ok(self.alloc(id)?.guest_pa)
    }

    fn alloc(&self, id: BufId) -> Result<&Alloc, Error> {
        let i = id.0.checked_sub(1).ok_or(Error::UnknownBuffer)? as usize;
        self.bufs.get(i).ok_or(Error::UnknownBuffer)
    }

    /// Opcode `memcpy`: explicit host copy. Not a HAL `TRANSFER` packet.
    pub fn memcpy(&mut self, dst: BufId, src: BufId) -> Result<Opcode, Error> {
        let (src_off, dst_off, n) = {
            let s = self.alloc(src)?;
            let d = self.alloc(dst)?;
            let n = core::cmp::min(s.len, d.len) as usize;
            let src_off = self.engine.mem().off(s.guest_pa).map_err(Error::Accel)?;
            let dst_off = self.engine.mem().off(d.guest_pa).map_err(Error::Accel)?;
            (src_off, dst_off, n)
        };
        if src_off == dst_off {
            return Ok(Opcode::Memcpy);
        }
        let bytes = self.engine.mem().bytes[src_off..src_off + n].to_vec();
        self.engine.mem_mut().bytes[dst_off..dst_off + n].copy_from_slice(&bytes);
        Ok(Opcode::Memcpy)
    }

    /// Fill a buffer from the host. Not a v1 TRANSFER packet.
    pub fn copy_i32_from_host(&mut self, id: BufId, vals: &[i32]) -> Result<(), Error> {
        let off = {
            let a = self.alloc(id)?;
            if (vals.len() * 4) as u64 > a.len {
                return Err(Error::Hal(HalError::BadArg));
            }
            self.engine.mem().off(a.guest_pa).map_err(Error::Accel)?
        };
        for (i, v) in vals.iter().enumerate() {
            let o = off + i * 4;
            self.engine.mem_mut().bytes[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        Ok(())
    }

    pub fn copy_i32_to_host(&self, id: BufId, out: &mut [i32]) -> Result<(), Error> {
        let a = self.alloc(id)?;
        if (out.len() * 4) as u64 > a.len {
            return Err(Error::Hal(HalError::BadArg));
        }
        let off = self.engine.mem().off(a.guest_pa).map_err(Error::Accel)?;
        for (i, slot) in out.iter_mut().enumerate() {
            let o = off + i * 4;
            *slot = i32::from_le_bytes(self.engine.mem().bytes[o..o + 4].try_into().unwrap());
        }
        Ok(())
    }

    /// Pin-less Nop doorbell (`command_categories = 0`).
    pub fn ring(&mut self) -> Result<AbiEvent, Error> {
        let mut job = AccelJobDesc::matmul_i32(0, 0, 0, PhysAddr(0), PhysAddr(0), PhysAddr(0), 1);
        job.op = AccelOp::Nop;
        job.space = MemorySpace::Host;
        self.submit_job(job)
    }

    /// MatMul-shaped `DISPATCH` on pinned buffers.
    pub fn submit_matmul(
        &mut self,
        m: u32,
        n: u32,
        k: u32,
        a: BufId,
        b: BufId,
        c: BufId,
    ) -> Result<AbiEvent, Error> {
        if m == 0 || n == 0 || k == 0 {
            return Err(Error::Accel(AccelError::BadShape));
        }
        let (pa_a, pa_b, pa_c) = {
            let aa = self.alloc(a)?;
            let bb = self.alloc(b)?;
            let cc = self.alloc(c)?;
            let es = 4u64;
            if es * m as u64 * k as u64 > aa.len
                || es * k as u64 * n as u64 > bb.len
                || es * m as u64 * n as u64 > cc.len
            {
                return Err(Error::Hal(HalError::BadArg));
            }
            (aa.guest_pa, bb.guest_pa, cc.guest_pa)
        };
        let mut job = AccelJobDesc::matmul_i32(m, n, k, pa_a, pa_b, pa_c, 1);
        job.space = MemorySpace::Host;
        job.place = job.place.with_tile(0);
        job.partition = self.profile.id;
        self.submit_job(job)
    }

    /// Wave-shaped fused `DISPATCH` (`function = 1`) plus bias.
    pub fn submit_wave(
        &mut self,
        m: u32,
        n: u32,
        k: u32,
        a: BufId,
        b: BufId,
        c: BufId,
        bias: BufId,
    ) -> Result<AbiEvent, Error> {
        if m == 0 || n == 0 || k == 0 {
            return Err(Error::Accel(AccelError::BadShape));
        }
        let (pa_a, pa_b, pa_c, pa_bias) = {
            let aa = self.alloc(a)?;
            let bb = self.alloc(b)?;
            let cc = self.alloc(c)?;
            let bias = self.alloc(bias)?;
            let es = 4u64;
            if es * m as u64 * k as u64 > aa.len
                || es * k as u64 * n as u64 > bb.len
                || es * m as u64 * n as u64 > cc.len
                || es * n as u64 > bias.len
            {
                return Err(Error::Hal(HalError::BadArg));
            }
            (aa.guest_pa, bb.guest_pa, cc.guest_pa, bias.guest_pa)
        };
        let mut job = AccelJobDesc::matmul_i32(m, n, k, pa_a, pa_b, pa_c, 1);
        job.op = AccelOp::Wave;
        job.bias = pa_bias;
        job.space = MemorySpace::Host;
        job.place = job.place.with_tile(0);
        job.partition = self.profile.id;
        self.submit_job(job)
    }

    /// Pack the frozen image, then submit on the bound path.
    pub fn submit_job(&mut self, mut job: AccelJobDesc) -> Result<AbiEvent, Error> {
        job.partition = self.profile.id;
        let fence = self.timeline.submit(&self.profile, None)?;
        job.fence_id = fence.id.0;
        let event = AbiEvent::on_timeline(fence.id, self.profile.id);
        let packed = match self.pack_frozen(&job) {
            Ok(cmd) => cmd,
            Err(e) => {
                let _ = self.timeline.timeout(fence.id);
                return Err(e);
            }
        };
        self.enqueue(packed, &job, event)
    }

    /// Submit a caller-packed frozen image (tests: bad executable, SID skip).
    pub fn submit_image(&mut self, cmd: IreeHalCmd, job: &AccelJobDesc) -> Result<AbiEvent, Error> {
        let mut job = *job;
        job.partition = self.profile.id;
        let fence = self.timeline.submit(&self.profile, None)?;
        job.fence_id = fence.id.0;
        let event = AbiEvent::on_timeline(fence.id, self.profile.id);
        self.enqueue(cmd, &job, event)
    }

    fn pack_frozen(&self, job: &AccelJobDesc) -> Result<IreeHalCmd, Error> {
        let iommu = match &self.engine {
            Engine::Doorbell(d) => &d.iommu,
            Engine::SoftCp { freeze, .. } => freeze,
        };
        IreeHalCmd::pack(job, iommu).map_err(Error::Hal)
    }

    fn enqueue(
        &mut self,
        cmd: IreeHalCmd,
        job: &AccelJobDesc,
        event: AbiEvent,
    ) -> Result<AbiEvent, Error> {
        if let Err(e) = Self::check_frozen(&cmd, job) {
            let _ = self.timeline.timeout(event.fence);
            return Err(e);
        }
        let submit = match &mut self.engine {
            Engine::Doorbell(d) => d.submit_hal(cmd, job).map(|_| ()),
            Engine::SoftCp { cp, .. } => cp.submit_xqueue(0, job).map(|_| ()),
        };
        match submit {
            Ok(()) => {
                self.last_hal = Some(cmd);
                Ok(event)
            }
            Err(e) => {
                let _ = self.timeline.timeout(event.fence);
                Err(Error::Hal(e))
            }
        }
    }

    fn check_frozen(cmd: &IreeHalCmd, job: &AccelJobDesc) -> Result<(), Error> {
        cmd.check_v1().map_err(Error::Hal)?;
        let wire = cmd.to_le_bytes();
        if wire.len() != IREE_HAL_CMD_SIZE {
            return Err(Error::FrozenImage);
        }
        if StreamId::from_raw(cmd.stream_id).ssid() != IREE_SSID {
            return Err(Error::FrozenImage);
        }
        if job.op != AccelOp::Nop && cmd.executable != IREE_REF_EXECUTABLE {
            return Err(Error::FrozenImage);
        }
        Ok(())
    }

    pub fn last_cmd(&self) -> Option<IreeHalCmd> {
        self.last_hal
    }

    pub fn last_wire(&self) -> Option<[u8; IREE_HAL_CMD_SIZE]> {
        self.last_hal.map(|c| c.to_le_bytes())
    }

    /// SoftCmdFirewall step counters. `None` on the doorbell path.
    pub fn last_firewall_sim(&self) -> Option<FirewallSim> {
        match &self.engine {
            Engine::Doorbell(_) => None,
            Engine::SoftCp { cp, .. } => Some(cp.last_firewall_sim()),
        }
    }

    /// `AccelDevice::map` without a cap walk — second clients cannot skip this.
    pub fn map_without_cap(&mut self, guest_pa: PhysAddr, len: u64) -> Result<PhysAddr, HalError> {
        let sid = self.dma_stream();
        match &mut self.engine {
            Engine::Doorbell(d) => AccelDevice::map(d, MapRequest::pin_accel(guest_pa, len, sid)),
            Engine::SoftCp { cp, .. } => {
                AccelDevice::map(cp, MapRequest::pin_accel(guest_pa, len, sid))
            }
        }
    }

    /// Pump `service()` then retire the timeline. Host tests have no IRQ thread.
    pub fn wait(&mut self, event: AbiEvent) -> Result<Completion, Error> {
        if event.partition.0 != self.profile.id.0 {
            return Err(Error::Partition(PartitionError::Unbound));
        }
        if self.timeline.wait(event.fence).is_ok() {
            return self.last_cpl.ok_or(Error::NotReady);
        }
        let serviced = match &mut self.engine {
            Engine::Doorbell(d) => d.service().ok_or(Error::NotReady)?,
            Engine::SoftCp { cp, .. } => cp.service().ok_or(Error::NotReady)?,
        };
        let cpl = match &mut self.engine {
            Engine::Doorbell(d) => d.poll().unwrap_or(serviced),
            Engine::SoftCp { cp, .. } => AccelDevice::poll(cp).unwrap_or(serviced),
        };
        match &self.engine {
            Engine::Doorbell(d) => {
                d.retire_into(&mut self.timeline)?;
            }
            Engine::SoftCp { cp, .. } => {
                cp.retire_into(&mut self.timeline)?;
            }
        }
        self.timeline.wait(event.fence)?;
        self.last_cpl = Some(cpl);
        if cpl.status != 0 {
            return Err(Error::JobFault(cpl.status));
        }
        Ok(cpl)
    }

    pub fn fence_ready(&self, event: AbiEvent) -> bool {
        self.timeline.wait(event.fence).is_ok()
    }

    pub fn iommu(&self) -> &IommuMap {
        match &self.engine {
            Engine::Doorbell(d) => &d.iommu,
            Engine::SoftCp { cp, .. } => &cp.iommu,
        }
    }

    pub fn iommu_mut(&mut self) -> &mut IommuMap {
        match &mut self.engine {
            Engine::Doorbell(d) => &mut d.iommu,
            Engine::SoftCp { cp, .. } => &mut cp.iommu,
        }
    }

    pub fn stream_id(&self) -> StreamId {
        self.dma_stream()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::iommu::{StreamState, SOFT_SMMU_IOVA_BASE};
    use aether_drivers::ireecp::{
        HAL_FN_FUSED, IREE_HAL_COMMAND_CATEGORY_DISPATCH, IREE_HAL_COMMAND_CATEGORY_TRANSFER,
        IREE_HAL_PKT_MAGIC,
    };
    use aether_hal::{
        ACCEL_BACKEND_PARTNER_STUB, ACCEL_BACKEND_SOFTNPU, ACCEL_BACKEND_VIRTIO_SOFTNPU,
    };

    #[test]
    fn probe_doorbell_is_iree_shaped_not_a_new_device() {
        let d = MpShim::doorbell().unwrap();
        assert_eq!(d.path(), SubmitPath::Doorbell);
        assert_eq!(d.info().backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_eq!(d.name(), "iree-shaped-cp");
        assert_ne!(d.info().backend, ACCEL_BACKEND_SOFTNPU);
        assert_ne!(d.info().backend, ACCEL_BACKEND_VIRTIO_SOFTNPU);
        assert_ne!(d.info().backend, ACCEL_BACKEND_PARTNER_STUB);
        assert_ne!(d.info().backend, ACCEL_BACKEND_SOFT_CP);
        assert_eq!(d.info().n_queues, 1, "still a single mailbox");
    }

    #[test]
    fn probe_soft_cp_is_not_a_vendor() {
        let d = MpShim::soft_cp().unwrap();
        assert_eq!(d.path(), SubmitPath::SoftCp);
        assert_eq!(d.info().backend, ACCEL_BACKEND_SOFT_CP);
        assert_eq!(d.name(), "soft-cp");
        assert_ne!(d.info().backend, ACCEL_BACKEND_IREE_SHAPED);
    }

    #[test]
    fn allocate_pins_soft_smmu_not_identity() {
        for path in [SubmitPath::Doorbell, SubmitPath::SoftCp] {
            let mut d = MpShim::new(path).unwrap();
            let buf = d.allocate(64).unwrap();
            let iova = d.buffer_iova(buf).unwrap();
            let pa = d.buffer_guest_pa(buf).unwrap();
            assert!(iova.0 >= SOFT_SMMU_IOVA_BASE, "{path:?}");
            assert_ne!(iova.0, pa.0, "{path:?}");
            assert_eq!(
                d.iommu().translate_stream(d.stream_id().raw(), pa),
                Some(iova),
                "{path:?}"
            );
        }
    }

    #[test]
    fn map_without_cap_is_refused() {
        for path in [SubmitPath::Doorbell, SubmitPath::SoftCp] {
            let mut d = MpShim::new(path).unwrap();
            assert_eq!(
                d.map_without_cap(PhysAddr(HEAP_BASE), 64).unwrap_err(),
                HalError::NoMemoryCap,
                "{path:?}"
            );
        }
    }

    #[test]
    fn memcpy_is_host_copy_not_transfer() {
        let mut d = MpShim::doorbell().unwrap();
        let src = d.allocate(16).unwrap();
        let dst = d.allocate(16).unwrap();
        d.copy_i32_from_host(src, &[1, 2, 3, 4]).unwrap();
        assert_eq!(d.memcpy(dst, src).unwrap(), Opcode::Memcpy);
        let mut got = [0i32; 4];
        d.copy_i32_to_host(dst, &mut got).unwrap();
        assert_eq!(got, [1, 2, 3, 4]);
        assert!(d.last_cmd().is_none(), "memcpy must not pack TRANSFER");
    }

    #[test]
    fn submit_matmul_wait_event_both_paths() {
        for path in [SubmitPath::Doorbell, SubmitPath::SoftCp] {
            let mut d = MpShim::new(path).unwrap();
            let a = d.allocate(16).unwrap();
            let b = d.allocate(16).unwrap();
            let out = d.allocate(16).unwrap();
            d.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
            d.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
            let ev = d.submit_matmul(2, 2, 2, a, b, out).unwrap();
            assert!(!d.fence_ready(ev), "{path:?}");
            let cmd = d.last_cmd().unwrap();
            assert_eq!(cmd.magic, IREE_HAL_PKT_MAGIC, "{path:?}");
            assert_eq!(cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE, "{path:?}");
            assert_eq!(
                cmd.command_categories, IREE_HAL_COMMAND_CATEGORY_DISPATCH,
                "{path:?}"
            );
            assert_eq!(cmd.executable, IREE_REF_EXECUTABLE, "{path:?}");
            assert_eq!(cmd.workgroup_count_x, 2, "{path:?}");
            assert_eq!(cmd.workgroup_count_y, 2, "{path:?}");
            assert_eq!(cmd.workgroup_count_z, 2, "{path:?}");
            assert_eq!(cmd.binding0_length, 16, "{path:?}");
            assert_eq!(
                StreamId::from_raw(cmd.stream_id).ssid(),
                IREE_SSID,
                "{path:?}"
            );
            if path == SubmitPath::SoftCp {
                assert!(
                    d.last_firewall_sim().unwrap().noted(),
                    "SoftCmdFirewall copy-then-validate on Soft-CP"
                );
            }
            d.wait(ev).unwrap();
            let mut got = [0i32; 4];
            d.copy_i32_to_host(out, &mut got).unwrap();
            assert_eq!(got, [19, 22, 43, 50], "{path:?}");
        }
    }

    #[test]
    fn submit_wave_wait_event_both_paths() {
        for path in [SubmitPath::Doorbell, SubmitPath::SoftCp] {
            let mut d = MpShim::new(path).unwrap();
            let a = d.allocate(16).unwrap();
            let b = d.allocate(16).unwrap();
            let out = d.allocate(16).unwrap();
            let bias = d.allocate(8).unwrap();
            d.copy_i32_from_host(a, &[1, 0, 0, 1]).unwrap();
            d.copy_i32_from_host(b, &[1, 2, 3, 4]).unwrap();
            d.copy_i32_from_host(bias, &[10, 20]).unwrap();
            let ev = d.submit_wave(2, 2, 2, a, b, out, bias).unwrap();
            let cmd = d.last_cmd().unwrap();
            assert_eq!(cmd.decode_op().unwrap(), AccelOp::Wave, "{path:?}");
            assert_eq!(cmd.function, HAL_FN_FUSED, "{path:?}");
            if path == SubmitPath::SoftCp {
                assert!(d.last_firewall_sim().unwrap().noted(), "{path:?}");
            }
            d.wait(ev).unwrap();
            let mut got = [0i32; 4];
            d.copy_i32_to_host(out, &mut got).unwrap();
            assert_eq!(got, [11, 22, 13, 24], "{path:?}");
        }
    }

    #[test]
    fn cannot_skip_soft_smmu_map() {
        for path in [SubmitPath::Doorbell, SubmitPath::SoftCp] {
            let mut d = MpShim::new(path).unwrap();
            let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
            assert_eq!(
                d.submit_job(job).unwrap_err(),
                Error::Hal(HalError::Fault),
                "{path:?}"
            );
        }
    }

    #[test]
    fn cannot_skip_sid_stamp() {
        let mut d = MpShim::doorbell().unwrap();
        let a = d.allocate(256).unwrap();
        let pa = d.buffer_guest_pa(a).unwrap();
        let mut job =
            AccelJobDesc::matmul_i32(2, 2, 2, pa, PhysAddr(pa.0 + 16), PhysAddr(pa.0 + 32), 1);
        job.place = job.place.with_tile(0);
        let packed = IreeHalCmd::pack(&job, d.iommu()).unwrap();
        assert_eq!(StreamId::from_raw(packed.stream_id).ssid(), IREE_SSID);

        let mut skipped = packed;
        skipped.stream_id = 0;
        assert_eq!(
            d.submit_image(skipped, &job).unwrap_err(),
            Error::FrozenImage
        );
    }

    #[test]
    fn unbound_sid_is_refused() {
        for path in [SubmitPath::Doorbell, SubmitPath::SoftCp] {
            let mut d = MpShim::new(path).unwrap();
            let sid = d.stream_id();
            assert_eq!(d.iommu_mut().capture(sid).unwrap(), StreamState::Captured);
            let job = AccelJobDesc::matmul_i32(
                2,
                2,
                2,
                PhysAddr(HEAP_BASE),
                PhysAddr(HEAP_BASE + 16),
                PhysAddr(HEAP_BASE + 32),
                1,
            );
            assert_eq!(
                d.submit_job(job).unwrap_err(),
                Error::Hal(HalError::Fault),
                "{path:?}"
            );
        }
    }

    #[test]
    fn bad_executable_is_refused() {
        for path in [SubmitPath::Doorbell, SubmitPath::SoftCp] {
            let mut d = MpShim::new(path).unwrap();
            let a = d.allocate(256).unwrap();
            let pa = d.buffer_guest_pa(a).unwrap();
            let mut job =
                AccelJobDesc::matmul_i32(2, 2, 2, pa, PhysAddr(pa.0 + 16), PhysAddr(pa.0 + 32), 1);
            job.place = job.place.with_tile(0);
            let mut cmd = d.pack_frozen(&job).unwrap();
            cmd.executable = 0xDEAD;
            assert_eq!(
                d.submit_image(cmd, &job).unwrap_err(),
                Error::Hal(HalError::Unsupported),
                "{path:?}"
            );
        }
    }

    #[test]
    fn transfer_only_image_is_refused() {
        let mut d = MpShim::doorbell().unwrap();
        let a = d.allocate(256).unwrap();
        let pa = d.buffer_guest_pa(a).unwrap();
        let mut job =
            AccelJobDesc::matmul_i32(2, 2, 2, pa, PhysAddr(pa.0 + 16), PhysAddr(pa.0 + 32), 1);
        job.place = job.place.with_tile(0);
        let mut cmd = IreeHalCmd::pack(&job, d.iommu()).unwrap();
        cmd.command_categories = IREE_HAL_COMMAND_CATEGORY_TRANSFER;
        assert_eq!(
            d.submit_image(cmd, &job).unwrap_err(),
            Error::Hal(HalError::Fault)
        );
    }

    #[test]
    fn soft_cp_path_still_runs_softcmdfirewall() {
        let mut d = MpShim::soft_cp().unwrap();
        let a = d.allocate(16).unwrap();
        let b = d.allocate(16).unwrap();
        let out = d.allocate(16).unwrap();
        d.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
        d.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
        let ev = d.submit_matmul(2, 2, 2, a, b, out).unwrap();
        let sim = d.last_firewall_sim().unwrap();
        assert!(sim.copy_steps > 0 && sim.validate_steps > 0);
        assert!(sim.noted(), "SoftCmdFirewall copy-then-validate");
        d.wait(ev).unwrap();
        let ev = d.submit_matmul(2, 2, 2, a, b, out).unwrap();
        assert!(d.last_firewall_sim().unwrap().noted());
        d.wait(ev).unwrap();
        let mut got = [0i32; 4];
        d.copy_i32_to_host(out, &mut got).unwrap();
        assert_eq!(got, [19, 22, 43, 50]);
    }

    #[test]
    fn pjrt_doorbell_and_mp_shim_coexist() {
        let mut pjrt = aether_pjrt::Client::iree_shaped().unwrap();
        let a = pjrt
            .allocate(aether_core::space::MemorySpace::Host, 16)
            .unwrap();
        let b = pjrt
            .allocate(aether_core::space::MemorySpace::Host, 16)
            .unwrap();
        let out = pjrt
            .allocate(aether_core::space::MemorySpace::Host, 16)
            .unwrap();
        pjrt.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
        pjrt.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
        let exec = pjrt
            .load_executable(AccelOp::MatMul, aether_core::accel::DType::I32)
            .unwrap();
        let ev = pjrt
            .execute(aether_pjrt::Dispatch::matmul(exec, 2, 2, 2, a, b, out))
            .unwrap();
        let pjrt_cmd = pjrt.last_iree_cmd().unwrap();
        pjrt.wait(ev).unwrap();
        let mut got = [0i32; 4];
        pjrt.copy_i32_to_host(out, &mut got).unwrap();
        assert_eq!(got, [19, 22, 43, 50]);

        let mut bell = aether_accel_client::Doorbell::new().unwrap();
        let a = bell.allocate(16).unwrap();
        let b = bell.allocate(16).unwrap();
        let out = bell.allocate(16).unwrap();
        bell.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
        bell.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
        let ev = bell.submit_matmul(2, 2, 2, a, b, out).unwrap();
        let bell_cmd = bell.last_cmd().unwrap();
        bell.wait(ev).unwrap();
        let mut got = [0i32; 4];
        bell.copy_i32_to_host(out, &mut got).unwrap();
        assert_eq!(got, [19, 22, 43, 50]);

        let mut mp = MpShim::doorbell().unwrap();
        let a = mp.allocate(16).unwrap();
        let b = mp.allocate(16).unwrap();
        let out = mp.allocate(16).unwrap();
        mp.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
        mp.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
        let ev = mp.submit_matmul(2, 2, 2, a, b, out).unwrap();
        let mp_cmd = mp.last_cmd().unwrap();
        mp.wait(ev).unwrap();
        let mut got = [0i32; 4];
        mp.copy_i32_to_host(out, &mut got).unwrap();
        assert_eq!(got, [19, 22, 43, 50]);

        assert_eq!(pjrt_cmd.magic, bell_cmd.magic);
        assert_eq!(pjrt_cmd.magic, mp_cmd.magic);
        assert_eq!(pjrt_cmd.magic, IREE_HAL_PKT_MAGIC);
        assert_eq!(pjrt_cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(bell_cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(mp_cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(pjrt_cmd.executable, IREE_REF_EXECUTABLE);
        assert_eq!(bell_cmd.executable, IREE_REF_EXECUTABLE);
        assert_eq!(mp_cmd.executable, IREE_REF_EXECUTABLE);
        assert_eq!(StreamId::from_raw(pjrt_cmd.stream_id).ssid(), IREE_SSID);
        assert_eq!(StreamId::from_raw(bell_cmd.stream_id).ssid(), IREE_SSID);
        assert_eq!(StreamId::from_raw(mp_cmd.stream_id).ssid(), IREE_SSID);
        assert_eq!(pjrt.info().backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_eq!(bell.info().backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_eq!(mp.info().backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_eq!(pjrt.name(), mp.name());
        assert_eq!(bell.name(), mp.name());
    }
}
