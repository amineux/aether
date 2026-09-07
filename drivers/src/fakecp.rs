//! Software command processor (`SoftCommandProcessor`).
//!
//! Honest model of what a silicon CP would ingest: a fixed 64-byte
//! packet packed from [`AccelJobDesc`] (opcode / dtype / place / IOVAs /
//! shape / packed [`StreamId`] / fence). Not SoftNPU's virtqueue BAR, not
//! [`crate::PartnerNpuStub`]'s no-op complete-on-submit, and not a
//! vendor partnership.
//!
//! The schedulable object is a software **XQueue** (two of them), not
//! the device mailbox. Inspiration: XSched XQueue
//! (https://github.com/XpuOS/xsched, OSDI'25) — an open, multi-level
//! hardware execution-queue model. This is **not** an LD_PRELOAD CUDA
//! shim and **not** a silicon queuing unit. Preemption is
//! **queue-boundary** only: `suspend` refuses the next packed command
//! on that queue; a command already inside `service()` runs to
//! completion. Soft-SMMU SID sticks to the queue (or inherits at
//! submit). Host1x-shaped SET_SID arms the Soft SMMU latch from that
//! queue SID at the job head. Not a Tegra driver.
//!
//! Uses the post-#7 Soft SMMU APIs:
//! ```text
//! StreamId::accel(chiplet, tile, CP_SSID)
//! bind_stream / map (Memory+MAP)     // DMA aborts until Bound
//! create_xqueue / stamp_queue_sid    // SID sticks to the queue
//! SET_SID (job head)                 // inherit or privileged latch → queue
//! submit_xqueue → pack CpCmd on the queue SID, enqueue (does not execute)
//! suspend / resume                   // queue-boundary only
//! service → pick Running queue, resolve_submit + SoftNPU
//! poll → completion; caller retires the fence
//! ```
//!
//! SET_SID is **not** a Tegra Host1x class opcode and not a second IR.
//! SoftCmdFirewall copies the packed image into a kernel-owned arena
//! before opcode / reloc / SID / addr-cap validate. Host1x lesson:
//! validate after copy or userspace races the rewrite. Integrity of
//! the command stream only — not confidential GPU.
//!
//! SoftChipletSync (wave / CU / chiplet / package timelines, Fleet-shaped
//! hierarchical counters, optional CPElide CCT) is a **software** fence
//! domain on this CP. Distinct from ChipletFleet placement. Not UCIe,
//! not a Vulkan timeline product. Latency wins need a multi-chiplet sim.

use aether_core::accel::{AccelJobDesc, AccelOp, Completion, DmaView, SoftNpu};
use aether_core::caps::Capability;
use aether_core::chipsync::{BufferLabel, ScopedFence, ScopedWork, SoftChipletSync, SyncScope};
use aether_core::fence::{Fence, FenceId, Timeline};
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::partition::{PartitionError, PartitionId};
use aether_core::types::{ChipletId, PhysAddr, TileId};
use aether_core::window::{MappedWindow, TypedWindow};
use aether_hal::{AccelDevice, AccelInfo, HalError, PreemptionLevel, ACCEL_BACKEND_SOFT_CP};

use crate::firewall::{job_template_from_cmd, ClientCmdStream, FirewallSim, SoftCmdFirewall};

/// Packet magic a CP mailbox would DMA (`AE7E` + command-processor `0C01`).
pub const CP_PKT_MAGIC: u32 = 0xAE7E_0C01;
pub const CP_CMD_SIZE: usize = 64;
pub const CP_FLAG_HAS_BIAS: u16 = 1 << 0;
/// Job-head SET_SID was programmed for this submit (Host1x-shaped).
pub const CP_FLAG_SET_SID: u16 = 1 << 1;
/// Soft-CP substream. Distinct from SoftNPU's `DEFAULT_STREAM` (ssid 0).
pub const CP_SSID: u8 = 1;
/// Two software XQueues. Not a silicon queueing-unit count.
pub const SOFT_CP_XQUEUES: usize = 2;
/// Pending `CpCmd`s per XQueue. Depth is software; not a HW ring size.
pub const XQUEUE_DEPTH: usize = 4;

/// Pack the CP stream from a job's fabric place.
pub fn stream_for_job(job: &AccelJobDesc) -> StreamId {
    StreamId::accel(
        job.place.chiplet,
        TileId(job.place.tile.unwrap_or(0)),
        CP_SSID,
    )
}

/// Software XQueue state. `Suspended` parks the queue; in-flight
/// `service()` of a command already dequeued still completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XQueueState {
    Running,
    Suspended,
}

/// Optional SoftChipletSync annotation on a queued command. Default
/// submit leaves this `None` (SID / XQueue path unchanged).
#[derive(Clone, Copy, Debug)]
struct XQueueSlot {
    cmd: CpCmd,
    job: AccelJobDesc,
    scoped: Option<ScopedWork>,
}

/// Software execution queue on Soft-CP. The schedulable object.
///
/// SID is sticky once stamped (`create_xqueue` / `stamp_queue_sid`) or
/// inherited at submit ([`stream_for_job`] or a privileged SET_SID).
/// Host1x-shaped doorbell: [`SoftCommandProcessor::set_sid`] /
/// [`SoftCommandProcessor::stamp_queue_sid`].
#[derive(Clone, Copy, Debug)]
pub struct XQueue {
    pub id: u8,
    pub sid: Option<StreamId>,
    pub state: XQueueState,
    pub priority: u8,
    slots: [Option<XQueueSlot>; XQUEUE_DEPTH],
    head: u8,
    len: u8,
}

impl XQueue {
    const fn empty(id: u8) -> Self {
        Self {
            id,
            sid: None,
            state: XQueueState::Running,
            priority: 0,
            slots: [None; XQUEUE_DEPTH],
            head: 0,
            len: 0,
        }
    }

    pub const fn pending(&self) -> u8 {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len as usize >= XQUEUE_DEPTH
    }

    fn push(&mut self, slot: XQueueSlot) -> Result<(), HalError> {
        if self.is_full() {
            return Err(HalError::Busy);
        }
        let i = (self.head as usize + self.len as usize) % XQUEUE_DEPTH;
        self.slots[i] = Some(slot);
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<XQueueSlot> {
        if self.is_empty() {
            return None;
        }
        let slot = self.slots[self.head as usize].take()?;
        self.head = (self.head + 1) % XQUEUE_DEPTH as u8;
        self.len -= 1;
        Some(slot)
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
    /// Uses the programmed submit SID when armed, else [`stream_for_job`].
    /// Queue submit uses [`Self::pack_on_stream`] with the queue's sticky SID.
    pub fn pack(job: &AccelJobDesc, iommu: &IommuMap) -> Result<Self, HalError> {
        let sid = iommu.submit_sid().unwrap_or_else(|| stream_for_job(job));
        Self::pack_on_stream(job, iommu, sid)
    }

    /// Pack on an explicit SET_SID / queue-sticky StreamID.
    pub fn pack_on(job: &AccelJobDesc, iommu: &IommuMap, sid: StreamId) -> Result<Self, HalError> {
        Self::pack_on_stream(job, iommu, sid)
    }

    /// Pack on an explicit Soft-SMMU SID (the queue's sticky stream).
    ///
    /// `Nop` is a doorbell / latency probe and does not require pins.
    /// Every other op refuses unless `sid` is Bound and A/B/C (and bias,
    /// if set) translate on that SID.
    pub fn pack_on_stream(
        job: &AccelJobDesc,
        iommu: &IommuMap,
        sid: StreamId,
    ) -> Result<Self, HalError> {
        if job.m > u16::MAX as u32 || job.n > u16::MAX as u32 || job.k > u16::MAX as u32 {
            return Err(HalError::BadArg);
        }
        if job.op == AccelOp::Nop {
            return Ok(Self::empty_from(job, sid));
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
        if job.bias.0 != 0
            && !iommu.covers_stream(sid.raw(), job.bias, es.saturating_mul(job.n as u64).max(es))
        {
            return Err(HalError::Fault);
        }
        let mut flags = 0u16;
        if job.bias.0 != 0 {
            flags |= CP_FLAG_HAS_BIAS;
        }
        if iommu.submit_sid() == Some(sid) {
            flags |= CP_FLAG_SET_SID;
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

    fn empty_from(job: &AccelJobDesc, sid: StreamId) -> Self {
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

    /// Inverse of [`Self::to_le_bytes`]. Used on the kernel copy only.
    pub fn from_le_bytes(b: [u8; CP_CMD_SIZE]) -> Result<Self, HalError> {
        Ok(Self {
            magic: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            opcode: b[4],
            dtype: b[5],
            space: b[6],
            phase: b[7],
            m: u16::from_le_bytes(b[8..10].try_into().unwrap()),
            n: u16::from_le_bytes(b[10..12].try_into().unwrap()),
            k: u16::from_le_bytes(b[12..14].try_into().unwrap()),
            flags: u16::from_le_bytes(b[14..16].try_into().unwrap()),
            stream_id: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            chiplet: u16::from_le_bytes(b[20..22].try_into().unwrap()),
            tile: u16::from_le_bytes(b[22..24].try_into().unwrap()),
            iova_a: u64::from_le_bytes(b[24..32].try_into().unwrap()),
            iova_b: u64::from_le_bytes(b[32..40].try_into().unwrap()),
            iova_c: u64::from_le_bytes(b[40..48].try_into().unwrap()),
            iova_bias: u64::from_le_bytes(b[48..56].try_into().unwrap()),
            fence_id: u64::from_le_bytes(b[56..64].try_into().unwrap()),
        })
    }
}

/// Software command processor. Completions arrive on the IRQ/poll path,
/// never inside [`AccelDevice::submit`]. Two software XQueues are the
/// schedulable objects; the integer engine still runs one command at a
/// time (queue-boundary preemption).
pub struct SoftCommandProcessor<M: DmaView> {
    pub info: AccelInfo,
    pub iommu: IommuMap,
    pub mem: M,
    npu: SoftNpu,
    queues: [XQueue; SOFT_CP_XQUEUES],
    submit_seq: u32,
    irq: bool,
    last_cmd: Option<CpCmd>,
    last_cpl: Option<Completion>,
    last_fence: Option<u64>,
    last_queue: Option<u8>,
    fence_done: bool,
    /// Scoped timelines + hierarchical counters + optional CCT.
    pub chipsync: SoftChipletSync,
    last_scoped: Option<ScopedFence>,
    /// Copy-then-validate arena (Host1x-shaped). Not GPU-CC.
    pub firewall: SoftCmdFirewall,
}

impl<M: DmaView> SoftCommandProcessor<M> {
    pub fn new(mem: M) -> Self {
        Self {
            info: AccelInfo {
                vendor: 0xAE7E,
                device: 0x0003,
                n_queues: SOFT_CP_XQUEUES as u16,
                max_wave: 64,
                backend: ACCEL_BACKEND_SOFT_CP,
            },
            iommu: IommuMap::new(),
            mem,
            npu: SoftNpu::new(),
            queues: [XQueue::empty(0), XQueue::empty(1)],
            submit_seq: 0,
            irq: false,
            last_cmd: None,
            last_cpl: None,
            last_fence: None,
            last_queue: None,
            fence_done: false,
            chipsync: SoftChipletSync::new(PartitionId(1)),
            last_scoped: None,
            firewall: SoftCmdFirewall::new(),
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

    /// Fault injection: overwrite the last queued packet StreamID after
    /// SET_SID. Soft SMMU must abort DMA. Not a public submit path.
    pub fn inject_wrong_sid(&mut self, sid: StreamId) {
        let Some(prev) = self.last_cmd else {
            return;
        };
        if let Some(cmd) = self.last_cmd.as_mut() {
            cmd.stream_id = sid.raw();
        }
        for q in &mut self.queues {
            for slot in q.slots.iter_mut().flatten() {
                if slot.cmd == prev {
                    slot.cmd.stream_id = sid.raw();
                }
            }
        }
    }

    pub fn map_with_cap(
        &mut self,
        cap: &Capability,
        req: MapRequest,
    ) -> Result<PhysAddr, HalError> {
        let region = self.iommu.map(cap, req).map_err(map_hal_error)?;
        Ok(region.iova)
    }

    /// Pin a typed window (SID + Memory+MAP). Not a CXL.mem decoder.
    pub fn map_window_with_cap(
        &mut self,
        cap: &Capability,
        win: TypedWindow,
    ) -> Result<MappedWindow, HalError> {
        self.iommu.map_window(cap, win).map_err(map_hal_error)
    }

    pub fn last_cmd(&self) -> Option<CpCmd> {
        self.last_cmd
    }

    /// Queue that last completed inside [`Self::service`].
    pub fn last_queue(&self) -> Option<u8> {
        self.last_queue
    }

    pub fn xqueue(&self, queue: u16) -> Option<&XQueue> {
        self.queues.get(queue as usize)
    }

    /// A Running XQueue has a packed command ready. Suspended work does
    /// not ring the doorbell.
    pub fn doorbell_pending(&self) -> bool {
        self.queues
            .iter()
            .any(|q| q.state == XQueueState::Running && !q.is_empty())
    }

    /// Create / restamp a software XQueue. SID sticks here.
    ///
    /// SET_SID at submit also inherits or restamps an empty queue via
    /// [`Self::sid_for_submit`]. [`Self::stamp_queue_sid`] is the
    /// doorbell write without a job.
    pub fn create_xqueue(
        &mut self,
        queue: u16,
        sid: StreamId,
        priority: u8,
    ) -> Result<(), HalError> {
        let q = self.queue_mut(queue)?;
        if !q.is_empty() {
            if let Some(stuck) = q.sid {
                if stuck != sid {
                    return Err(HalError::Busy);
                }
            }
        }
        q.sid = Some(sid);
        q.priority = priority;
        q.state = XQueueState::Running;
        Ok(())
    }

    /// Host1x-shaped doorbell: program the queue SID without a submit.
    pub fn stamp_queue_sid(&mut self, queue: u16, sid: StreamId) -> Result<(), HalError> {
        let q = self.queue_mut(queue)?;
        if !q.is_empty() {
            if let Some(stuck) = q.sid {
                if stuck != sid {
                    return Err(HalError::Busy);
                }
            }
        }
        q.sid = Some(sid);
        Ok(())
    }

    pub fn submit_xqueue(&mut self, queue: u16, job: &AccelJobDesc) -> Result<u32, HalError> {
        let prev_sid = self.xqueue(queue).and_then(|q| q.sid);
        let sid = self.sid_for_submit(queue, job)?;
        let restore_sid = |this: &mut Self| {
            this.iommu.clear_submit_sid();
            if let Ok(q) = this.queue_mut(queue) {
                q.sid = prev_sid;
            }
        };
        if job.op != AccelOp::Nop {
            if let Err(e) = self.iommu.set_sid_bound(sid) {
                restore_sid(self);
                return Err(map_hal_error(e));
            }
        }
        let cmd = match CpCmd::pack_on_stream(job, &self.iommu, sid) {
            Ok(cmd) => cmd,
            Err(e) => {
                restore_sid(self);
                return Err(e);
            }
        };
        // Host1x lesson: copy the packed image, validate the copy, enqueue
        // the copy. A later rewrite of a userspace alias cannot sneak.
        let cmd = match self.firewall.admit_packed(cmd, &self.iommu, Some(sid)) {
            Ok(cmd) => cmd,
            Err(e) => {
                restore_sid(self);
                return Err(e);
            }
        };
        let push_err = {
            let q = self.queue_mut(queue)?;
            q.push(XQueueSlot {
                cmd,
                job: *job,
                scoped: None,
            })
            .err()
        };
        if let Some(e) = push_err {
            restore_sid(self);
            return Err(e);
        }
        // Latch travels with the packet (`CP_FLAG_SET_SID` + stream_id).
        // Clear so a later submit on another queue cannot inherit it.
        self.iommu.clear_submit_sid();
        self.last_cmd = Some(cmd);
        self.irq = false;
        self.last_cpl = None;
        self.fence_done = false;
        self.last_fence = if job.fence_id != 0 {
            Some(job.fence_id)
        } else {
            None
        };
        self.submit_seq = self.submit_seq.wrapping_add(1);
        Ok(self.submit_seq)
    }

    /// Userspace command-image submit: copy-then-validate, then enqueue.
    ///
    /// `job` is the DMA template (`service` resolves IOVAs back to guest
    /// PAs). The packet comes from `stream`, not from a second pack.
    pub fn submit_cmdbuf<S: ClientCmdStream>(
        &mut self,
        queue: u16,
        stream: &mut S,
        job: &AccelJobDesc,
    ) -> Result<u32, HalError> {
        let prev_sid = self.xqueue(queue).and_then(|q| q.sid);
        let sid = self.sid_for_submit(queue, job)?;
        let restore_sid = |this: &mut Self| {
            this.iommu.clear_submit_sid();
            if let Ok(q) = this.queue_mut(queue) {
                q.sid = prev_sid;
            }
        };
        if job.op != AccelOp::Nop {
            if let Err(e) = self.iommu.set_sid_bound(sid) {
                restore_sid(self);
                return Err(map_hal_error(e));
            }
        }
        let cmd = match self.firewall.ingest(stream, &self.iommu, Some(sid)) {
            Ok(cmd) => cmd,
            Err(e) => {
                restore_sid(self);
                return Err(e);
            }
        };
        let job = match job_template_from_cmd(&cmd) {
            Ok(mut t) => {
                t.tenant = job.tenant;
                t.completion_ep = job.completion_ep;
                t.partition = job.partition;
                t.a = job.a;
                t.b = job.b;
                t.c = job.c;
                t.bias = job.bias;
                t
            }
            Err(e) => {
                restore_sid(self);
                return Err(e);
            }
        };
        let push_err = {
            let q = self.queue_mut(queue)?;
            q.push(XQueueSlot {
                cmd,
                job,
                scoped: None,
            })
            .err()
        };
        if let Some(e) = push_err {
            restore_sid(self);
            return Err(e);
        }
        self.iommu.clear_submit_sid();
        self.last_cmd = Some(cmd);
        self.irq = false;
        self.last_cpl = None;
        self.fence_done = false;
        self.last_fence = if cmd.fence_id != 0 {
            Some(cmd.fence_id)
        } else {
            None
        };
        self.submit_seq = self.submit_seq.wrapping_add(1);
        Ok(self.submit_seq)
    }

    /// Software copy + validate steps from the last firewall admit.
    pub fn last_firewall_sim(&self) -> FirewallSim {
        self.firewall.last_sim
    }

    /// Submit onto an XQueue and tag SoftChipletSync (scope + CCT labels).
    ///
    /// [`Self::submit_xqueue`] is unchanged (no scoped annotation). Not a
    /// second IR. `write` records last-writer chiplet; `read` is the
    /// consumer wait.
    pub fn submit_scoped(
        &mut self,
        queue: u16,
        job: &AccelJobDesc,
        scope: SyncScope,
        write: Option<BufferLabel>,
        read: Option<BufferLabel>,
    ) -> Result<u32, HalError> {
        let seq = self.submit_xqueue(queue, job)?;
        self.chipsync.set_scope(scope);
        let q = self.queue_mut(queue)?;
        if q.len == 0 {
            return Ok(seq);
        }
        let tail = (q.head as usize + q.len as usize - 1) % XQUEUE_DEPTH;
        if let Some(slot) = q.slots[tail].as_mut() {
            slot.scoped = Some(ScopedWork { scope, write, read });
        }
        Ok(seq)
    }

    pub fn last_scoped(&self) -> Option<ScopedFence> {
        self.last_scoped
    }

    pub fn suspend_xqueue(&mut self, queue: u16) -> Result<PreemptionLevel, HalError> {
        let q = self.queue_mut(queue)?;
        q.state = XQueueState::Suspended;
        // Honest grain: we do not stop SoftNpu::execute mid-op.
        Ok(PreemptionLevel::QueueBoundary)
    }

    pub fn resume_xqueue(&mut self, queue: u16) -> Result<(), HalError> {
        let q = self.queue_mut(queue)?;
        q.state = XQueueState::Running;
        Ok(())
    }

    fn queue_mut(&mut self, queue: u16) -> Result<&mut XQueue, HalError> {
        self.queues.get_mut(queue as usize).ok_or(HalError::BadArg)
    }

    /// Sticky queue SID, privileged SET_SID latch, or inherit
    /// [`stream_for_job`] and stick it. An empty queue may restamp from
    /// an armed SET_SID (Host1x-shaped doorbell between jobs).
    fn sid_for_submit(&mut self, queue: u16, job: &AccelJobDesc) -> Result<StreamId, HalError> {
        let job_sid = stream_for_job(job);
        let armed = self.iommu.submit_sid();
        let q = self.queue_mut(queue)?;
        match (q.sid, armed) {
            (Some(stuck), Some(armed)) if stuck != armed => {
                if !q.is_empty() {
                    return Err(HalError::Fault);
                }
                q.sid = Some(armed);
                Ok(armed)
            }
            (Some(stuck), Some(_)) => Ok(stuck),
            (Some(stuck), None) if stuck != job_sid => Err(HalError::Fault),
            (Some(stuck), None) => Ok(stuck),
            (None, Some(armed)) => {
                q.sid = Some(armed);
                Ok(armed)
            }
            (None, None) => {
                q.sid = Some(job_sid);
                Ok(job_sid)
            }
        }
    }

    fn pick_running(&self) -> Option<usize> {
        self.queues
            .iter()
            .enumerate()
            .filter(|(_, q)| q.state == XQueueState::Running && !q.is_empty())
            .max_by(|a, b| a.1.priority.cmp(&b.1.priority).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
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

    /// Device-side: dequeue one command from the highest-priority
    /// Running XQueue, resolve Soft-SMMU IOVAs, execute, raise IRQ.
    /// Suspended queues are skipped (queue-boundary preemption).
    pub fn service(&mut self) -> Option<Completion> {
        let qi = self.pick_running()?;
        let slot = self.queues[qi].pop()?;
        let cmd = slot.cmd;
        let job = slot.job;
        self.last_queue = Some(qi as u8);
        if job.op != AccelOp::Nop {
            let sid = StreamId::from_raw(cmd.stream_id);
            let stamped = cmd.flags & CP_FLAG_SET_SID != 0;
            if !stamped || self.iommu.set_sid_bound(sid).is_err() || self.smmu_walk(&cmd).is_err() {
                self.iommu.clear_submit_sid();
                let cpl = Completion {
                    job_seq: self.npu.seq,
                    status: -2,
                    cycles: 0,
                };
                return Some(self.complete(cmd.fence_id, cpl));
            }
        }
        let Some(job) = self.job_from_cmd(&cmd, job) else {
            self.iommu.clear_submit_sid();
            let cpl = Completion {
                job_seq: self.npu.seq,
                status: -2,
                cycles: 0,
            };
            return Some(self.complete(cmd.fence_id, cpl));
        };
        let result = match self.npu.execute(&job, &mut self.mem) {
            Ok(cpl) => {
                self.note_scoped(ChipletId(cmd.chiplet as u8), slot.scoped);
                Some(self.complete(cmd.fence_id, cpl))
            }
            Err(_) => {
                let cpl = Completion {
                    job_seq: self.npu.seq,
                    status: -1,
                    cycles: 0,
                };
                Some(self.complete(cmd.fence_id, cpl))
            }
        };
        self.iommu.clear_submit_sid();
        result
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

    /// IOVA → guest PA. No identity shortcut: tensors come from Soft SMMU.
    fn job_from_cmd(&self, cmd: &CpCmd, job: AccelJobDesc) -> Option<AccelJobDesc> {
        if self.iommu.is_empty() || job.op == AccelOp::Nop {
            return Some(job);
        }
        let mut pa = job;
        pa.a = self
            .iommu
            .resolve_stream(cmd.stream_id, PhysAddr(cmd.iova_a))?;
        pa.b = self
            .iommu
            .resolve_stream(cmd.stream_id, PhysAddr(cmd.iova_b))?;
        pa.c = self
            .iommu
            .resolve_stream(cmd.stream_id, PhysAddr(cmd.iova_c))?;
        if cmd.flags & CP_FLAG_HAS_BIAS != 0 {
            pa.bias = self
                .iommu
                .resolve_stream(cmd.stream_id, PhysAddr(cmd.iova_bias))?;
        }
        Some(pa)
    }

    fn smmu_walk(&self, cmd: &CpCmd) -> Result<(), HalError> {
        let _ = self
            .iommu
            .resolve_submit(cmd.stream_id, PhysAddr(cmd.iova_a), None)
            .map_err(map_hal_error)?;
        let _ = self
            .iommu
            .resolve_submit(cmd.stream_id, PhysAddr(cmd.iova_b), None)
            .map_err(map_hal_error)?;
        let _ = self
            .iommu
            .resolve_submit(cmd.stream_id, PhysAddr(cmd.iova_c), None)
            .map_err(map_hal_error)?;
        if cmd.flags & CP_FLAG_HAS_BIAS != 0 {
            let _ = self
                .iommu
                .resolve_submit(cmd.stream_id, PhysAddr(cmd.iova_bias), None)
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
        self.submit_xqueue(0, job)
    }

    fn create_queue(&mut self, queue: u16, stream_id: u32, priority: u8) -> Result<(), HalError> {
        self.create_xqueue(queue, StreamId::from_raw(stream_id), priority)
    }

    fn submit_queue(&mut self, queue: u16, job: &AccelJobDesc) -> Result<u32, HalError> {
        self.submit_xqueue(queue, job)
    }

    fn suspend_queue(&mut self, queue: u16) -> Result<PreemptionLevel, HalError> {
        self.suspend_xqueue(queue)
    }

    fn resume_queue(&mut self, queue: u16) -> Result<(), HalError> {
        self.resume_xqueue(queue)
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
    use aether_core::chipsync::{BufferLabel, SignalKind, SyncScope};
    use aether_core::fence::{FenceId, Timeline};
    use aether_core::iommu::{InvCmd, MapError, SteConfig, StreamState, SOFT_SMMU_IOVA_BASE};
    use aether_core::partition::{
        BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
    };
    use aether_core::smmu_bringup::{kit_cp_sid, kit_iree_sid, kit_wrong_sid};
    use aether_core::space::Place;
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
        assert_eq!(info.n_queues, SOFT_CP_XQUEUES as u16);
        assert_eq!(d.name(), "soft-cp");
        assert_eq!(d.xqueue(0).unwrap().state, XQueueState::Running);
        assert_eq!(d.xqueue(1).unwrap().state, XQueueState::Running);
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
    fn map_window_with_cap_sid_and_rights() {
        use aether_core::window::{TypedWindow, WindowKind};

        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let sid = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        let win = TypedWindow::new(
            PhysAddr(0xB000),
            0x1000,
            WindowKind::CxlMemStub,
            sid,
            TenantId(1),
        );
        assert_eq!(d.map_window(win).unwrap_err(), HalError::NoMemoryCap);
        let mapped = d.map_window_with_cap(&mem_cap(), win).unwrap();
        assert_ne!(mapped.region.iova.0, 0xB000);
        assert_eq!(mapped.window.kind, WindowKind::CxlMemStub);
        let other = StreamId::accel(ChipletId(0), TileId(2), 3);
        assert_eq!(
            d.iommu
                .unmap_window(&mem_cap(), other, mapped.region.iova)
                .unwrap_err(),
            MapError::WrongStream
        );
        d.iommu
            .unmap_window(&mem_cap(), sid, mapped.region.iova)
            .unwrap();
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

    #[test]
    fn submit_stamps_set_sid_flag() {
        let (mut backing, job) = matmul_backing();
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.flags & CP_FLAG_SET_SID, CP_FLAG_SET_SID);
        assert_eq!(cmd.stream_id, stream_for_job(&job).raw());
        assert_eq!(d.xqueue(0).unwrap().sid, Some(stream_for_job(&job)));
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.iommu.submit_sid(), None);
    }

    #[test]
    fn two_tenants_two_sids_set_sid_at_submit() {
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
        let cap_a = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 11, TenantId(1))
            .with_generation(1);
        let cap_b = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 12, TenantId(2))
            .with_generation(1);

        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        d.map_with_cap(&cap_a, MapRequest::pin_accel(PhysAddr(0), 256, sid_a))
            .unwrap();
        d.map_with_cap(&cap_b, MapRequest::pin_accel(PhysAddr(256), 256, sid_b))
            .unwrap();

        assert_eq!(d.set_sid(&cap_a, sid_b).unwrap_err(), HalError::Fault);
        assert_eq!(d.set_sid(&cap_a, sid_a).unwrap(), sid_a);
        d.submit(&job_a).unwrap();
        assert_eq!(d.last_cmd().unwrap().stream_id, sid_a.raw());
        assert_eq!(
            d.last_cmd().unwrap().flags & CP_FLAG_SET_SID,
            CP_FLAG_SET_SID
        );
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid_a));
        assert_eq!(d.service().unwrap().status, 0);
        d.poll();

        d.set_sid(&cap_b, sid_b).unwrap();
        d.submit(&job_b).unwrap();
        assert_eq!(d.last_cmd().unwrap().stream_id, sid_b.raw());
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid_b));
        assert_eq!(d.service().unwrap().status, 0);
        d.poll();

        // Channel SID B cannot DMA tenant A's pins.
        d.set_sid(&cap_b, sid_b).unwrap();
        assert_eq!(d.submit(&job_a).unwrap_err(), HalError::Fault);
        assert_eq!(d.iommu.submit_sid(), None);
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid_b));
        drop(d);
        let out_a = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        let out_b = i32::from_le_bytes(backing[288..292].try_into().unwrap());
        assert_eq!(out_a, 19);
        assert_eq!(out_b, 9);
    }

    #[test]
    fn inject_wrong_sid_aborts_dma() {
        let (mut backing, job) = matmul_backing();
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        d.inject_wrong_sid(StreamId::accel(ChipletId(0), TileId(0), 7));
        assert_eq!(d.service().unwrap().status, -2);
        assert_eq!(d.iommu.submit_sid(), None);
    }

    fn place_sid(chiplet: u8, tile: u16) -> (Place, StreamId) {
        let place =
            Place::new(ChipletId(chiplet), aether_core::space::MemorySpace::Host).with_tile(tile);
        let sid = StreamId::accel(ChipletId(chiplet), TileId(tile), CP_SSID);
        (place, sid)
    }

    fn write_matmul_pair(backing: &mut [u8], base: usize) {
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            let o = base + i * 4;
            backing[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            let o = base + 16 + i * 4;
            backing[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    fn two_queue_jobs(backing: &mut [u8]) -> (AccelJobDesc, AccelJobDesc, StreamId, StreamId) {
        write_matmul_pair(backing, 0);
        write_matmul_pair(backing, 64);
        let (place_a, sid_a) = place_sid(0, 2);
        let (place_b, sid_b) = place_sid(1, 3);
        let mut job_a =
            AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job_a.place = place_a;
        let mut job_b =
            AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(64), PhysAddr(80), PhysAddr(96), 2);
        job_b.place = place_b;
        (job_a, job_b, sid_a, sid_b)
    }

    #[test]
    fn xqueue_suspend_a_b_progresses_blast_radius() {
        let mut backing = [0u8; 256];
        let (job_a, job_b, sid_a, sid_b) = two_queue_jobs(&mut backing);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        d.create_xqueue(0, sid_a, 0).unwrap();
        d.create_xqueue(1, sid_b, 1).unwrap();
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 48, sid_a))
            .unwrap();
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(64), 48, sid_b))
            .unwrap();

        d.submit_xqueue(0, &job_a).unwrap();
        d.submit_xqueue(1, &job_b).unwrap();
        assert_eq!(d.xqueue(0).unwrap().pending(), 1);
        assert_eq!(d.xqueue(1).unwrap().pending(), 1);
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid_a));
        assert_eq!(d.xqueue(1).unwrap().sid, Some(sid_b));

        let level = d.suspend_xqueue(0).unwrap();
        assert_eq!(level, PreemptionLevel::QueueBoundary);
        assert_ne!(level, PreemptionLevel::MidOp);
        assert_eq!(d.xqueue(0).unwrap().state, XQueueState::Suspended);
        assert!(d.doorbell_pending(), "B is Running with work");

        let serviced = d.service().unwrap();
        assert_eq!(serviced.status, 0);
        assert_eq!(d.last_queue(), Some(1));
        assert_eq!(d.poll().unwrap().status, 0);
        assert_eq!(d.xqueue(0).unwrap().pending(), 1, "A stayed frozen");
        assert_eq!(d.xqueue(1).unwrap().pending(), 0);
        assert!(d.service().is_none(), "A is suspended; no mid-op steal");
        let out_b = i32::from_le_bytes(d.mem.bytes[96..100].try_into().unwrap());
        let out_a = i32::from_le_bytes(d.mem.bytes[32..36].try_into().unwrap());
        assert_eq!(out_b, 19, "B DMA under SID_B completed");
        assert_eq!(out_a, 0, "A C tensor untouched while frozen");

        d.resume_xqueue(0).unwrap();
        assert_eq!(d.xqueue(0).unwrap().state, XQueueState::Running);
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.last_queue(), Some(0));
        assert_eq!(d.poll().unwrap().status, 0);
        let out_a = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(out_a, 19);
    }

    #[test]
    fn xqueue_priority_picks_b_while_both_running() {
        let mut backing = [0u8; 256];
        let (job_a, job_b, sid_a, sid_b) = two_queue_jobs(&mut backing);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        d.create_xqueue(0, sid_a, 0).unwrap();
        d.create_xqueue(1, sid_b, 7).unwrap();
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 48, sid_a))
            .unwrap();
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(64), 48, sid_b))
            .unwrap();
        d.submit_xqueue(0, &job_a).unwrap();
        d.submit_xqueue(1, &job_b).unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.last_queue(), Some(1));
        let out_b = i32::from_le_bytes(d.mem.bytes[96..100].try_into().unwrap());
        let out_a = i32::from_le_bytes(d.mem.bytes[32..36].try_into().unwrap());
        assert_eq!(out_b, 19);
        assert_eq!(out_a, 0);
    }

    #[test]
    fn xqueue_wrong_sid_still_aborts() {
        let mut backing = [0u8; 256];
        let (job_a, _job_b, sid_a, sid_b) = two_queue_jobs(&mut backing);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        d.create_xqueue(0, sid_a, 0).unwrap();
        d.create_xqueue(1, sid_b, 0).unwrap();
        // Pins live on B; queue A is stamped SID_A.
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 48, sid_b))
            .unwrap();
        assert_eq!(d.submit_xqueue(0, &job_a).unwrap_err(), HalError::Fault);

        // Job place packs SID_B; queue A will not accept a foreign stamp.
        let mut foreign = job_a;
        foreign.place =
            Place::new(ChipletId(1), aether_core::space::MemorySpace::Host).with_tile(3);
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(64), 48, sid_a))
            .unwrap();
        assert_eq!(d.submit_xqueue(0, &foreign).unwrap_err(), HalError::Fault);
    }

    #[test]
    fn xqueue_stamp_hook_inherits_then_refuses_override() {
        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        let sid = stream_for_job(&job);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid));
        assert_eq!(
            d.last_cmd().unwrap().flags & CP_FLAG_SET_SID,
            CP_FLAG_SET_SID
        );
        d.stamp_queue_sid(0, sid).unwrap();
        let other = StreamId::accel(ChipletId(0), TileId(2), 7);
        assert_eq!(d.stamp_queue_sid(0, other).unwrap_err(), HalError::Busy);
        // Pending work keeps the inherited SID; a foreign place cannot override.
        let mut foreign = job;
        foreign.place =
            Place::new(ChipletId(1), aether_core::space::MemorySpace::Host).with_tile(3);
        assert_eq!(d.submit(&foreign).unwrap_err(), HalError::Fault);
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid));
        assert_eq!(d.service().unwrap().status, 0);
        d.poll();
        // After drain, a second submit on queue 0 reuses the sticky SID.
        d.submit(&job).unwrap();
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid));
        assert_eq!(d.last_cmd().unwrap().stream_id, sid.raw());
        assert_eq!(
            AccelDevice::create_queue(&mut d, 2, sid.raw(), 0).unwrap_err(),
            HalError::BadArg
        );
        assert_eq!(d.suspend_xqueue(9).unwrap_err(), HalError::BadArg);
    }

    #[test]
    fn iree_shaped_stays_device_mailbox() {
        // XQueue this cut is Soft-CP only. IreeShapedCp keeps defaults.
        use crate::IreeShapedCp;
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = IreeShapedCp::new(mem);
        assert_eq!(d.probe().unwrap().n_queues, 1);
        assert_eq!(
            AccelDevice::create_queue(&mut d, 0, 0, 0).unwrap_err(),
            HalError::Unsupported
        );
        assert_eq!(
            AccelDevice::suspend_queue(&mut d, 0).unwrap_err(),
            HalError::Unsupported
        );
    }

    /// AccelDevice::map / Soft-CP SID bind twin of `docs/bringup/smmu_replay.jsonl`.
    /// Default Soft-CP map is Nested + identity Stage-2 (not `bind_nested`).
    #[test]
    fn bringup_accel_map_soft_cp_sid_bind() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let sid = kit_cp_sid();
        assert_eq!(
            AccelDevice::map(&mut d, MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, sid)),
            Err(HalError::NoMemoryCap)
        );
        assert_eq!(d.iommu.capture(sid).unwrap(), StreamState::Captured);
        assert_eq!(
            d.iommu
                .translate_result(sid.raw(), PhysAddr(0x1000), None)
                .unwrap_err(),
            MapError::StreamAbort
        );
        assert_eq!(d.bind_stream(&mem_cap(), sid).unwrap(), StreamState::Bound);
        let iova = d
            .map_with_cap(
                &mem_cap(),
                MapRequest::pin_accel(PhysAddr(0x1000), 0x1000, sid),
            )
            .unwrap();
        assert!(iova.0 >= SOFT_SMMU_IOVA_BASE);
        assert_ne!(iova.0, 0x1000);
        let w = d.iommu.walk(sid, iova).unwrap();
        assert_eq!(w.pa.0, 0x1000);
        assert_eq!(w.ipa.0, 0x1000, "Soft-CP default map: identity Stage-2");
        assert_eq!(w.config, SteConfig::Nested);
        assert_eq!(
            d.iommu
                .translate_result(kit_wrong_sid().raw(), PhysAddr(0x1000), None)
                .unwrap_err(),
            MapError::StreamAbort
        );
        d.bind_stream(&mem_cap(), kit_iree_sid()).unwrap();
        assert_eq!(
            d.iommu
                .translate_result(kit_iree_sid().raw(), PhysAddr(0x1000), None)
                .unwrap_err(),
            MapError::WrongStream
        );
        assert_eq!(d.iommu.resolve_ats(sid.raw(), iova).unwrap().0, 0x1000);
        let dropped = d
            .iommu
            .invalidate(InvCmd::Ats {
                sid,
                iova: Some(iova),
                len: 0x1000,
            })
            .unwrap();
        assert_eq!(dropped, 1);
        assert_eq!(d.iommu.walk(sid, iova).unwrap().pa.0, 0x1000);
        let dump = d.iommu.dump();
        assert_eq!(dump.stes[0].as_ref().unwrap().state, StreamState::Bound);
    }

    #[test]
    fn chipsync_two_chiplet_producer_consumer_package_lt_naive() {
        let mut backing = [0u8; 256];
        let (job_a, job_b, sid_a, sid_b) = two_queue_jobs(&mut backing);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        d.create_xqueue(0, sid_a, 0).unwrap();
        d.create_xqueue(1, sid_b, 0).unwrap();
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(0), 48, sid_a))
            .unwrap();
        d.map_with_cap(&mem_cap(), MapRequest::pin_accel(PhysAddr(64), 48, sid_b))
            .unwrap();

        let buf = BufferLabel(1);
        d.chipsync.enable_cct(true);
        d.chipsync.open(SyncScope::Package);
        d.chipsync
            .expect(
                ChipletId(0),
                aether_core::chipsync::DEMO_WORKERS_PER_CHIPLET,
            )
            .unwrap();
        for _ in 0..(aether_core::chipsync::DEMO_WORKERS_PER_CHIPLET - 1) {
            let f = d.chipsync.arrive(ChipletId(0), Some(buf)).unwrap();
            assert_eq!(f.kind, SignalKind::ChipletLocal);
        }

        d.submit_scoped(0, &job_a, SyncScope::Package, Some(buf), None)
            .unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.last_queue(), Some(0));
        assert_eq!(
            d.last_scoped().unwrap().kind,
            SignalKind::PendingPackage,
            "last worker defers package fence for CCT"
        );
        d.poll();

        d.submit_scoped(1, &job_b, SyncScope::Package, None, Some(buf))
            .unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.last_queue(), Some(1));
        assert_eq!(d.last_scoped().unwrap().kind, SignalKind::PackageFence);
        d.poll();

        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid_a), "SID still sticky");
        assert_eq!(d.xqueue(1).unwrap().sid, Some(sid_b));
        assert_eq!(d.chipsync.package_fences(), 1);
        assert_eq!(
            d.chipsync.naive_package_fences(),
            aether_core::chipsync::DEMO_WORKERS_PER_CHIPLET
        );
        assert!(d.chipsync.package_lt_naive());
        assert_eq!(d.chipsync.elided(), 0);
        assert_eq!(d.chipsync.cct().last_writer(buf), Some(ChipletId(0)));
        let out_a = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        let out_b = i32::from_le_bytes(backing[96..100].try_into().unwrap());
        assert_eq!(out_a, 19);
        assert_eq!(out_b, 19);
    }

    #[test]
    fn chipsync_cct_elides_same_chiplet_consumer() {
        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        let sid = stream_for_job(&job);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        pin_job(&mut d, &job);
        let buf = BufferLabel(4);
        d.chipsync.enable_cct(true);
        d.chipsync.open(SyncScope::Package);
        d.chipsync.expect(ChipletId(0), 2).unwrap();

        d.submit_scoped(0, &job, SyncScope::Package, Some(buf), None)
            .unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        d.poll();
        d.submit_scoped(0, &job, SyncScope::Package, Some(buf), None)
            .unwrap();
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.last_scoped().unwrap().kind, SignalKind::PendingPackage);
        d.poll();

        assert_eq!(
            d.chipsync.wait(ChipletId(0), Some(buf)).unwrap(),
            SignalKind::Elided
        );
        assert_eq!(d.chipsync.package_fences(), 0);
        assert_eq!(d.chipsync.elided(), 1);
        assert!(d.chipsync.package_lt_naive());
        // XQueue SID-at-submit still sticky after scoped submits.
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid));
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.flags & CP_FLAG_SET_SID, CP_FLAG_SET_SID);
    }

    #[test]
    fn firewall_golden_submit_still_executes() {
        use crate::firewall::{FirewallMode, RacingCmdStream, CP_OFF_OPCODE};

        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let iova = pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        assert!(d.last_firewall_sim().noted(), "copy+validate sim note");
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.opcode, AccelOp::MatMul as u32 as u8);
        assert_eq!(cmd.iova_a, iova.0);
        assert_eq!(d.service().unwrap().status, 0);
        d.poll();

        // Cmdbuf path: copy-then-validate a racing client; mutation ignored.
        let good = cmd.to_le_bytes();
        let mut poison = good;
        poison[CP_OFF_OPCODE] = 0x7F;
        let mut race = RacingCmdStream::new(good, poison, 1);
        d.firewall.mode = FirewallMode::CopyThenValidate;
        d.submit_cmdbuf(0, &mut race, &job).unwrap();
        assert!(race.mutated());
        assert_eq!(
            d.last_cmd().unwrap().opcode,
            AccelOp::MatMul as u32 as u8,
            "firewall ignores the rewrite"
        );
        assert_eq!(d.service().unwrap().status, 0);
        let out0 = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(out0, 19);
    }

    #[test]
    fn firewall_in_place_mutation_sneaks_on_cmdbuf() {
        use crate::firewall::{FirewallMode, RacingCmdStream, CP_OFF_IOVA_A};

        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        pin_job(&mut d, &job);
        d.submit(&job).unwrap();
        let good = d.last_cmd().unwrap().to_le_bytes();
        d.service();
        d.poll();

        let mut poison = good;
        poison[CP_OFF_IOVA_A..CP_OFF_IOVA_A + 8].copy_from_slice(&0x1000u64.to_le_bytes());
        let mut race = RacingCmdStream::new(good, poison, 1);
        d.firewall.mode = FirewallMode::ValidateInPlace;
        d.submit_cmdbuf(0, &mut race, &job).unwrap();
        assert_eq!(
            d.last_cmd().unwrap().iova_a,
            0x1000,
            "without copy the identity PA sneaks onto the queue"
        );
        // Soft SMMU will not resolve a guest PA that skipped the IOVA window.
        let cpl = d.service().unwrap();
        assert_ne!(cpl.status, 0);
    }
}
