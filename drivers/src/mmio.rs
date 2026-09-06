//! Virtqueue-shaped MMIO window for VirtIO-Accel.
//!
//! This is the register file a future `-device virtio-accel` would expose.
//! v0.1 emulates it in-kernel: the kernel driver pokes these offsets; the
//! SoftNPU backend services the avail ring when the doorbell is kicked
//! and raises a used-ring IRQ flag. There is no custom QEMU device.

use aether_core::accel::{AccelJobDesc, AccelOp, Completion, DType};
use aether_core::partition::PartitionId;
use aether_core::phase::Phase;
use aether_core::space::{MemorySpace, Place};
use aether_core::types::{ChipletId, PhysAddr};

use crate::virtio_accel::{
    STATUS_ACK, STATUS_DRIVER, STATUS_DRIVER_OK, STATUS_FAILED, VIRTIO_ACCEL_MAGIC,
    VIRTIO_ACCEL_VERSION, VIRTQ_SIZE,
};

/// Byte length of the emulated BAR. Cfg + rings fit comfortably.
pub const ACCEL_MMIO_SIZE: usize = 0x400;

pub const REG_MAGIC: usize = 0x00;
pub const REG_VERSION: usize = 0x04;
pub const REG_STATUS: usize = 0x08;
pub const REG_QSIZE: usize = 0x0C;
pub const REG_DOORBELL: usize = 0x10;
pub const REG_USED_IDX: usize = 0x14;
pub const REG_IRQ_STATUS: usize = 0x18;
pub const REG_IRQ_ACK: usize = 0x1C;
pub const REG_AVAIL_IDX: usize = 0x20;

pub const IRQ_USED: u32 = 1;

/// Avail ring: 8 packed job descs starting here.
pub const AVAIL_BASE: usize = 0x80;
/// Used ring: 8 × 16-byte completions (after 8 × 88-byte jobs).
pub const USED_BASE: usize = 0x340;
pub const JOB_WIRE_SIZE: usize = 88;
pub const USED_WIRE_SIZE: usize = 16;

/// Packed job written into the avail ring (MMIO ABI, not the Rust layout).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AccelJobWire {
    pub op: u32,
    pub flags: u32,
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub tenant: u32,
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub bias: u64,
    pub fence_id: u64,
    pub a_stride: u32,
    pub b_stride: u32,
    pub c_stride: u32,
    pub completion_ep: u32,
    pub partition: u32,
    pub dtype: u8,
    pub space: u8,
    pub phase: u8,
    pub _pad: u8,
}

const _: [(); JOB_WIRE_SIZE] = [(); core::mem::size_of::<AccelJobWire>()];

impl AccelJobWire {
    pub fn from_job(j: &AccelJobDesc) -> Self {
        Self {
            op: j.op as u32,
            flags: j.flags,
            m: j.m,
            n: j.n,
            k: j.k,
            tenant: j.tenant,
            a: j.a.0,
            b: j.b.0,
            c: j.c.0,
            bias: j.bias.0,
            fence_id: j.fence_id,
            a_stride: j.a_stride,
            b_stride: j.b_stride,
            c_stride: j.c_stride,
            completion_ep: j.completion_ep,
            partition: j.partition.0,
            dtype: j.dtype as u8,
            space: j.space as u8,
            phase: j.phase as u8,
            _pad: 0,
        }
    }

    pub fn to_job(self) -> AccelJobDesc {
        AccelJobDesc {
            op: AccelOp::from_u32(self.op).unwrap_or(AccelOp::Nop),
            flags: self.flags,
            m: self.m,
            n: self.n,
            k: self.k,
            a: PhysAddr(self.a),
            b: PhysAddr(self.b),
            c: PhysAddr(self.c),
            bias: PhysAddr(self.bias),
            a_stride: self.a_stride,
            b_stride: self.b_stride,
            c_stride: self.c_stride,
            dtype: DType::from_u8(self.dtype).unwrap_or(DType::I32),
            tenant: self.tenant,
            completion_ep: self.completion_ep,
            space: match self.space {
                1 => MemorySpace::DeviceHbm,
                2 => MemorySpace::TileSram,
                3 => MemorySpace::CxlRegion,
                4 => MemorySpace::Scratch,
                5 => MemorySpace::Streaming,
                _ => MemorySpace::Host,
            },
            place: Place::new(ChipletId(0), MemorySpace::Host),
            phase: match self.phase {
                1 => Phase::Exchange,
                2 => Phase::Barrier,
                _ => Phase::Compute,
            },
            partition: PartitionId(self.partition),
            fence_id: self.fence_id,
        }
    }
}

/// In-kernel emulated BAR0. Driver and device both poke this window.
#[derive(Clone, Debug)]
pub struct AccelMmio {
    bytes: [u8; ACCEL_MMIO_SIZE],
}

impl AccelMmio {
    pub fn new() -> Self {
        let mut s = Self {
            bytes: [0; ACCEL_MMIO_SIZE],
        };
        s.write_u32(REG_MAGIC, VIRTIO_ACCEL_MAGIC);
        s.write_u32(REG_VERSION, VIRTIO_ACCEL_VERSION);
        s.write_u32(REG_QSIZE, VIRTQ_SIZE as u32);
        s.write_u32(REG_STATUS, 0);
        s
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn read_u32(&self, off: usize) -> u32 {
        if off + 4 > ACCEL_MMIO_SIZE {
            return 0;
        }
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.bytes[off..off + 4]);
        u32::from_le_bytes(b)
    }

    pub fn write_u32(&mut self, off: usize, v: u32) {
        if off + 4 > ACCEL_MMIO_SIZE {
            return;
        }
        self.bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn negotiate(&mut self) -> bool {
        if self.read_u32(REG_MAGIC) != VIRTIO_ACCEL_MAGIC
            || self.read_u32(REG_VERSION) != VIRTIO_ACCEL_VERSION
        {
            self.write_u32(REG_STATUS, STATUS_FAILED);
            return false;
        }
        self.write_u32(REG_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK);
        true
    }

    pub fn status(&self) -> u32 {
        self.read_u32(REG_STATUS)
    }

    pub fn doorbell_pending(&self) -> bool {
        self.read_u32(REG_DOORBELL) != 0
    }

    pub fn irq_pending(&self) -> bool {
        self.read_u32(REG_IRQ_STATUS) & IRQ_USED != 0
    }

    /// Driver: write a job into the next avail slot and kick the doorbell.
    pub fn driver_submit(&mut self, job: &AccelJobDesc) -> Result<u32, ()> {
        if self.status() & STATUS_FAILED != 0 {
            return Err(());
        }
        let idx = self.read_u32(REG_AVAIL_IDX);
        let used = self.read_u32(REG_USED_IDX);
        if idx.wrapping_sub(used) as usize >= VIRTQ_SIZE {
            return Err(());
        }
        let slot = (idx as usize) % VIRTQ_SIZE;
        self.write_job(slot, &AccelJobWire::from_job(job));
        self.write_u32(REG_AVAIL_IDX, idx + 1);
        self.write_u32(REG_DOORBELL, 1);
        Ok(idx)
    }

    /// Device: pop one avail job if the doorbell was kicked.
    pub fn device_take_avail(&mut self) -> Option<(u32, AccelJobDesc)> {
        if !self.doorbell_pending() {
            return None;
        }
        let avail = self.read_u32(REG_AVAIL_IDX);
        let taken = self.read_u32(REG_USED_IDX);
        // Device-local cursor: how many we have already consumed lives in
        // a reserved nibble of irq_ack's high half? Use the used ring fill
        // count: jobs in flight = avail - used (after complete). Before
        // complete, we need a consume cursor. Store it at 0x24.
        const REG_DEV_CURSOR: usize = 0x24;
        let cursor = self.read_u32(REG_DEV_CURSOR);
        if cursor >= avail {
            self.write_u32(REG_DOORBELL, 0);
            return None;
        }
        let slot = (cursor as usize) % VIRTQ_SIZE;
        let job = self.read_job(slot).to_job();
        self.write_u32(REG_DEV_CURSOR, cursor + 1);
        if cursor + 1 >= avail {
            self.write_u32(REG_DOORBELL, 0);
        }
        let _ = taken;
        Some((cursor, job))
    }

    /// Device: publish a used element and raise the used-ring IRQ.
    pub fn device_complete(&mut self, token: u32, cpl: Completion) {
        let idx = self.read_u32(REG_USED_IDX);
        let slot = (idx as usize) % VIRTQ_SIZE;
        self.write_used(slot, cpl);
        self.write_u32(REG_USED_IDX, idx + 1);
        self.write_u32(REG_IRQ_STATUS, self.read_u32(REG_IRQ_STATUS) | IRQ_USED);
        let _ = token;
    }

    /// Driver: read one used completion and ack the IRQ if the ring is empty.
    pub fn driver_poll_used(&mut self) -> Option<Completion> {
        if !self.irq_pending() && self.read_u32(REG_USED_IDX) == 0 {
            return None;
        }
        const REG_DRV_CURSOR: usize = 0x28;
        let cursor = self.read_u32(REG_DRV_CURSOR);
        let used = self.read_u32(REG_USED_IDX);
        if cursor >= used {
            self.write_u32(REG_IRQ_ACK, 1);
            self.write_u32(REG_IRQ_STATUS, self.read_u32(REG_IRQ_STATUS) & !IRQ_USED);
            return None;
        }
        let slot = (cursor as usize) % VIRTQ_SIZE;
        let cpl = self.read_used(slot);
        self.write_u32(REG_DRV_CURSOR, cursor + 1);
        if cursor + 1 >= used {
            self.write_u32(REG_IRQ_ACK, 1);
            self.write_u32(REG_IRQ_STATUS, self.read_u32(REG_IRQ_STATUS) & !IRQ_USED);
        }
        Some(cpl)
    }

    fn write_job(&mut self, slot: usize, job: &AccelJobWire) {
        let off = AVAIL_BASE + slot * JOB_WIRE_SIZE;
        let bytes = unsafe {
            core::slice::from_raw_parts(
                job as *const AccelJobWire as *const u8,
                JOB_WIRE_SIZE,
            )
        };
        self.bytes[off..off + JOB_WIRE_SIZE].copy_from_slice(bytes);
    }

    fn read_job(&self, slot: usize) -> AccelJobWire {
        let off = AVAIL_BASE + slot * JOB_WIRE_SIZE;
        let mut job = AccelJobWire::from_job(&AccelJobDesc::matmul_i32(
            0,
            0,
            0,
            PhysAddr(0),
            PhysAddr(0),
            PhysAddr(0),
            0,
        ));
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.bytes[off..off + JOB_WIRE_SIZE].as_ptr(),
                &mut job as *mut AccelJobWire as *mut u8,
                JOB_WIRE_SIZE,
            );
        }
        job
    }

    fn write_used(&mut self, slot: usize, cpl: Completion) {
        let off = USED_BASE + slot * USED_WIRE_SIZE;
        self.bytes[off..off + 4].copy_from_slice(&cpl.job_seq.to_le_bytes());
        self.bytes[off + 4..off + 8].copy_from_slice(&cpl.status.to_le_bytes());
        self.bytes[off + 8..off + 12].copy_from_slice(&cpl.cycles.to_le_bytes());
    }

    fn read_used(&self, slot: usize) -> Completion {
        let off = USED_BASE + slot * USED_WIRE_SIZE;
        let mut seq = [0u8; 4];
        let mut st = [0u8; 4];
        let mut cy = [0u8; 4];
        seq.copy_from_slice(&self.bytes[off..off + 4]);
        st.copy_from_slice(&self.bytes[off + 4..off + 8]);
        cy.copy_from_slice(&self.bytes[off + 8..off + 12]);
        Completion {
            job_seq: u32::from_le_bytes(seq),
            status: i32::from_le_bytes(st),
            cycles: u32::from_le_bytes(cy),
        }
    }
}

impl Default for AccelMmio {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_offsets() {
        assert_eq!(REG_DOORBELL, 0x10);
        assert_eq!(REG_USED_IDX, 0x14);
        assert_eq!(REG_IRQ_STATUS, 0x18);
        assert_eq!(AVAIL_BASE, 0x80);
        assert_eq!(USED_BASE, 0x340);
        assert_eq!(core::mem::size_of::<AccelJobWire>(), JOB_WIRE_SIZE);
    }

    #[test]
    fn doorbell_then_used_irq() {
        let mut mmio = AccelMmio::new();
        assert!(mmio.negotiate());
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        mmio.driver_submit(&job).unwrap();
        assert!(mmio.doorbell_pending());
        assert!(mmio.driver_poll_used().is_none());

        let (token, got) = mmio.device_take_avail().unwrap();
        assert_eq!(got.m, 2);
        mmio.device_complete(
            token,
            Completion {
                job_seq: 1,
                status: 0,
                cycles: 8,
            },
        );
        assert!(mmio.irq_pending());
        assert!(!mmio.doorbell_pending());
        let c = mmio.driver_poll_used().unwrap();
        assert_eq!(c.cycles, 8);
        assert!(!mmio.irq_pending());
    }

    #[test]
    fn wire_roundtrips_f16_f32_dtype() {
        let mut mmio = AccelMmio::new();
        assert!(mmio.negotiate());
        let job = AccelJobDesc::matmul_f32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        mmio.driver_submit(&job).unwrap();
        let (_, got) = mmio.device_take_avail().unwrap();
        assert_eq!(got.dtype, DType::F32);

        let mut mmio = AccelMmio::new();
        assert!(mmio.negotiate());
        let job = AccelJobDesc::matmul_f16(2, 2, 2, PhysAddr(0), PhysAddr(8), PhysAddr(16), 1);
        mmio.driver_submit(&job).unwrap();
        let (_, got) = mmio.device_take_avail().unwrap();
        assert_eq!(got.dtype, DType::F16);
    }

    #[test]
    fn bad_magic_fails_negotiate() {
        let mut mmio = AccelMmio::new();
        mmio.write_u32(REG_MAGIC, 0);
        assert!(!mmio.negotiate());
        assert_eq!(mmio.status(), STATUS_FAILED);
    }
}
