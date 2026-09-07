//! IREE HAL-shaped command processor (`IreeShapedCp`).
//!
//! Partner-shaped AccelDevice spine: packs a **frozen** dispatch packet
//! whose field names come from the public IREE HAL
//! (`iree_hal_device_queue_dispatch` / `iree_hal_command_buffer_dispatch`
//! in [iree-org/iree](https://github.com/iree-org/iree)
//! `runtime/src/iree/hal/{device,command_buffer,buffer_view,semaphore}.h`).
//!
//! Chosen source (see `docs/ACCEL.md`): IREE HAL Device / Buffer /
//! Executable / Event nouns already sketched in `aether_core::abi`, not
//! a TT-Metal descriptor and not a fabricated NVIDIA opcode list.
//!
//! This is **not**:
//! - Soft-CP 2.0 (`CpCmd` still uses Aether-native `AccelOp` bytes)
//! - `PartnerNpuStub` enrichment theater
//! - a signed IREE or silicon partnership
//! - an IREE VM / compiler in the kernel
//!
//! Completions arrive on IRQ/poll. DMA uses Soft-SMMU IOVAs only
//! (`ssid = IREE_SSID`, distinct from SoftNPU 0 and Soft-CP 1).
//! SoftChipletSync + SoftCCT are optional on this mailbox (same software
//! fence domains as Soft-CP). Not a Vulkan / ROCm product; still a
//! single mailbox.

use aether_core::abi::{Buffer, Device, Event, Executable};
use aether_core::accel::{AccelJobDesc, AccelOp, Completion, DType, DmaView, SoftNpu};
use aether_core::activity::ActivityId;
use aether_core::caps::Capability;
use aether_core::chipsync::{BufferLabel, ScopedFence, ScopedWork, SoftChipletSync, SyncScope};
use aether_core::fence::{Fence, FenceId, Timeline};
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::partition::{PartitionError, PartitionId};
use aether_core::space::{FabricAddr, Place};
use aether_core::types::{ChipletId, PhysAddr, TileId};
use aether_hal::{AccelDevice, AccelInfo, HalError, ACCEL_BACKEND_IREE_SHAPED};

/// Packet magic (`AE7E` + IREE-shaped `1EE1`). Distinct from `CpCmd` `0xAE7E0C01`.
pub const IREE_HAL_PKT_MAGIC: u32 = 0xAE7E_1EE1;
pub const IREE_HAL_CMD_SIZE: usize = 96;
/// Soft-SMMU substream. Distinct from SoftNPU (`ssid` 0) and Soft-CP (`ssid` 1).
pub const IREE_SSID: u8 = 2;

/// Frozen `abi::Executable.isa_blob_id` this software CP can dispatch.
/// Opaque handle — the kernel does not parse IREE VM bytecode.
pub const IREE_REF_EXECUTABLE: u32 = 0x0001_EE00;

/// Public IREE `iree_hal_command_category_t` bits
/// (`runtime/src/iree/hal/command_buffer.h`).
pub const IREE_HAL_COMMAND_CATEGORY_TRANSFER: u16 = 1 << 0;
pub const IREE_HAL_COMMAND_CATEGORY_DISPATCH: u16 = 1 << 1;

/// IREE `iree_hal_executable_function_t` export ordinals on
/// [`IREE_REF_EXECUTABLE`]. These are **not** [`AccelOp`] values.
/// First real dispatch export is 0 (IREE convention); fused wave is 1.
pub const HAL_FN_MATMUL: u32 = 0;
pub const HAL_FN_FUSED: u32 = 1;

/// IREE `iree_hal_element_type_t` packing from
/// `runtime/src/iree/hal/buffer_view.h`:
/// `IREE_HAL_ELEMENT_TYPE_VALUE(numerical_type, bit_count) =
///  (numerical_type << 24) | bit_count`.
pub const IREE_HAL_NUMERICAL_TYPE_INTEGER: u32 = 0x10;
pub const IREE_HAL_NUMERICAL_TYPE_FLOAT_IEEE: u32 = 0x21;
/// `IREE_HAL_ELEMENT_TYPE_INT_32` (signless i32).
pub const IREE_HAL_ELEMENT_TYPE_INT_32: u32 = (IREE_HAL_NUMERICAL_TYPE_INTEGER << 24) | 32;
/// `IREE_HAL_ELEMENT_TYPE_FLOAT_16`.
pub const IREE_HAL_ELEMENT_TYPE_FLOAT_16: u32 = (IREE_HAL_NUMERICAL_TYPE_FLOAT_IEEE << 24) | 16;
/// `IREE_HAL_ELEMENT_TYPE_FLOAT_32`.
pub const IREE_HAL_ELEMENT_TYPE_FLOAT_32: u32 = (IREE_HAL_NUMERICAL_TYPE_FLOAT_IEEE << 24) | 32;

/// Pack the IREE-shaped stream from a job's fabric place.
pub fn stream_for_job(job: &AccelJobDesc) -> StreamId {
    StreamId::accel(
        job.place.chiplet,
        TileId(job.place.tile.unwrap_or(0)),
        IREE_SSID,
    )
}

/// IREE `iree_hal_queue_affinity_t` stand-in (low 32 bits): chiplet in
/// `[31:16]`, tile in `[15:0]`. Not IREE's physical-device bitmask.
pub fn queue_affinity_from_place(place: Place) -> u32 {
    ((place.chiplet.0 as u32) << 16) | (place.tile.unwrap_or(0) as u32)
}

pub fn element_type_from_dtype(dtype: DType) -> u32 {
    match dtype {
        DType::I32 => IREE_HAL_ELEMENT_TYPE_INT_32,
        DType::F16 => IREE_HAL_ELEMENT_TYPE_FLOAT_16,
        DType::F32 => IREE_HAL_ELEMENT_TYPE_FLOAT_32,
    }
}

/// v1 pack emits **0** (Nop doorbell) or **DISPATCH** only.
/// `TRANSFER` alone is not a defined v1 packet.
pub fn categories_from_op(op: AccelOp) -> u16 {
    match op {
        AccelOp::Nop => 0,
        AccelOp::MatMul | AccelOp::Wave => IREE_HAL_COMMAND_CATEGORY_DISPATCH,
    }
}

pub fn function_from_op(op: AccelOp) -> u32 {
    match op {
        AccelOp::Nop | AccelOp::MatMul => HAL_FN_MATMUL,
        AccelOp::Wave => HAL_FN_FUSED,
    }
}

/// Decode keys off `command_categories` first. Do **not** branch on
/// `function` until DISPATCH is set. Nop is `categories = 0`; `function`
/// is ignored (pack writes 0). TRANSFER alone / any other v1 category
/// bit pattern is [`HalError::Fault`].
pub fn op_from_hal(categories: u16, function: u32) -> Result<AccelOp, HalError> {
    match categories {
        0 => Ok(AccelOp::Nop),
        IREE_HAL_COMMAND_CATEGORY_DISPATCH => Ok(if function == HAL_FN_FUSED {
            AccelOp::Wave
        } else {
            AccelOp::MatMul
        }),
        _ => Err(HalError::Fault),
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
        | MapError::Stage2Fault
        | MapError::SubmitSid => HalError::Fault,
        MapError::SidBudget => HalError::Busy,
    }
}

/// Host nouns a PJRT / IREE runtime would present, filled from one job.
/// Types only — not a graph IR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IreeHalNouns {
    pub device: Device,
    pub executable: Executable,
    pub event: Event,
    pub buffers: [Buffer; 4],
}

impl IreeHalNouns {
    pub fn from_job(job: &AccelJobDesc) -> Self {
        let place = job.place;
        let mk = |local: u64, len: u64| Buffer::new(job.space, FabricAddr::new(place, local), len);
        Self {
            device: Device {
                activity: ActivityId(job.completion_ep),
                partition: job.partition,
            },
            executable: Executable {
                activity: ActivityId(job.completion_ep),
                isa_blob_id: IREE_REF_EXECUTABLE,
            },
            event: Event {
                fence: FenceId(job.fence_id),
                partition: job.partition,
            },
            buffers: [
                mk(job.a.0, job.bytes_a()),
                mk(job.b.0, job.bytes_b()),
                mk(job.c.0, job.bytes_c()),
                mk(
                    job.bias.0,
                    if job.bias.0 != 0 {
                        job.elem_bytes().saturating_mul(job.n as u64)
                    } else {
                        0
                    },
                ),
            ],
        }
    }
}

/// Frozen 96-byte IREE HAL dispatch packet. Layout is the architectural
/// contract; see `docs/ACCEL.md`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IreeHalCmd {
    pub magic: u32,
    /// `iree_hal_command_category_t` (TRANSFER / DISPATCH). Not `AccelOp`.
    pub command_categories: u16,
    /// `iree_hal_buffer_ref_list_t.count` (0–4).
    pub binding_count: u16,
    /// `iree_hal_executable_t` handle (`Executable.isa_blob_id`).
    pub executable: u32,
    /// `iree_hal_executable_function_t` export ordinal. Not `AccelOp`.
    pub function: u32,
    /// AccelJobDesc `m,n,k` shape stand-ins — **not** compiler tile
    /// sizes or IREE launch geometry.
    pub workgroup_count_x: u32,
    pub workgroup_count_y: u32,
    pub workgroup_count_z: u32,
    /// `iree_hal_element_type_t`.
    pub element_type: u32,
    /// Low 32 of `iree_hal_queue_affinity_t` (Place chiplet/tile).
    pub queue_affinity: u32,
    /// Soft-SMMU packed [`StreamId`] (Aether pin, not an IREE field).
    pub stream_id: u32,
    /// `iree_hal_buffer_ref_t.offset` for bindings 0–3 (Soft-SMMU IOVA).
    pub binding0_offset: u64,
    pub binding1_offset: u64,
    pub binding2_offset: u64,
    pub binding3_offset: u64,
    /// `iree_hal_buffer_ref_t.length` for bindings 0–3. Byte spans
    /// (`dtype` × elements), not element counts.
    pub binding0_length: u32,
    pub binding1_length: u32,
    pub binding2_length: u32,
    pub binding3_length: u32,
    /// `iree_hal_semaphore_t` signal payload (`Event.fence` / `fence_id`).
    pub signal_payload: u64,
}

const _: [(); IREE_HAL_CMD_SIZE] = [(); core::mem::size_of::<IreeHalCmd>()];

impl IreeHalCmd {
    /// Frozen v1 image: magic, `executable = IREE_REF_EXECUTABLE`,
    /// categories ∈ {0, DISPATCH}.
    pub fn check_v1(&self) -> Result<(), HalError> {
        if self.magic != IREE_HAL_PKT_MAGIC {
            return Err(HalError::BadArg);
        }
        if self.executable != IREE_REF_EXECUTABLE {
            return Err(HalError::Unsupported);
        }
        match self.command_categories {
            0 | IREE_HAL_COMMAND_CATEGORY_DISPATCH => Ok(()),
            _ => Err(HalError::Fault),
        }
    }

    pub fn decode_op(&self) -> Result<AccelOp, HalError> {
        self.check_v1()?;
        op_from_hal(self.command_categories, self.function)
    }

    /// Translate an Aether job through Soft SMMU into an IREE HAL packet.
    ///
    /// `Nop` is a doorbell / latency probe (`command_categories = 0`) and
    /// does not require pins. Every other op refuses unless the job's
    /// packed SID is Bound and A/B/C (and bias, if set) translate on that SID.
    pub fn pack(job: &AccelJobDesc, iommu: &IommuMap) -> Result<Self, HalError> {
        let nouns = IreeHalNouns::from_job(job);
        Self::pack_with_nouns(job, iommu, &nouns)
    }

    /// PJRT path: `abi::{Device,Buffer,Executable,Event}` → frozen image.
    /// Refuses any `isa_blob_id` other than [`IREE_REF_EXECUTABLE`].
    pub fn pack_with_nouns(
        job: &AccelJobDesc,
        iommu: &IommuMap,
        nouns: &IreeHalNouns,
    ) -> Result<Self, HalError> {
        if nouns.executable.isa_blob_id != IREE_REF_EXECUTABLE {
            return Err(HalError::Unsupported);
        }
        let sid = iommu.submit_sid().unwrap_or_else(|| stream_for_job(job));
        Self::pack_on(job, iommu, nouns, sid)
    }

    /// Pack on an explicit SET_SID (job-head StreamID).
    pub fn pack_on(
        job: &AccelJobDesc,
        iommu: &IommuMap,
        nouns: &IreeHalNouns,
        sid: StreamId,
    ) -> Result<Self, HalError> {
        if nouns.executable.isa_blob_id != IREE_REF_EXECUTABLE {
            return Err(HalError::Unsupported);
        }
        if job.op == AccelOp::Nop {
            return Ok(Self::empty_from(job, nouns));
        }
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
        let bias_bytes = es.saturating_mul(job.n as u64).max(es);
        if job.bias.0 != 0 && !iommu.covers_stream(sid.raw(), job.bias, bias_bytes) {
            return Err(HalError::Fault);
        }
        if a_bytes > u32::MAX as u64 || b_bytes > u32::MAX as u64 || c_bytes > u32::MAX as u64 {
            return Err(HalError::BadArg);
        }
        let mut binding_count = 3u16;
        let mut b3_len = 0u32;
        if job.bias.0 != 0 {
            binding_count = 4;
            b3_len = bias_bytes as u32;
        }
        Ok(Self {
            magic: IREE_HAL_PKT_MAGIC,
            command_categories: categories_from_op(job.op),
            binding_count,
            executable: nouns.executable.isa_blob_id,
            function: function_from_op(job.op),
            workgroup_count_x: job.m,
            workgroup_count_y: job.n,
            workgroup_count_z: job.k,
            element_type: element_type_from_dtype(job.dtype),
            queue_affinity: queue_affinity_from_place(job.place),
            stream_id: sid.raw(),
            binding0_offset: a.0,
            binding1_offset: b.0,
            binding2_offset: c.0,
            binding3_offset: bias.0,
            binding0_length: a_bytes as u32,
            binding1_length: b_bytes as u32,
            binding2_length: c_bytes as u32,
            binding3_length: b3_len,
            signal_payload: nouns.event.fence.0,
        })
    }

    fn empty_from(job: &AccelJobDesc, nouns: &IreeHalNouns) -> Self {
        let sid = stream_for_job(job);
        Self {
            magic: IREE_HAL_PKT_MAGIC,
            command_categories: 0,
            binding_count: 0,
            executable: nouns.executable.isa_blob_id,
            function: HAL_FN_MATMUL, // 0; ignored when categories = 0
            workgroup_count_x: 0,
            workgroup_count_y: 0,
            workgroup_count_z: 0,
            element_type: element_type_from_dtype(job.dtype),
            queue_affinity: queue_affinity_from_place(job.place),
            stream_id: sid.raw(),
            binding0_offset: 0,
            binding1_offset: 0,
            binding2_offset: 0,
            binding3_offset: 0,
            binding0_length: 0,
            binding1_length: 0,
            binding2_length: 0,
            binding3_length: 0,
            signal_payload: nouns.event.fence.0,
        }
    }

    /// Little-endian wire image a HAL backend would DMA from the mailbox.
    pub fn to_le_bytes(self) -> [u8; IREE_HAL_CMD_SIZE] {
        let mut b = [0u8; IREE_HAL_CMD_SIZE];
        b[0..4].copy_from_slice(&self.magic.to_le_bytes());
        b[4..6].copy_from_slice(&self.command_categories.to_le_bytes());
        b[6..8].copy_from_slice(&self.binding_count.to_le_bytes());
        b[8..12].copy_from_slice(&self.executable.to_le_bytes());
        b[12..16].copy_from_slice(&self.function.to_le_bytes());
        b[16..20].copy_from_slice(&self.workgroup_count_x.to_le_bytes());
        b[20..24].copy_from_slice(&self.workgroup_count_y.to_le_bytes());
        b[24..28].copy_from_slice(&self.workgroup_count_z.to_le_bytes());
        b[28..32].copy_from_slice(&self.element_type.to_le_bytes());
        b[32..36].copy_from_slice(&self.queue_affinity.to_le_bytes());
        b[36..40].copy_from_slice(&self.stream_id.to_le_bytes());
        b[40..48].copy_from_slice(&self.binding0_offset.to_le_bytes());
        b[48..56].copy_from_slice(&self.binding1_offset.to_le_bytes());
        b[56..64].copy_from_slice(&self.binding2_offset.to_le_bytes());
        b[64..72].copy_from_slice(&self.binding3_offset.to_le_bytes());
        b[72..76].copy_from_slice(&self.binding0_length.to_le_bytes());
        b[76..80].copy_from_slice(&self.binding1_length.to_le_bytes());
        b[80..84].copy_from_slice(&self.binding2_length.to_le_bytes());
        b[84..88].copy_from_slice(&self.binding3_length.to_le_bytes());
        b[88..96].copy_from_slice(&self.signal_payload.to_le_bytes());
        b
    }

    /// Inverse of [`Self::to_le_bytes`]. SoftCmdFirewall-shaped: parse the
    /// kernel copy, not a live userspace alias.
    pub fn from_le_bytes(b: [u8; IREE_HAL_CMD_SIZE]) -> Result<Self, HalError> {
        Ok(Self {
            magic: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            command_categories: u16::from_le_bytes(b[4..6].try_into().unwrap()),
            binding_count: u16::from_le_bytes(b[6..8].try_into().unwrap()),
            executable: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            function: u32::from_le_bytes(b[12..16].try_into().unwrap()),
            workgroup_count_x: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            workgroup_count_y: u32::from_le_bytes(b[20..24].try_into().unwrap()),
            workgroup_count_z: u32::from_le_bytes(b[24..28].try_into().unwrap()),
            element_type: u32::from_le_bytes(b[28..32].try_into().unwrap()),
            queue_affinity: u32::from_le_bytes(b[32..36].try_into().unwrap()),
            stream_id: u32::from_le_bytes(b[36..40].try_into().unwrap()),
            binding0_offset: u64::from_le_bytes(b[40..48].try_into().unwrap()),
            binding1_offset: u64::from_le_bytes(b[48..56].try_into().unwrap()),
            binding2_offset: u64::from_le_bytes(b[56..64].try_into().unwrap()),
            binding3_offset: u64::from_le_bytes(b[64..72].try_into().unwrap()),
            binding0_length: u32::from_le_bytes(b[72..76].try_into().unwrap()),
            binding1_length: u32::from_le_bytes(b[76..80].try_into().unwrap()),
            binding2_length: u32::from_le_bytes(b[80..84].try_into().unwrap()),
            binding3_length: u32::from_le_bytes(b[84..88].try_into().unwrap()),
            signal_payload: u64::from_le_bytes(b[88..96].try_into().unwrap()),
        })
    }
}

/// Software IREE-shaped command processor. Completions arrive on the
/// IRQ/poll path, never inside [`AccelDevice::submit`].
pub struct IreeShapedCp<M: DmaView> {
    pub info: AccelInfo,
    pub iommu: IommuMap,
    pub mem: M,
    npu: SoftNpu,
    mailbox: Option<IreeHalCmd>,
    pending_job: Option<AccelJobDesc>,
    doorbell: bool,
    irq: bool,
    last_cmd: Option<IreeHalCmd>,
    last_cpl: Option<Completion>,
    last_fence: Option<u64>,
    fence_done: bool,
    pub chipsync: SoftChipletSync,
    pending_scoped: Option<ScopedWork>,
    last_scoped: Option<ScopedFence>,
}

impl<M: DmaView> IreeShapedCp<M> {
    pub fn new(mem: M) -> Self {
        Self {
            info: AccelInfo {
                vendor: 0xAE7E,
                device: 0x0004,
                n_queues: 1,
                max_wave: 64,
                backend: ACCEL_BACKEND_IREE_SHAPED,
                sm_count: 0,
                wq_count: 0,
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
            chipsync: SoftChipletSync::new(PartitionId(1)),
            pending_scoped: None,
            last_scoped: None,
        }
    }

    pub fn bind_stream(
        &mut self,
        cap: &Capability,
        sid: StreamId,
    ) -> Result<aether_core::iommu::StreamState, HalError> {
        self.iommu.bind_stream(cap, sid).map_err(map_hal_error)
    }

    /// Privileged Host1x-shaped SET_SID (Memory+MAP). Programs Soft SMMU
    /// StreamID at the job head. Not a Tegra class opcode.
    pub fn set_sid(&mut self, cap: &Capability, sid: StreamId) -> Result<StreamId, HalError> {
        self.iommu.set_sid(cap, sid).map_err(map_hal_error)
    }

    /// Fault injection: overwrite mailbox StreamID after SET_SID.
    pub fn inject_wrong_sid(&mut self, sid: StreamId) {
        if let Some(cmd) = self.mailbox.as_mut() {
            cmd.stream_id = sid.raw();
        }
    }

    fn arm_set_sid(&mut self, job: &AccelJobDesc) -> Result<StreamId, HalError> {
        if job.op == AccelOp::Nop {
            return Ok(self
                .iommu
                .submit_sid()
                .unwrap_or_else(|| stream_for_job(job)));
        }
        if let Some(sid) = self.iommu.submit_sid() {
            return Ok(sid);
        }
        let sid = stream_for_job(job);
        self.iommu.set_sid_bound(sid).map_err(map_hal_error)?;
        Ok(sid)
    }

    pub fn map_with_cap(
        &mut self,
        cap: &Capability,
        req: MapRequest,
    ) -> Result<PhysAddr, HalError> {
        let region = self.iommu.map(cap, req).map_err(map_hal_error)?;
        Ok(region.iova)
    }

    /// Consume a frozen `IreeHalCmd` image (PJRT / IREE HAL path).
    /// `AccelDevice::submit` packs then calls this; the job record is
    /// kept for guest-PA DMA after IOVA resolve.
    pub fn submit_hal(&mut self, cmd: IreeHalCmd, job: &AccelJobDesc) -> Result<u32, HalError> {
        if self.doorbell || self.mailbox.is_some() {
            return Err(HalError::Busy);
        }
        // Copy-then-validate the frozen image (same Host1x lesson as Soft-CP).
        let cmd = IreeHalCmd::from_le_bytes(cmd.to_le_bytes())?;
        let op = cmd.decode_op()?;
        if op != AccelOp::Nop {
            if let Err(e) = self
                .iommu
                .set_sid_bound(StreamId::from_raw(cmd.stream_id))
                .map_err(map_hal_error)
            {
                return Err(e);
            }
        }
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
        self.pending_scoped = None;
        Ok(self.npu.seq)
    }

    pub fn last_cmd(&self) -> Option<IreeHalCmd> {
        self.last_cmd
    }

    pub fn doorbell_pending(&self) -> bool {
        self.doorbell
    }

    pub fn irq_pending(&self) -> bool {
        self.irq
    }

    /// Semaphore payload retired by the last IRQ, if the job named one.
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

    /// Mailbox submit tagged with SoftChipletSync. Not a second IR.
    /// IreeShapedCp stays a single mailbox; two fake chiplets are sequential.
    pub fn submit_scoped(
        &mut self,
        job: &AccelJobDesc,
        scope: SyncScope,
        write: Option<BufferLabel>,
        read: Option<BufferLabel>,
    ) -> Result<u32, HalError> {
        let seq = AccelDevice::submit(self, job)?;
        self.chipsync.set_scope(scope);
        self.pending_scoped = Some(ScopedWork { scope, write, read });
        Ok(seq)
    }

    pub fn last_scoped(&self) -> Option<ScopedFence> {
        self.last_scoped
    }

    /// Device-side: consume the mailbox, resolve Soft-SMMU IOVAs, execute, raise IRQ.
    pub fn service(&mut self) -> Option<Completion> {
        let cmd = self.mailbox.take()?;
        let job = self.pending_job.take()?;
        let scoped = self.pending_scoped.take();
        self.doorbell = false;
        let op = match cmd.decode_op() {
            Ok(op) => op,
            Err(_) => {
                let cpl = Completion {
                    job_seq: self.npu.seq,
                    status: -2,
                    cycles: 0,
                };
                return Some(self.complete(cmd.signal_payload, cpl));
            }
        };
        if op != AccelOp::Nop && self.smmu_walk(&cmd).is_err() {
            self.iommu.clear_submit_sid();
            let cpl = Completion {
                job_seq: self.npu.seq,
                status: -2,
                cycles: 0,
            };
            return Some(self.complete(cmd.signal_payload, cpl));
        }
        let Some(job) = self.job_from_cmd(&cmd, job) else {
            self.iommu.clear_submit_sid();
            let cpl = Completion {
                job_seq: self.npu.seq,
                status: -2,
                cycles: 0,
            };
            return Some(self.complete(cmd.signal_payload, cpl));
        };
        let result = match self.npu.execute(&job, &mut self.mem) {
            Ok(cpl) => {
                self.note_scoped(job.place.chiplet, scoped);
                Some(self.complete(cmd.signal_payload, cpl))
            }
            Err(_) => {
                let cpl = Completion {
                    job_seq: self.npu.seq,
                    status: -1,
                    cycles: 0,
                };
                Some(self.complete(cmd.signal_payload, cpl))
            }
        };
        self.iommu.clear_submit_sid();
        result
    }

    /// IOVA → guest PA. No identity shortcut: tensors come from Soft SMMU.
    fn job_from_cmd(&self, cmd: &IreeHalCmd, job: AccelJobDesc) -> Option<AccelJobDesc> {
        let op = cmd.decode_op().ok()?;
        if self.iommu.is_empty() || op == AccelOp::Nop {
            let mut pa = job;
            pa.op = op;
            return Some(pa);
        }
        let mut pa = job;
        pa.op = op;
        pa.a = self
            .iommu
            .resolve_stream(cmd.stream_id, PhysAddr(cmd.binding0_offset))?;
        pa.b = self
            .iommu
            .resolve_stream(cmd.stream_id, PhysAddr(cmd.binding1_offset))?;
        pa.c = self
            .iommu
            .resolve_stream(cmd.stream_id, PhysAddr(cmd.binding2_offset))?;
        if cmd.binding_count >= 4 && cmd.binding3_offset != 0 {
            pa.bias = self
                .iommu
                .resolve_stream(cmd.stream_id, PhysAddr(cmd.binding3_offset))?;
        }
        Some(pa)
    }

    fn smmu_walk(&self, cmd: &IreeHalCmd) -> Result<(), HalError> {
        let _ = self
            .iommu
            .resolve_submit(cmd.stream_id, PhysAddr(cmd.binding0_offset), None)
            .map_err(map_hal_error)?;
        let _ = self
            .iommu
            .resolve_submit(cmd.stream_id, PhysAddr(cmd.binding1_offset), None)
            .map_err(map_hal_error)?;
        let _ = self
            .iommu
            .resolve_submit(cmd.stream_id, PhysAddr(cmd.binding2_offset), None)
            .map_err(map_hal_error)?;
        if cmd.binding_count >= 4 && cmd.binding3_offset != 0 {
            let _ = self
                .iommu
                .resolve_submit(cmd.stream_id, PhysAddr(cmd.binding3_offset), None)
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

    fn note_scoped(&mut self, chiplet: ChipletId, scoped: Option<ScopedWork>) {
        let Some(s) = scoped else {
            return;
        };
        if let Ok(kind) = self.chipsync.note(chiplet, s) {
            self.last_scoped = Some(ScopedFence {
                fence: Fence::new(
                    FenceId(self.chipsync.timeline(s.scope).retired()),
                    aether_core::fence::TimelineId(s.scope as u32),
                    PartitionId(1),
                ),
                scope: s.scope,
                chiplet,
                kind,
            });
        }
    }
}

impl<M: DmaView> AccelDevice for IreeShapedCp<M> {
    fn probe(&mut self) -> Result<AccelInfo, HalError> {
        Ok(self.info)
    }

    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError> {
        let sid = self.arm_set_sid(job)?;
        let nouns = IreeHalNouns::from_job(job);
        let cmd = match IreeHalCmd::pack_on(job, &self.iommu, &nouns, sid) {
            Ok(cmd) => cmd,
            Err(e) => {
                self.iommu.clear_submit_sid();
                return Err(e);
            }
        };
        self.submit_hal(cmd, job)
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
        "iree-shaped-cp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::accel::SliceMem;
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::fence::{FenceId, Timeline};
    use aether_core::iommu::{StreamState, SOFT_SMMU_IOVA_BASE};
    use aether_core::partition::{
        BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
    };
    use aether_core::types::{ChipletId, TenantId};
    use aether_hal::{
        AccelDevice, ACCEL_BACKEND_IREE_SHAPED, ACCEL_BACKEND_PARTNER_STUB, ACCEL_BACKEND_SOFTNPU,
        ACCEL_BACKEND_SOFT_CP, ACCEL_BACKEND_VIRTIO_SOFTNPU,
    };

    fn mem_cap() -> Capability {
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 4, TenantId(1)).with_generation(1)
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

    fn pin_job(dev: &mut IreeShapedCp<SliceMem<'_>>, job: &AccelJobDesc) -> PhysAddr {
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
        let mut d = IreeShapedCp::new(mem);
        let info = d.probe().unwrap();
        assert_eq!(info.backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_eq!(info.backend, 4);
        assert_ne!(info.backend, ACCEL_BACKEND_SOFTNPU);
        assert_ne!(info.backend, ACCEL_BACKEND_VIRTIO_SOFTNPU);
        assert_ne!(info.backend, ACCEL_BACKEND_PARTNER_STUB);
        assert_ne!(info.backend, ACCEL_BACKEND_SOFT_CP);
        assert_eq!(info.device, 0x0004);
        assert_eq!(d.name(), "iree-shaped-cp");
    }

    #[test]
    fn iree_public_element_types_match_buffer_view_h() {
        // runtime/src/iree/hal/buffer_view.h IREE_HAL_ELEMENT_TYPE_VALUE.
        assert_eq!(IREE_HAL_ELEMENT_TYPE_INT_32, 0x1000_0020);
        assert_eq!(IREE_HAL_ELEMENT_TYPE_FLOAT_16, 0x2100_0010);
        assert_eq!(IREE_HAL_ELEMENT_TYPE_FLOAT_32, 0x2100_0020);
        assert_eq!(IREE_HAL_COMMAND_CATEGORY_TRANSFER, 1);
        assert_eq!(IREE_HAL_COMMAND_CATEGORY_DISPATCH, 2);
        assert_ne!(
            IREE_HAL_COMMAND_CATEGORY_DISPATCH as u32,
            AccelOp::MatMul as u32
        );
        assert_ne!(HAL_FN_FUSED, AccelOp::Wave as u32);
    }

    #[test]
    fn map_requires_memory_cap_and_relocates() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        let sid = StreamId::accel(ChipletId(0), TileId(2), IREE_SSID);
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
        let mut d = IreeShapedCp::new(mem);
        let sid = StreamId::accel(ChipletId(0), TileId(2), IREE_SSID);
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
    fn submit_packs_iree_hal_packet_poll_after_irq() {
        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        job.fence_id = 9;
        let sid = stream_for_job(&job);
        let nouns = IreeHalNouns::from_job(&job);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        assert!(d.probe().is_ok());
        let iova = pin_job(&mut d, &job);
        assert!(iova.0 >= SOFT_SMMU_IOVA_BASE);
        assert_ne!(iova.0, 0);

        d.submit(&job).unwrap();
        assert!(d.doorbell_pending());
        assert!(d.poll().is_none(), "completions come from IRQ, not submit");

        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.magic, IREE_HAL_PKT_MAGIC);
        assert_ne!(cmd.magic, 0xAE7E_0C01); // not CpCmd
        assert_eq!(cmd.command_categories, IREE_HAL_COMMAND_CATEGORY_DISPATCH);
        assert_ne!(cmd.command_categories as u32, AccelOp::MatMul as u32);
        assert_eq!(cmd.function, HAL_FN_MATMUL);
        assert_eq!(cmd.executable, IREE_REF_EXECUTABLE);
        assert_eq!(cmd.executable, nouns.executable.isa_blob_id);
        assert_eq!(cmd.element_type, IREE_HAL_ELEMENT_TYPE_INT_32);
        assert_eq!(cmd.workgroup_count_x, 2);
        assert_eq!(cmd.workgroup_count_y, 2);
        assert_eq!(cmd.workgroup_count_z, 2);
        assert_eq!(cmd.queue_affinity, queue_affinity_from_place(job.place));
        assert_eq!(cmd.stream_id, sid.raw());
        assert_eq!(StreamId::from_raw(cmd.stream_id).ssid(), IREE_SSID);
        assert_eq!(cmd.signal_payload, 9);
        assert_eq!(cmd.signal_payload, nouns.event.fence.0);
        assert_eq!(cmd.binding_count, 3);
        assert_eq!(cmd.binding0_offset, iova.0);
        assert_eq!(cmd.binding1_offset, iova.0 + 16);
        assert_eq!(cmd.binding2_offset, iova.0 + 32);
        assert_eq!(cmd.binding0_length, 16);
        assert_ne!(cmd.binding0_offset, job.a.0);
        let wire = cmd.to_le_bytes();
        assert_eq!(&wire[0..4], &IREE_HAL_PKT_MAGIC.to_le_bytes());
        assert_eq!(wire.len(), IREE_HAL_CMD_SIZE);

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
    fn submit_packs_f32_element_type_and_executes() {
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
        let mut d = IreeShapedCp::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.element_type, IREE_HAL_ELEMENT_TYPE_FLOAT_32);
        assert_eq!(
            cmd.to_le_bytes()[28..32],
            IREE_HAL_ELEMENT_TYPE_FLOAT_32.to_le_bytes()
        );
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
        let mut d = IreeShapedCp::new(mem);
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
        let mut d = IreeShapedCp::new(mem);
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
        let mut d = IreeShapedCp::new(mem);
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 256, other))
            .unwrap();
        assert_eq!(d.submit(&job).unwrap_err(), HalError::Fault);
    }

    #[test]
    fn softnpu_and_softcp_streams_are_wrong_sid() {
        let (mut backing, job) = matmul_backing();
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        // SoftNPU stream 0.
        d.map_with_cap(&mem_cap(), MapRequest::pin(PhysAddr(0), 256))
            .unwrap();
        assert_eq!(d.submit(&job).unwrap_err(), HalError::Fault);
        // Soft-CP ssid 1 on a fresh device.
        let mut backing2 = backing;
        let mem2 = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing2,
        };
        let mut d2 = IreeShapedCp::new(mem2);
        let cp_sid = StreamId::accel(ChipletId(0), TileId(0), 1);
        d2.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 256, cp_sid))
            .unwrap();
        assert_eq!(d2.submit(&job).unwrap_err(), HalError::Fault);
    }

    #[test]
    fn nop_doorbell_without_maps() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        let mut job = AccelJobDesc::matmul_i32(0, 0, 0, PhysAddr(0), PhysAddr(0), PhysAddr(0), 1);
        job.op = AccelOp::Nop;
        d.submit(&job).unwrap();
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.command_categories, 0);
        assert_eq!(cmd.function, 0, "Nop function is 0 and ignored");
        assert_eq!(cmd.binding_count, 0);
        assert_eq!(cmd.decode_op().unwrap(), AccelOp::Nop);
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
        let mut d = IreeShapedCp::new(mem);
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

    #[test]
    fn decode_keys_off_dispatch_bit_not_function() {
        assert_eq!(op_from_hal(0, HAL_FN_FUSED).unwrap(), AccelOp::Nop);
        assert_eq!(op_from_hal(0, 99).unwrap(), AccelOp::Nop);
        assert_eq!(
            op_from_hal(IREE_HAL_COMMAND_CATEGORY_DISPATCH, HAL_FN_MATMUL).unwrap(),
            AccelOp::MatMul
        );
        assert_eq!(
            op_from_hal(IREE_HAL_COMMAND_CATEGORY_DISPATCH, HAL_FN_FUSED).unwrap(),
            AccelOp::Wave
        );
        assert_eq!(
            op_from_hal(IREE_HAL_COMMAND_CATEGORY_TRANSFER, HAL_FN_MATMUL).unwrap_err(),
            HalError::Fault
        );
        assert_eq!(
            op_from_hal(
                IREE_HAL_COMMAND_CATEGORY_TRANSFER | IREE_HAL_COMMAND_CATEGORY_DISPATCH,
                HAL_FN_MATMUL
            )
            .unwrap_err(),
            HalError::Fault
        );
        assert_eq!(categories_from_op(AccelOp::Nop), 0);
        assert_eq!(
            categories_from_op(AccelOp::MatMul),
            IREE_HAL_COMMAND_CATEGORY_DISPATCH
        );
        assert_eq!(
            categories_from_op(AccelOp::Wave),
            IREE_HAL_COMMAND_CATEGORY_DISPATCH
        );
    }

    #[test]
    fn submit_hal_transfer_alone_and_foreign_executable_fault() {
        let (mut backing, job) = matmul_backing();
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        pin_job(&mut d, &job);
        let packed = IreeHalCmd::pack(&job, &d.iommu).unwrap();
        assert_eq!(packed.magic, IREE_HAL_PKT_MAGIC);
        assert_eq!(packed.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(StreamId::from_raw(packed.stream_id).ssid(), IREE_SSID);
        assert_eq!(packed.workgroup_count_x, job.m);
        assert_eq!(packed.workgroup_count_y, job.n);
        assert_eq!(packed.workgroup_count_z, job.k);
        assert_eq!(packed.binding0_length, job.bytes_a() as u32);
        assert_ne!(packed.binding0_length, job.m * job.k, "bytes, not elements");

        let mut transfer = packed;
        transfer.command_categories = IREE_HAL_COMMAND_CATEGORY_TRANSFER;
        assert_eq!(d.submit_hal(transfer, &job).unwrap_err(), HalError::Fault);

        let mut foreign = packed;
        foreign.executable = 1;
        assert_eq!(
            d.submit_hal(foreign, &job).unwrap_err(),
            HalError::Unsupported
        );

        let mut nouns = IreeHalNouns::from_job(&job);
        nouns.executable.isa_blob_id = 0xDEAD;
        assert_eq!(
            IreeHalCmd::pack_with_nouns(&job, &d.iommu, &nouns).unwrap_err(),
            HalError::Unsupported
        );
    }

    #[test]
    fn binding_lengths_are_dtype_aware_byte_spans() {
        let mut backing = [0u8; 256];
        let mut job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job.dtype = DType::F16;
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        pin_job(&mut d, &job);
        let cmd = IreeHalCmd::pack(&job, &d.iommu).unwrap();
        // 2×2 F16 = 8 bytes, not 4 elements.
        assert_eq!(cmd.binding0_length, 8);
        assert_eq!(cmd.binding1_length, 8);
        assert_eq!(cmd.binding2_length, 8);
        assert_eq!(cmd.element_type, IREE_HAL_ELEMENT_TYPE_FLOAT_16);
        assert_ne!(cmd.binding0_length, 4);
    }

    #[test]
    fn submit_arms_set_sid_before_dma() {
        let (mut backing, job) = matmul_backing();
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.stream_id, stream_for_job(&job).raw());
        assert_eq!(d.iommu.submit_sid(), Some(stream_for_job(&job)));
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.iommu.submit_sid(), None);
    }

    #[test]
    fn two_tenants_two_sids_iree_set_sid() {
        use aether_core::space::{MemorySpace, Place};

        let mut backing = [0u8; 512];
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            backing[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [1i32, 0, 0, 1].iter().enumerate() {
            backing[256 + i * 4..256 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [9i32, 10, 11, 12].iter().enumerate() {
            backing[272 + i * 4..272 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }

        let mut job_a =
            AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job_a.place = job_a.place.with_tile(2);
        let mut job_b =
            AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(256), PhysAddr(272), PhysAddr(288), 2);
        job_b.place = Place::new(ChipletId(1), MemorySpace::Host).with_tile(3);
        let sid_a = stream_for_job(&job_a);
        let sid_b = stream_for_job(&job_b);
        let cap_a = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 21, TenantId(1))
            .with_generation(1);
        let cap_b = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 22, TenantId(2))
            .with_generation(1);

        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        d.map_with_cap(&cap_a, MapRequest::pin_accel(PhysAddr(0), 256, sid_a))
            .unwrap();
        d.map_with_cap(&cap_b, MapRequest::pin_accel(PhysAddr(256), 256, sid_b))
            .unwrap();

        assert_eq!(d.set_sid(&cap_a, sid_b).unwrap_err(), HalError::Fault);
        d.set_sid(&cap_a, sid_a).unwrap();
        d.submit(&job_a).unwrap();
        assert_eq!(d.last_cmd().unwrap().stream_id, sid_a.raw());
        assert_eq!(
            StreamId::from_raw(d.last_cmd().unwrap().stream_id).ssid(),
            IREE_SSID
        );
        assert_eq!(d.service().unwrap().status, 0);
        d.poll();

        d.set_sid(&cap_b, sid_b).unwrap();
        d.submit(&job_b).unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        d.poll();

        d.set_sid(&cap_b, sid_b).unwrap();
        assert_eq!(d.submit(&job_a).unwrap_err(), HalError::Fault);
        drop(d);
        assert_eq!(i32::from_le_bytes(backing[32..36].try_into().unwrap()), 19);
        assert_eq!(i32::from_le_bytes(backing[288..292].try_into().unwrap()), 9);
    }

    #[test]
    fn inject_wrong_sid_aborts_iree_dma() {
        let (mut backing, job) = matmul_backing();
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        d.inject_wrong_sid(StreamId::accel(ChipletId(0), TileId(0), 7));
        assert_eq!(d.service().unwrap().status, -2);
        assert_eq!(d.iommu.submit_sid(), None);
    }

    #[test]
    fn chipsync_two_chiplet_iree_mailbox_producer_consumer() {
        use aether_core::chipsync::{BufferLabel, SignalKind, SyncScope};
        use aether_core::space::{MemorySpace, Place};

        let mut backing = [0u8; 512];
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            backing[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [1i32, 0, 0, 1].iter().enumerate() {
            backing[256 + i * 4..256 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [9i32, 10, 11, 12].iter().enumerate() {
            backing[272 + i * 4..272 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }

        let mut job_a =
            AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job_a.place = job_a.place.with_tile(2);
        let mut job_b =
            AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(256), PhysAddr(272), PhysAddr(288), 2);
        job_b.place = Place::new(ChipletId(1), MemorySpace::Host).with_tile(3);
        let sid_a = stream_for_job(&job_a);
        let sid_b = stream_for_job(&job_b);
        let cap_a = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 31, TenantId(1))
            .with_generation(1);
        let cap_b = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 32, TenantId(2))
            .with_generation(1);

        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        d.map_with_cap(&cap_a, MapRequest::pin_accel(PhysAddr(0), 256, sid_a))
            .unwrap();
        d.map_with_cap(&cap_b, MapRequest::pin_accel(PhysAddr(256), 256, sid_b))
            .unwrap();

        let buf = BufferLabel(2);
        d.chipsync.enable_cct(true);
        d.chipsync.open(SyncScope::Package);
        d.chipsync.expect(ChipletId(0), 8).unwrap();
        for _ in 0..7 {
            assert_eq!(
                d.chipsync.arrive(ChipletId(0), Some(buf)).unwrap().kind,
                SignalKind::ChipletLocal
            );
        }

        d.submit_scoped(&job_a, SyncScope::Package, Some(buf), None)
            .unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.last_scoped().unwrap().kind, SignalKind::PendingPackage);
        d.poll();

        d.submit_scoped(&job_b, SyncScope::Package, None, Some(buf))
            .unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.last_scoped().unwrap().kind, SignalKind::PackageFence);
        d.poll();

        assert_eq!(d.chipsync.package_fences(), 1);
        assert_eq!(d.chipsync.naive_package_fences(), 8);
        assert!(d.chipsync.package_lt_naive());
        assert_eq!(d.chipsync.elided(), 0);
        assert!(d.chipsync.cct().incorrect_elide(buf));
        assert!(!d.chipsync.softcct().should_elide(buf, ChipletId(1)));
        assert_eq!(d.last_cmd().unwrap().stream_id, sid_b.raw());
        drop(d);
        assert_eq!(i32::from_le_bytes(backing[32..36].try_into().unwrap()), 19);
        assert_eq!(i32::from_le_bytes(backing[288..292].try_into().unwrap()), 9);
    }
}
