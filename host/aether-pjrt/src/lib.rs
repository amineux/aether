//! Host-side PJRT / IREE HAL-shaped shim over [`aether_hal::AccelDevice`].
//!
//! Public nouns match OpenXLA PJRT (`PJRT_Client`, `PJRT_Device`,
//! `PJRT_Memory`, `PJRT_Buffer`, `PJRT_Executable` /
//! `PJRT_LoadedExecutable`, `PJRT_Event`) and IREE HAL
//! (`iree_hal_device_t`, `iree_hal_buffer_t`, `iree_hal_executable_t`,
//! `iree_hal_event_t`, `iree_hal_fence_t`). See [`docs/HOST.md`].
//!
//! This crate is **not** a PJRT plugin, **not** an IREE HAL driver, and
//! **not** a vendor runtime. It maps `abi::{Device,Buffer,Executable,Event}`
//! onto a frozen [`IreeHalCmd`] image and submits into [`IreeShapedCp`].
//! SoftNPU remains a host backend for virtqueue tests; `make qemu` still
//! demos path-B SoftNPU. `PartnerNpuStub` is not used.
//!
//! [`docs/HOST.md`]: https://github.com/amineux/aether/blob/main/docs/HOST.md

#![deny(unsafe_code)]

use aether_core::abi::{
    Buffer as AbiBuffer, Device as AbiDevice, Event as AbiEvent, Executable as AbiExecutable,
};
use aether_core::accel::{AccelError, AccelJobDesc, AccelOp, Completion, DType, DmaView};
use aether_core::activity::{Activity, ActivityId, ActivityKind};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::fence::Timeline;
use aether_core::iommu::{MapRequest, StreamId, DEFAULT_STREAM};
use aether_core::partition::{
    BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
};
use aether_core::phase::Phase;
use aether_core::space::{FabricAddr, MemorySpace, Place, SpaceError};
use aether_core::types::{ChipletId, PhysAddr, TenantId, TileId};
use aether_drivers::ireecp::{
    stream_for_job, IreeHalCmd, IreeHalNouns, IreeShapedCp, IREE_HAL_CMD_SIZE, IREE_HAL_PKT_MAGIC,
    IREE_REF_EXECUTABLE, IREE_SSID,
};
use aether_drivers::SoftNpuDevice;
use aether_hal::{AccelDevice, AccelInfo, HalError};

/// Spaces a PJRT `PJRT_Memory` / IREE allocator would list as addressable.
/// `SCRATCH` and `STREAMING` exist on the fabric but are not randomly mappable.
pub const ADDRESSABLE_SPACES: &[MemorySpace] = &[
    MemorySpace::Host,
    MemorySpace::DeviceHbm,
    MemorySpace::TileSram,
    MemorySpace::CxlRegion,
];

const HEAP_LEN: usize = 64 * 1024;
const HEAP_BASE: u64 = 0x1_0000;
const BUF_ALIGN: u64 = 64;
const MAX_BUFS: usize = 16;

/// Which real `AccelDevice` the client binds. Never a fake vendor plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    /// SoftNPU behind the virtqueue MMIO BAR (`backend = 1`).
    SoftNpu,
    /// IREE HAL-shaped command processor (`backend = 4`). Not Soft-CP.
    IreeShaped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutableId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Hal(HalError),
    Accel(AccelError),
    Partition(PartitionError),
    Space(SpaceError),
    UnknownBuffer,
    UnknownExecutable,
    SpaceMismatch,
    NotReady,
    JobFault(i32),
    OutOfMemory,
    Unsupported,
    /// Packed `IreeHalCmd` did not match the SpecForge freeze.
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

impl From<SpaceError> for Error {
    fn from(e: SpaceError) -> Self {
        Self::Space(e)
    }
}

/// Owned host heap the software NPU / CP DMA into. Guest PAs are offsets
/// from [`HEAP_BASE`]; Soft SMMU relocates them to IOVAs above 4 GiB.
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

enum Engine {
    SoftNpu(SoftNpuDevice<HostDma>),
    IreeShaped(IreeShapedCp<HostDma>),
}

impl Engine {
    fn probe(&mut self) -> Result<AccelInfo, HalError> {
        match self {
            Self::SoftNpu(d) => d.probe(),
            Self::IreeShaped(d) => d.probe(),
        }
    }

    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError> {
        match self {
            Self::SoftNpu(d) => d.submit(job),
            Self::IreeShaped(d) => d.submit(job),
        }
    }

    fn submit_iree(&mut self, job: &AccelJobDesc, nouns: &IreeHalNouns) -> Result<u32, HalError> {
        match self {
            Self::IreeShaped(d) => {
                let cmd = IreeHalCmd::pack_with_nouns(job, &d.iommu, nouns)?;
                d.submit_hal(cmd, job)
            }
            Self::SoftNpu(_) => Err(HalError::Unsupported),
        }
    }

    fn last_iree_cmd(&self) -> Option<IreeHalCmd> {
        match self {
            Self::IreeShaped(d) => d.last_cmd(),
            Self::SoftNpu(_) => None,
        }
    }

    fn poll(&mut self) -> Option<Completion> {
        match self {
            Self::SoftNpu(d) => d.poll(),
            Self::IreeShaped(d) => d.poll(),
        }
    }

    fn service(&mut self) -> Option<Completion> {
        match self {
            Self::SoftNpu(d) => d.service(),
            Self::IreeShaped(d) => d.service(),
        }
    }

    fn map_with_cap(&mut self, cap: &Capability, req: MapRequest) -> Result<PhysAddr, HalError> {
        match self {
            Self::SoftNpu(d) => d.map_with_cap(cap, req),
            Self::IreeShaped(d) => d.map_with_cap(cap, req),
        }
    }

    fn translate_stream(&self, stream_id: u32, guest_pa: PhysAddr) -> Option<PhysAddr> {
        match self {
            Self::SoftNpu(d) => d.translate_stream(stream_id, guest_pa),
            Self::IreeShaped(d) => d.translate_stream(stream_id, guest_pa),
        }
    }

    fn retire_into(
        &self,
        timeline: &mut Timeline,
    ) -> Result<Option<aether_core::fence::Fence>, PartitionError> {
        match self {
            Self::SoftNpu(d) => d.retire_into(timeline),
            Self::IreeShaped(d) => d.retire_into(timeline),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::SoftNpu(d) => d.name(),
            Self::IreeShaped(d) => d.name(),
        }
    }

    fn mem(&self) -> &HostDma {
        match self {
            Self::SoftNpu(d) => &d.mem,
            Self::IreeShaped(d) => &d.mem,
        }
    }

    fn mem_mut(&mut self) -> &mut HostDma {
        match self {
            Self::SoftNpu(d) => &mut d.mem,
            Self::IreeShaped(d) => &mut d.mem,
        }
    }
}

struct Alloc {
    space: MemorySpace,
    place: Place,
    guest_pa: PhysAddr,
    iova: PhysAddr,
    len: u64,
    unified: bool,
}

impl Alloc {
    fn as_abi(&self) -> AbiBuffer {
        AbiBuffer {
            space: self.space,
            addr: FabricAddr::new(self.place, self.guest_pa.0),
            len: self.len,
            unified: self.unified,
        }
    }
}

struct Loaded {
    exec: AbiExecutable,
    op: AccelOp,
    dtype: DType,
}

/// PJRT_Client / IREE HAL device session over a real AccelDevice.
pub struct Client {
    kind: BackendKind,
    engine: Engine,
    info: AccelInfo,
    activity: Activity,
    device: AbiDevice,
    profile: PartitionProfile,
    timeline: Timeline,
    cap: Capability,
    bump: u64,
    bufs: Vec<Alloc>,
    execs: Vec<Loaded>,
    last_cpl: Option<Completion>,
}

impl Client {
    /// `PJRT_Client_Create` / IREE `iree_hal_driver_create_device` analogue.
    pub fn new(kind: BackendKind) -> Result<Self, Error> {
        let engine = match kind {
            BackendKind::SoftNpu => Engine::SoftNpu(SoftNpuDevice::new(HostDma::new())),
            BackendKind::IreeShaped => Engine::IreeShaped(IreeShapedCp::new(HostDma::new())),
        };
        let mut client = Self {
            kind,
            engine,
            info: AccelInfo {
                vendor: 0,
                device: 0,
                n_queues: 0,
                max_wave: 0,
                backend: 0,
            },
            activity: Activity::new(
                ActivityId(1),
                ActivityKind::SoftNpu,
                aether_core::fabric::EndpointId(1),
            ),
            device: AbiDevice {
                activity: ActivityId(1),
                partition: PartitionId(1),
            },
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
            execs: Vec::new(),
            last_cpl: None,
        };
        client.info = client.engine.probe()?;
        let act_kind = match kind {
            BackendKind::SoftNpu => ActivityKind::SoftNpu,
            BackendKind::IreeShaped => ActivityKind::DeviceAccel,
        };
        client.activity =
            Activity::new(ActivityId(1), act_kind, aether_core::fabric::EndpointId(1))
                .bind_partition(PartitionId(1));
        client.device = AbiDevice {
            activity: client.activity.id,
            partition: PartitionId(1),
        };
        Ok(client)
    }

    pub fn softnpu() -> Result<Self, Error> {
        Self::new(BackendKind::SoftNpu)
    }

    pub fn iree_shaped() -> Result<Self, Error> {
        Self::new(BackendKind::IreeShaped)
    }

    pub fn backend_kind(&self) -> BackendKind {
        self.kind
    }

    pub fn info(&self) -> AccelInfo {
        self.info
    }

    /// PJRT_Device / `iree_hal_device_t` handle published on the fabric.
    pub fn device(&self) -> AbiDevice {
        self.device
    }

    pub fn name(&self) -> &'static str {
        self.engine.name()
    }

    pub fn memory_spaces(&self) -> &'static [MemorySpace] {
        ADDRESSABLE_SPACES
    }

    fn stream(&self) -> StreamId {
        match self.kind {
            BackendKind::SoftNpu => StreamId::from_raw(DEFAULT_STREAM),
            BackendKind::IreeShaped => StreamId::accel(ChipletId(0), TileId(0), IREE_SSID),
        }
    }

    fn device_place(&self, space: MemorySpace) -> Place {
        Place::new(ChipletId(0), space).with_tile(0)
    }

    /// Allocate a buffer in one typed space and pin it through Soft SMMU.
    ///
    /// `unified` is always false here (`MEM_FULL` does not imply
    /// [`CapRights::UNIFIED`](CapRights::UNIFIED)).
    pub fn allocate(&mut self, space: MemorySpace, len: u64) -> Result<BufferId, Error> {
        if len == 0 {
            return Err(Error::Hal(HalError::BadArg));
        }
        if matches!(space, MemorySpace::Scratch | MemorySpace::Streaming) {
            return Err(Error::Space(SpaceError::NotMappable));
        }
        if !ADDRESSABLE_SPACES.contains(&space) {
            return Err(Error::Unsupported);
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
        let place = self.device_place(space);
        let sid = self.stream();
        let iova = self
            .engine
            .map_with_cap(&self.cap, MapRequest::pin_accel(guest_pa, aligned, sid))?;
        self.bump += aligned;
        self.bufs.push(Alloc {
            space,
            place,
            guest_pa,
            iova,
            len,
            unified: false,
        });
        Ok(BufferId(self.bufs.len() as u32))
    }

    pub fn buffer(&self, id: BufferId) -> Result<AbiBuffer, Error> {
        Ok(self.alloc(id)?.as_abi())
    }

    pub fn buffer_iova(&self, id: BufferId) -> Result<PhysAddr, Error> {
        Ok(self.alloc(id)?.iova)
    }

    fn alloc(&self, id: BufferId) -> Result<&Alloc, Error> {
        let i = id.0.checked_sub(1).ok_or(Error::UnknownBuffer)? as usize;
        self.bufs.get(i).ok_or(Error::UnknownBuffer)
    }

    /// Explicit host→device copy (PJRT `BufferFromHostBuffer` / IREE transfer).
    /// Not a coherent load of HBM or tile SRAM.
    pub fn copy_from_host(&mut self, id: BufferId, bytes: &[u8]) -> Result<(), Error> {
        let off = {
            let a = self.alloc(id)?;
            if bytes.len() as u64 > a.len {
                return Err(Error::Hal(HalError::BadArg));
            }
            self.engine.mem().off(a.guest_pa).map_err(Error::Accel)?
        };
        let n = bytes.len();
        self.engine.mem_mut().bytes[off..off + n].copy_from_slice(bytes);
        Ok(())
    }

    /// Explicit device→host copy (PJRT `ToHostBuffer` / IREE transfer).
    pub fn copy_to_host(&self, id: BufferId, out: &mut [u8]) -> Result<(), Error> {
        let a = self.alloc(id)?;
        if out.len() as u64 > a.len {
            return Err(Error::Hal(HalError::BadArg));
        }
        let off = self.engine.mem().off(a.guest_pa).map_err(Error::Accel)?;
        let n = out.len();
        out.copy_from_slice(&self.engine.mem().bytes[off..off + n]);
        Ok(())
    }

    pub fn copy_i32_from_host(&mut self, id: BufferId, vals: &[i32]) -> Result<(), Error> {
        let mut bytes = vec![0u8; vals.len() * 4];
        for (i, v) in vals.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.copy_from_host(id, &bytes)
    }

    pub fn copy_i32_to_host(&self, id: BufferId, out: &mut [i32]) -> Result<(), Error> {
        let mut bytes = vec![0u8; out.len() * 4];
        self.copy_to_host(id, &mut bytes)?;
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = i32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        }
        Ok(())
    }

    /// Load a compiler-owned executable handle. The kernel/host does not
    /// parse an ISA blob; v0.1 stand-in is opcode + dtype.
    ///
    /// On [`BackendKind::IreeShaped`] the frozen blob id is
    /// [`IREE_REF_EXECUTABLE`] (`0x0001EE00`). SoftNPU uses a sequential
    /// host handle (path-B qemu demo).
    pub fn load_executable(&mut self, op: AccelOp, dtype: DType) -> Result<ExecutableId, Error> {
        let isa_blob_id = match self.kind {
            BackendKind::SoftNpu => (self.execs.len() as u32).saturating_add(1),
            BackendKind::IreeShaped => IREE_REF_EXECUTABLE,
        };
        self.load_executable_blob(isa_blob_id, op, dtype)
    }

    /// Same as [`Self::load_executable`] with an explicit `isa_blob_id`.
    /// IreeShaped refuses any id other than [`IREE_REF_EXECUTABLE`].
    pub fn load_executable_blob(
        &mut self,
        isa_blob_id: u32,
        op: AccelOp,
        dtype: DType,
    ) -> Result<ExecutableId, Error> {
        if !matches!(op, AccelOp::Nop | AccelOp::MatMul | AccelOp::Wave) {
            return Err(Error::Unsupported);
        }
        if self.kind == BackendKind::IreeShaped && isa_blob_id != IREE_REF_EXECUTABLE {
            return Err(Error::Unsupported);
        }
        let handle = (self.execs.len() as u32).saturating_add(1);
        self.execs.push(Loaded {
            exec: AbiExecutable {
                activity: self.activity.id,
                isa_blob_id,
            },
            op,
            dtype,
        });
        Ok(ExecutableId(handle))
    }

    pub fn executable(&self, id: ExecutableId) -> Result<AbiExecutable, Error> {
        Ok(self.loaded(id)?.exec)
    }

    fn loaded(&self, id: ExecutableId) -> Result<&Loaded, Error> {
        let i = id.0.checked_sub(1).ok_or(Error::UnknownExecutable)? as usize;
        self.execs.get(i).ok_or(Error::UnknownExecutable)
    }

    /// `PJRT_LoadedExecutable_Execute` / `iree_hal_device_queue_dispatch`.
    ///
    /// Maps `abi::{Device,Buffer,Executable,Event}` onto a frozen
    /// [`IreeHalCmd`] (IreeShaped) or a virtqueue `AccelJobDesc` (SoftNPU).
    /// Does **not** parse graph IR. Completions arrive on [`Self::wait`].
    pub fn execute(&mut self, job: Dispatch) -> Result<AbiEvent, Error> {
        let (op, dtype, exec_abi) = {
            let loaded = self.loaded(job.executable)?;
            (loaded.op, loaded.dtype, loaded.exec)
        };
        if self.kind == BackendKind::IreeShaped && exec_abi.isa_blob_id != IREE_REF_EXECUTABLE {
            return Err(Error::Unsupported);
        }
        if (job.m == 0 || job.n == 0 || job.k == 0) && op != AccelOp::Nop {
            return Err(Error::Accel(AccelError::BadShape));
        }
        let es = dtype.size_bytes() as u64;
        let (space, place, pa_a, pa_b, pa_c, bias_pa, buf_nouns) = {
            let a = self.alloc(job.a)?;
            let b = self.alloc(job.b)?;
            let c = self.alloc(job.c)?;
            if a.space != b.space || a.space != c.space {
                return Err(Error::SpaceMismatch);
            }
            if es * (job.m as u64) * (job.k as u64) > a.len
                || es * (job.k as u64) * (job.n as u64) > b.len
                || es * (job.m as u64) * (job.n as u64) > c.len
            {
                return Err(Error::Hal(HalError::BadArg));
            }
            let (bias_pa, bias_abi) = if let Some(id) = job.bias {
                let bias = self.alloc(id)?;
                if bias.space != c.space {
                    return Err(Error::SpaceMismatch);
                }
                if es * job.n as u64 > bias.len {
                    return Err(Error::Hal(HalError::BadArg));
                }
                (bias.guest_pa, bias.as_abi())
            } else {
                (
                    PhysAddr(0),
                    AbiBuffer::new(c.space, FabricAddr::new(c.place, 0), 0),
                )
            };
            (
                c.space,
                c.place,
                a.guest_pa,
                b.guest_pa,
                c.guest_pa,
                bias_pa,
                [a.as_abi(), b.as_abi(), c.as_abi(), bias_abi],
            )
        };

        let wait_for = job.wait_for.map(|e| e.fence);
        let fence = self.timeline.submit(&self.profile, wait_for)?;

        let mut desc = AccelJobDesc::matmul_i32(job.m, job.n, job.k, pa_a, pa_b, pa_c, 1);
        desc.op = op;
        desc.dtype = dtype;
        desc.bias = bias_pa;
        desc.space = space;
        desc.place = place;
        desc.phase = Phase::Compute;
        desc.partition = self.profile.id;
        desc.fence_id = fence.id.0;
        if self.kind == BackendKind::IreeShaped && stream_for_job(&desc) != self.stream() {
            let _ = self.timeline.timeout(fence.id);
            return Err(Error::Hal(HalError::Fault));
        }

        let event = AbiEvent {
            fence: fence.id,
            partition: self.profile.id,
        };
        let submit = if self.kind == BackendKind::IreeShaped {
            let nouns = IreeHalNouns {
                device: self.device,
                executable: exec_abi,
                event,
                buffers: buf_nouns,
            };
            match self.engine.submit_iree(&desc, &nouns) {
                Ok(_) => {
                    if let Some(cmd) = self.engine.last_iree_cmd() {
                        if let Err(e) = Self::check_frozen(&cmd, &desc) {
                            let _ = self.timeline.timeout(fence.id);
                            return Err(e);
                        }
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            self.engine.submit(&desc).map(|_| ())
        };

        match submit {
            Ok(()) => Ok(event),
            Err(e) => {
                let _ = self.timeline.timeout(fence.id);
                Err(Error::Hal(e))
            }
        }
    }

    /// SpecForge freeze: magic 0xAE7E1EE1, 96-byte LE, ssid=2, backend=4
    /// (probed), DISPATCH or 0, executable 0x0001EE00, workgroup = m,n,k,
    /// binding lengths = dtype-aware byte spans.
    fn check_frozen(cmd: &IreeHalCmd, job: &AccelJobDesc) -> Result<(), Error> {
        cmd.check_v1().map_err(Error::Hal)?;
        let wire = cmd.to_le_bytes();
        if cmd.magic != IREE_HAL_PKT_MAGIC || wire.len() != IREE_HAL_CMD_SIZE {
            return Err(Error::FrozenImage);
        }
        if StreamId::from_raw(cmd.stream_id).ssid() != IREE_SSID {
            return Err(Error::FrozenImage);
        }
        if job.op != AccelOp::Nop {
            if cmd.workgroup_count_x != job.m
                || cmd.workgroup_count_y != job.n
                || cmd.workgroup_count_z != job.k
            {
                return Err(Error::FrozenImage);
            }
            let es = job.elem_bytes().max(1);
            if cmd.binding0_length as u64 != job.bytes_a().max(es)
                || cmd.binding1_length as u64 != job.bytes_b().max(es)
                || cmd.binding2_length as u64 != job.bytes_c().max(es)
            {
                return Err(Error::FrozenImage);
            }
        }
        Ok(())
    }

    pub fn last_iree_cmd(&self) -> Option<IreeHalCmd> {
        self.engine.last_iree_cmd()
    }

    /// `PJRT_Event_Await` / `iree_hal_fence_wait`.
    ///
    /// Host tests have no device IRQ thread, so this pumps `service()`
    /// (SoftNPU used-ring or IreeShapedCp mailbox) then retires the timeline.
    pub fn wait(&mut self, event: AbiEvent) -> Result<Completion, Error> {
        if event.partition.0 != self.profile.id.0 {
            return Err(Error::Partition(PartitionError::Unbound));
        }
        if self.timeline.wait(event.fence).is_ok() {
            return self.last_cpl.ok_or(Error::NotReady);
        }
        let serviced = self.engine.service().ok_or(Error::NotReady)?;
        let cpl = self.engine.poll().unwrap_or(serviced);
        self.engine.retire_into(&mut self.timeline)?;
        self.timeline.wait(event.fence)?;
        self.last_cpl = Some(cpl);
        if cpl.status != 0 {
            return Err(Error::JobFault(cpl.status));
        }
        Ok(cpl)
    }

    /// Fence watermark without pumping the device (must be `FenceNotReady`
    /// until [`Self::wait`] retires the seq).
    pub fn fence_ready(&self, event: AbiEvent) -> bool {
        self.timeline.wait(event.fence).is_ok()
    }

    pub fn translate(&self, guest_pa: PhysAddr) -> Option<PhysAddr> {
        self.engine.translate_stream(self.stream().raw(), guest_pa)
    }
}

/// One compiled dispatch: bindings + shape. Not a graph.
#[derive(Clone, Copy, Debug)]
pub struct Dispatch {
    pub executable: ExecutableId,
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub a: BufferId,
    pub b: BufferId,
    pub c: BufferId,
    pub bias: Option<BufferId>,
    pub wait_for: Option<AbiEvent>,
}

impl Dispatch {
    pub fn matmul(
        executable: ExecutableId,
        m: u32,
        n: u32,
        k: u32,
        a: BufferId,
        b: BufferId,
        c: BufferId,
    ) -> Self {
        Self {
            executable,
            m,
            n,
            k,
            a,
            b,
            c,
            bias: None,
            wait_for: None,
        }
    }

    pub fn wave(
        executable: ExecutableId,
        m: u32,
        n: u32,
        k: u32,
        a: BufferId,
        b: BufferId,
        c: BufferId,
        bias: BufferId,
    ) -> Self {
        Self {
            executable,
            m,
            n,
            k,
            a,
            b,
            c,
            bias: Some(bias),
            wait_for: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::iommu::SOFT_SMMU_IOVA_BASE;
    use aether_hal::{
        ACCEL_BACKEND_IREE_SHAPED, ACCEL_BACKEND_PARTNER_STUB, ACCEL_BACKEND_SOFTNPU,
        ACCEL_BACKEND_SOFT_CP, ACCEL_BACKEND_VIRTIO_SOFTNPU,
    };

    fn each_backend() -> [BackendKind; 2] {
        [BackendKind::SoftNpu, BackendKind::IreeShaped]
    }

    #[test]
    fn create_device_probes_real_backends() {
        for kind in each_backend() {
            let c = Client::new(kind).unwrap();
            match kind {
                BackendKind::SoftNpu => {
                    assert_eq!(c.info().backend, ACCEL_BACKEND_VIRTIO_SOFTNPU);
                    assert_eq!(c.device().activity, ActivityId(1));
                    assert!(c.name().contains("softnpu"));
                }
                BackendKind::IreeShaped => {
                    assert_eq!(c.info().backend, ACCEL_BACKEND_IREE_SHAPED);
                    assert_eq!(c.name(), "iree-shaped-cp");
                }
            }
            assert_ne!(c.info().backend, ACCEL_BACKEND_SOFTNPU);
            assert_ne!(c.info().backend, ACCEL_BACKEND_PARTNER_STUB);
            assert_ne!(c.info().backend, ACCEL_BACKEND_SOFT_CP);
            assert_eq!(c.device().partition, PartitionId(1));
            assert_eq!(c.memory_spaces(), ADDRESSABLE_SPACES);
        }
    }

    #[test]
    fn allocate_places_are_typed_and_not_unified() {
        let mut c = Client::softnpu().unwrap();
        let host = c.allocate(MemorySpace::Host, 64).unwrap();
        let hbm = c.allocate(MemorySpace::DeviceHbm, 64).unwrap();
        let sram = c.allocate(MemorySpace::TileSram, 64).unwrap();
        let hb = c.buffer(host).unwrap();
        let db = c.buffer(hbm).unwrap();
        let sb = c.buffer(sram).unwrap();
        assert_eq!(hb.space, MemorySpace::Host);
        assert_eq!(db.space, MemorySpace::DeviceHbm);
        assert_eq!(sb.space, MemorySpace::TileSram);
        assert!(!hb.unified);
        assert!(!db.unified);
        assert!(!sb.unified);
        assert_eq!(hb.addr.place.space, MemorySpace::Host);
        assert_eq!(db.addr.place.space, MemorySpace::DeviceHbm);
        assert!(matches!(
            c.allocate(MemorySpace::Streaming, 16).unwrap_err(),
            Error::Space(SpaceError::NotMappable)
        ));
        assert!(matches!(
            c.allocate(MemorySpace::Scratch, 16).unwrap_err(),
            Error::Space(SpaceError::NotMappable)
        ));
    }

    #[test]
    fn soft_smmu_iova_is_not_identity() {
        for kind in each_backend() {
            let mut c = Client::new(kind).unwrap();
            let buf = c.allocate(MemorySpace::Host, 256).unwrap();
            let abi = c.buffer(buf).unwrap();
            let iova = c.buffer_iova(buf).unwrap();
            assert!(iova.0 >= SOFT_SMMU_IOVA_BASE);
            assert_ne!(iova.0, abi.addr.local);
            let translated = c.translate(PhysAddr(abi.addr.local)).unwrap();
            assert_eq!(translated, iova);
        }
    }

    fn matmul_2x2(c: &mut Client, space: MemorySpace) -> [i32; 4] {
        let a = c.allocate(space, 16).unwrap();
        let b = c.allocate(space, 16).unwrap();
        let out = c.allocate(space, 16).unwrap();
        c.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
        c.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
        let exec = c.load_executable(AccelOp::MatMul, DType::I32).unwrap();
        let ev = c
            .execute(Dispatch::matmul(exec, 2, 2, 2, a, b, out))
            .unwrap();
        assert!(!c.fence_ready(ev), "submit does not execute");
        let cpl = c.wait(ev).unwrap();
        assert_eq!(cpl.status, 0);
        assert!(c.fence_ready(ev));
        let mut got = [0i32; 4];
        c.copy_i32_to_host(out, &mut got).unwrap();
        got
    }

    #[test]
    fn submit_matmul_wait_event() {
        for kind in each_backend() {
            let mut c = Client::new(kind).unwrap();
            let got = matmul_2x2(&mut c, MemorySpace::Host);
            assert_eq!(got, [19, 22, 43, 50], "{kind:?}");
        }
    }

    #[test]
    fn submit_matmul_on_tile_sram_place() {
        for kind in each_backend() {
            let mut c = Client::new(kind).unwrap();
            let got = matmul_2x2(&mut c, MemorySpace::TileSram);
            assert_eq!(got, [19, 22, 43, 50], "{kind:?}");
        }
    }

    #[test]
    fn submit_wave_wait_event() {
        for kind in each_backend() {
            let mut c = Client::new(kind).unwrap();
            let a = c.allocate(MemorySpace::Host, 16).unwrap();
            let b = c.allocate(MemorySpace::Host, 16).unwrap();
            let out = c.allocate(MemorySpace::Host, 16).unwrap();
            let bias = c.allocate(MemorySpace::Host, 8).unwrap();
            // A = I2, B = [1,2; 3,4], bias = [10, 20]
            c.copy_i32_from_host(a, &[1, 0, 0, 1]).unwrap();
            c.copy_i32_from_host(b, &[1, 2, 3, 4]).unwrap();
            c.copy_i32_from_host(bias, &[10, 20]).unwrap();
            let exec = c.load_executable(AccelOp::Wave, DType::I32).unwrap();
            let ev = c
                .execute(Dispatch::wave(exec, 2, 2, 2, a, b, out, bias))
                .unwrap();
            c.wait(ev).unwrap();
            let mut got = [0i32; 4];
            c.copy_i32_to_host(out, &mut got).unwrap();
            assert_eq!(got, [11, 22, 13, 24], "{kind:?}");
        }
    }

    #[test]
    fn mixed_spaces_are_not_unified() {
        let mut c = Client::iree_shaped().unwrap();
        let a = c.allocate(MemorySpace::Host, 16).unwrap();
        let b = c.allocate(MemorySpace::Host, 16).unwrap();
        let out = c.allocate(MemorySpace::DeviceHbm, 16).unwrap();
        let exec = c.load_executable(AccelOp::MatMul, DType::I32).unwrap();
        assert_eq!(
            c.execute(Dispatch::matmul(exec, 2, 2, 2, a, b, out))
                .unwrap_err(),
            Error::SpaceMismatch
        );
    }

    #[test]
    fn executable_is_an_opaque_handle() {
        let mut c = Client::softnpu().unwrap();
        let id = c.load_executable(AccelOp::MatMul, DType::F32).unwrap();
        let e = c.executable(id).unwrap();
        assert_eq!(e.activity, ActivityId(1));
        assert_eq!(e.isa_blob_id, 1);
    }

    #[test]
    fn iree_shaped_uses_frozen_ref_executable() {
        let mut c = Client::iree_shaped().unwrap();
        let id = c.load_executable(AccelOp::MatMul, DType::I32).unwrap();
        assert_eq!(c.executable(id).unwrap().isa_blob_id, IREE_REF_EXECUTABLE);
        assert_eq!(
            c.load_executable_blob(0xDEAD, AccelOp::MatMul, DType::I32)
                .unwrap_err(),
            Error::Unsupported
        );
        assert_eq!(c.info().backend, ACCEL_BACKEND_IREE_SHAPED);
    }

    #[test]
    fn iree_execute_packs_frozen_hal_image() {
        use aether_drivers::ireecp::{
            IREE_HAL_COMMAND_CATEGORY_DISPATCH, IREE_HAL_ELEMENT_TYPE_INT_32,
        };

        let mut c = Client::iree_shaped().unwrap();
        let a = c.allocate(MemorySpace::Host, 16).unwrap();
        let b = c.allocate(MemorySpace::Host, 16).unwrap();
        let out = c.allocate(MemorySpace::Host, 16).unwrap();
        c.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
        c.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
        let exec = c.load_executable(AccelOp::MatMul, DType::I32).unwrap();
        let ev = c
            .execute(Dispatch::matmul(exec, 2, 2, 2, a, b, out))
            .unwrap();
        let cmd = c.last_iree_cmd().unwrap();
        assert_eq!(cmd.magic, IREE_HAL_PKT_MAGIC);
        assert_eq!(cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(cmd.command_categories, IREE_HAL_COMMAND_CATEGORY_DISPATCH);
        assert_eq!(cmd.executable, IREE_REF_EXECUTABLE);
        assert_eq!(cmd.workgroup_count_x, 2);
        assert_eq!(cmd.workgroup_count_y, 2);
        assert_eq!(cmd.workgroup_count_z, 2);
        assert_eq!(cmd.binding0_length, 16, "I32 2×2 is 16 bytes, not 4 elems");
        assert_ne!(cmd.binding0_length, 4);
        assert_eq!(cmd.element_type, IREE_HAL_ELEMENT_TYPE_INT_32);
        assert_eq!(
            aether_core::iommu::StreamId::from_raw(cmd.stream_id).ssid(),
            IREE_SSID
        );
        c.wait(ev).unwrap();
    }

    #[test]
    fn iree_nop_categories_zero_ignores_function() {
        let mut c = Client::iree_shaped().unwrap();
        let a = c.allocate(MemorySpace::Host, 16).unwrap();
        let b = c.allocate(MemorySpace::Host, 16).unwrap();
        let out = c.allocate(MemorySpace::Host, 16).unwrap();
        let exec = c.load_executable(AccelOp::Nop, DType::I32).unwrap();
        let ev = c
            .execute(Dispatch::matmul(exec, 0, 0, 0, a, b, out))
            .unwrap();
        let cmd = c.last_iree_cmd().unwrap();
        assert_eq!(cmd.command_categories, 0);
        assert_eq!(cmd.function, 0);
        assert_eq!(cmd.decode_op().unwrap(), AccelOp::Nop);
        c.wait(ev).unwrap();
    }

    #[test]
    fn iree_workgroup_counts_are_job_shape_not_tiles() {
        let mut c = Client::iree_shaped().unwrap();
        // 3×4×5 I32: A=3×5×4=60, B=5×4×4=80, C=3×4×4=48 bytes.
        let a = c.allocate(MemorySpace::Host, 64).unwrap();
        let b = c.allocate(MemorySpace::Host, 80).unwrap();
        let out = c.allocate(MemorySpace::Host, 64).unwrap();
        let exec = c.load_executable(AccelOp::MatMul, DType::I32).unwrap();
        c.execute(Dispatch::matmul(exec, 3, 4, 5, a, b, out))
            .unwrap();
        let cmd = c.last_iree_cmd().unwrap();
        assert_eq!(cmd.workgroup_count_x, 3);
        assert_eq!(cmd.workgroup_count_y, 4);
        assert_eq!(cmd.workgroup_count_z, 5);
        assert_eq!(cmd.binding0_length, 60);
        assert_eq!(cmd.binding1_length, 80);
        assert_eq!(cmd.binding2_length, 48);
    }
}
