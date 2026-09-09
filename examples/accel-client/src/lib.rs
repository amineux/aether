//! Tiny host doorbell client: a **second consumer** of frozen [`IreeHalCmd`].
//!
//! [`host/aether-pjrt`](https://github.com/amineux/aether/blob/main/host/aether-pjrt)
//! is the compiler-facing PJRT / IREE HAL-shaped shim. This crate is not that
//! shim, not a PJRT plugin, not NVIDIA, and not a MicroPerceptron port. It
//! allocates a buffer, packs the same 96-byte little-endian image (magic
//! `0xAE7E1EE1`, `ssid = 2`), submits through [`IreeShapedCp`], and waits on
//! an event. Soft SMMU map and Host1x-shaped SID stamp are required; bad
//! `isa_blob_id` and unbound SID use the same refuse rules as the shim.
//!
//! v1 `IreeHalCmd` does not define TRANSFER-only memcpy (`TRANSFER` alone is
//! Fault). Host copies fill buffers; the doorbell itself is `Nop`; compute is
//! a MatMul-shaped `DISPATCH`. Path B / `make qemu` is unchanged.
//! MicroPerceptron remains later and optional.

#![deny(unsafe_code)]

use aether_core::abi::Event as AbiEvent;
use aether_core::accel::{AccelError, AccelJobDesc, AccelOp, Completion, DmaView};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::fence::Timeline;
use aether_core::iommu::{MapRequest, StreamId};
use aether_core::partition::{
    BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
};
use aether_core::space::MemorySpace;
use aether_core::types::{ChipletId, PhysAddr, TenantId, TileId};
use aether_drivers::ireecp::{
    IreeHalCmd, IreeShapedCp, IREE_HAL_CMD_SIZE, IREE_REF_EXECUTABLE, IREE_SSID,
};
use aether_hal::{AccelDevice, AccelInfo, HalError, ACCEL_BACKEND_IREE_SHAPED};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufId(pub u32);

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

/// Doorbell / accel client over one [`IreeShapedCp`]. Same frozen packet as
/// `aether-pjrt`; no Device/Executable noun layer.
pub struct Doorbell {
    cp: IreeShapedCp<HostDma>,
    info: AccelInfo,
    profile: PartitionProfile,
    timeline: Timeline,
    cap: Capability,
    bump: u64,
    bufs: Vec<Alloc>,
    last_cpl: Option<Completion>,
}

impl Doorbell {
    pub fn new() -> Result<Self, Error> {
        let mut cp = IreeShapedCp::new(HostDma::new());
        let info = cp.probe()?;
        if info.backend != ACCEL_BACKEND_IREE_SHAPED {
            return Err(Error::Hal(HalError::Unsupported));
        }
        Ok(Self {
            cp,
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
        })
    }

    pub fn info(&self) -> AccelInfo {
        self.info
    }

    pub fn name(&self) -> &'static str {
        self.cp.name()
    }

    fn stream() -> StreamId {
        StreamId::accel(ChipletId(0), TileId(0), IREE_SSID)
    }

    /// Allocate and pin through Soft SMMU (Memory+MAP, `ssid = 2`).
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
        let iova = self.cp.map_with_cap(
            &self.cap,
            MapRequest::pin_accel(guest_pa, aligned, Self::stream()),
        )?;
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

    /// Explicit host→device copy. Not a HAL `TRANSFER` packet (v1 Fault).
    pub fn copy_i32_from_host(&mut self, id: BufId, vals: &[i32]) -> Result<(), Error> {
        let off = {
            let a = self.alloc(id)?;
            if (vals.len() * 4) as u64 > a.len {
                return Err(Error::Hal(HalError::BadArg));
            }
            self.cp.mem.off(a.guest_pa).map_err(Error::Accel)?
        };
        for (i, v) in vals.iter().enumerate() {
            let o = off + i * 4;
            self.cp.mem.bytes[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        Ok(())
    }

    pub fn copy_i32_to_host(&self, id: BufId, out: &mut [i32]) -> Result<(), Error> {
        let a = self.alloc(id)?;
        if (out.len() * 4) as u64 > a.len {
            return Err(Error::Hal(HalError::BadArg));
        }
        let off = self.cp.mem.off(a.guest_pa).map_err(Error::Accel)?;
        for (i, slot) in out.iter_mut().enumerate() {
            let o = off + i * 4;
            *slot = i32::from_le_bytes(self.cp.mem.bytes[o..o + 4].try_into().unwrap());
        }
        Ok(())
    }

    /// Pin-less Nop doorbell (`command_categories = 0`). Latency probe.
    pub fn ring(&mut self) -> Result<AbiEvent, Error> {
        let mut job = AccelJobDesc::matmul_i32(0, 0, 0, PhysAddr(0), PhysAddr(0), PhysAddr(0), 1);
        job.op = AccelOp::Nop;
        job.space = MemorySpace::Host;
        self.submit_job(job)
    }

    /// MatMul-shaped `DISPATCH` on pinned buffers. Same 96-byte freeze as PJRT.
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

    /// Pack + `submit_hal` a compiler-owned job record. Does not execute.
    pub fn submit_job(&mut self, mut job: AccelJobDesc) -> Result<AbiEvent, Error> {
        job.partition = self.profile.id;
        let fence = self.timeline.submit(&self.profile, None)?;
        job.fence_id = fence.id.0;
        let event = AbiEvent::on_timeline(fence.id, self.profile.id);
        match IreeHalCmd::pack(&job, &self.cp.iommu) {
            Ok(cmd) => self.enqueue(cmd, &job, event),
            Err(e) => {
                let _ = self.timeline.timeout(fence.id);
                Err(Error::Hal(e))
            }
        }
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

    fn enqueue(
        &mut self,
        cmd: IreeHalCmd,
        job: &AccelJobDesc,
        event: AbiEvent,
    ) -> Result<AbiEvent, Error> {
        match self.cp.submit_hal(cmd, job) {
            Ok(_) => {
                if let Some(cmd) = self.cp.last_cmd() {
                    if let Err(e) = Self::check_frozen(&cmd, job) {
                        let _ = self.timeline.timeout(event.fence);
                        return Err(e);
                    }
                }
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

    /// `AccelDevice::map` without a cap walk — second clients cannot skip this.
    pub fn map_without_cap(&mut self, guest_pa: PhysAddr, len: u64) -> Result<PhysAddr, HalError> {
        AccelDevice::map(
            &mut self.cp,
            MapRequest::pin_accel(guest_pa, len, Self::stream()),
        )
    }

    pub fn last_cmd(&self) -> Option<IreeHalCmd> {
        self.cp.last_cmd()
    }

    pub fn last_wire(&self) -> Option<[u8; IREE_HAL_CMD_SIZE]> {
        self.cp.last_cmd().map(|c| c.to_le_bytes())
    }

    /// Pump `service()` then retire the timeline. Host tests have no IRQ thread.
    pub fn wait(&mut self, event: AbiEvent) -> Result<Completion, Error> {
        if event.partition.0 != self.profile.id.0 {
            return Err(Error::Partition(PartitionError::Unbound));
        }
        if self.timeline.wait(event.fence).is_ok() {
            return self.last_cpl.ok_or(Error::NotReady);
        }
        let serviced = self.cp.service().ok_or(Error::NotReady)?;
        let cpl = self.cp.poll().unwrap_or(serviced);
        self.cp.retire_into(&mut self.timeline)?;
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

    pub fn iommu(&self) -> &aether_core::iommu::IommuMap {
        &self.cp.iommu
    }

    pub fn iommu_mut(&mut self) -> &mut aether_core::iommu::IommuMap {
        &mut self.cp.iommu
    }

    pub fn stream_id() -> StreamId {
        Self::stream()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::iommu::{StreamState, SOFT_SMMU_IOVA_BASE};
    use aether_drivers::ireecp::{
        IREE_HAL_COMMAND_CATEGORY_DISPATCH, IREE_HAL_PKT_MAGIC, IREE_REF_EXECUTABLE,
    };
    use aether_hal::{
        ACCEL_BACKEND_PARTNER_STUB, ACCEL_BACKEND_SOFTNPU, ACCEL_BACKEND_SOFT_CP,
        ACCEL_BACKEND_VIRTIO_SOFTNPU,
    };

    #[test]
    fn probe_is_iree_shaped_not_a_new_device() {
        let d = Doorbell::new().unwrap();
        assert_eq!(d.info().backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_eq!(d.name(), "iree-shaped-cp");
        assert_ne!(d.info().backend, ACCEL_BACKEND_SOFTNPU);
        assert_ne!(d.info().backend, ACCEL_BACKEND_VIRTIO_SOFTNPU);
        assert_ne!(d.info().backend, ACCEL_BACKEND_PARTNER_STUB);
        assert_ne!(d.info().backend, ACCEL_BACKEND_SOFT_CP);
        assert_eq!(d.info().n_queues, 1, "still a single mailbox");
    }

    #[test]
    fn allocate_pins_soft_smmu_not_identity() {
        let mut d = Doorbell::new().unwrap();
        let buf = d.allocate(64).unwrap();
        let iova = d.buffer_iova(buf).unwrap();
        let pa = d.buffer_guest_pa(buf).unwrap();
        assert!(iova.0 >= SOFT_SMMU_IOVA_BASE);
        assert_ne!(iova.0, pa.0);
        assert_eq!(
            d.iommu().translate_stream(Doorbell::stream_id().raw(), pa),
            Some(iova)
        );
    }

    #[test]
    fn map_without_cap_is_refused() {
        let mut d = Doorbell::new().unwrap();
        assert_eq!(
            d.map_without_cap(PhysAddr(HEAP_BASE), 64).unwrap_err(),
            HalError::NoMemoryCap
        );
    }

    #[test]
    fn ring_doorbell_wait_event() {
        let mut d = Doorbell::new().unwrap();
        let ev = d.ring().unwrap();
        assert!(!d.fence_ready(ev), "submit does not execute");
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.magic, IREE_HAL_PKT_MAGIC);
        assert_eq!(cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(cmd.command_categories, 0);
        assert_eq!(cmd.function, 0);
        assert_eq!(cmd.decode_op().unwrap(), AccelOp::Nop);
        let cpl = d.wait(ev).unwrap();
        assert_eq!(cpl.status, 0);
        assert!(d.fence_ready(ev));
    }

    #[test]
    fn submit_matmul_wait_event() {
        let mut d = Doorbell::new().unwrap();
        let a = d.allocate(16).unwrap();
        let b = d.allocate(16).unwrap();
        let out = d.allocate(16).unwrap();
        d.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
        d.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
        let ev = d.submit_matmul(2, 2, 2, a, b, out).unwrap();
        assert!(!d.fence_ready(ev));
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.magic, IREE_HAL_PKT_MAGIC);
        assert_eq!(cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(cmd.command_categories, IREE_HAL_COMMAND_CATEGORY_DISPATCH);
        assert_eq!(cmd.executable, IREE_REF_EXECUTABLE);
        assert_eq!(cmd.workgroup_count_x, 2);
        assert_eq!(cmd.workgroup_count_y, 2);
        assert_eq!(cmd.workgroup_count_z, 2);
        assert_eq!(cmd.binding0_length, 16);
        assert_eq!(StreamId::from_raw(cmd.stream_id).ssid(), IREE_SSID);
        d.wait(ev).unwrap();
        let mut got = [0i32; 4];
        d.copy_i32_to_host(out, &mut got).unwrap();
        assert_eq!(got, [19, 22, 43, 50]);
    }

    #[test]
    fn cannot_skip_soft_smmu_map() {
        let mut d = Doorbell::new().unwrap();
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        assert_eq!(d.submit_job(job).unwrap_err(), Error::Hal(HalError::Fault));
    }

    #[test]
    fn cannot_skip_sid_stamp() {
        let mut d = Doorbell::new().unwrap();
        let a = d.allocate(256).unwrap();
        let pa = d.buffer_guest_pa(a).unwrap();
        let mut job =
            AccelJobDesc::matmul_i32(2, 2, 2, pa, PhysAddr(pa.0 + 16), PhysAddr(pa.0 + 32), 1);
        job.place = job.place.with_tile(0);
        let packed = IreeHalCmd::pack(&job, d.iommu()).unwrap();
        assert_eq!(StreamId::from_raw(packed.stream_id).ssid(), IREE_SSID);

        let mut skipped = packed;
        skipped.stream_id = 0; // SoftNPU ssid 0 — skip IREE SID stamp
        assert_eq!(
            d.submit_image(skipped, &job).unwrap_err(),
            Error::Hal(HalError::Fault)
        );
    }

    #[test]
    fn unbound_sid_is_refused() {
        let mut d = Doorbell::new().unwrap();
        let sid = Doorbell::stream_id();
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
        assert_eq!(d.submit_job(job).unwrap_err(), Error::Hal(HalError::Fault));
    }

    #[test]
    fn bad_executable_is_refused() {
        let mut d = Doorbell::new().unwrap();
        let a = d.allocate(256).unwrap();
        let pa = d.buffer_guest_pa(a).unwrap();
        let mut job =
            AccelJobDesc::matmul_i32(2, 2, 2, pa, PhysAddr(pa.0 + 16), PhysAddr(pa.0 + 32), 1);
        job.place = job.place.with_tile(0);
        let mut cmd = IreeHalCmd::pack(&job, d.iommu()).unwrap();
        cmd.executable = 0xDEAD;
        assert_eq!(
            d.submit_image(cmd, &job).unwrap_err(),
            Error::Hal(HalError::Unsupported)
        );
    }

    #[test]
    fn both_clients_submit_same_frozen_image() {
        let mut pjrt = aether_pjrt::Client::iree_shaped().unwrap();
        let a = pjrt.allocate(MemorySpace::Host, 16).unwrap();
        let b = pjrt.allocate(MemorySpace::Host, 16).unwrap();
        let out = pjrt.allocate(MemorySpace::Host, 16).unwrap();
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

        let mut bell = Doorbell::new().unwrap();
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

        assert_eq!(pjrt_cmd.magic, bell_cmd.magic);
        assert_eq!(pjrt_cmd.magic, IREE_HAL_PKT_MAGIC);
        assert_eq!(pjrt_cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(bell_cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(pjrt_cmd.executable, bell_cmd.executable);
        assert_eq!(pjrt_cmd.executable, IREE_REF_EXECUTABLE);
        assert_eq!(StreamId::from_raw(pjrt_cmd.stream_id).ssid(), IREE_SSID);
        assert_eq!(StreamId::from_raw(bell_cmd.stream_id).ssid(), IREE_SSID);
        assert_eq!(pjrt.info().backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_eq!(bell.info().backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_eq!(pjrt.name(), bell.name());
    }
}
