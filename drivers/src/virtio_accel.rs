//! VirtIO-Accel: a virtio-inspired job/completion ring.
//!
//! Host-tested ring model. The live QEMU path uses [`crate::mmio::AccelMmio`]
//! (doorbell + used-ring IRQ). SpecForge Y1H1 path B: that in-kernel BAR
//! is the canonical demo for stock QEMU; this module keeps the same
//! frozen ABI in Rust structs so unit tests do not need a byte BAR.
//! Path A is the optional QEMU device in `qemu/` (`make qemu-accel`) plus
//! the host Soft-SMMU IOVA contract in [`crate::path_a`].
//!
//! ```text
//! MMIO cfg @ BAR0 (frozen)
//!   0x00 magic    u32  = 0xAE7EACC1
//!   0x04 version  u32  = 1
//!   0x08 status   u32  (ACK, DRIVER, DRIVER_OK, FAILED)
//!   0x0C qsize    u32
//!   0x10 doorbell u32  (write = kick)
//!   0x14 used_idx u32  (device-updated)
//!
//! Queue: descriptor table of AccelJobDesc, avail ring, used ring
//!   (completions). Same split-virtqueue idea as virtio 1.2, specialized
//!   for wave submit — no general-purpose scatter-gather in v0.1.
//! ```
//!
//! A real NPU driver would:
//! 1. `probe` PCI/MMIO, check magic/version
//! 2. `map` tensor arenas (IOMMU / SMMU + our Memory caps)
//! 3. `submit` by filling a desc + ringing doorbell
//! 4. complete via IRQ/MSI-X → fabric Notification cap

use aether_core::accel::{AccelJobDesc, Completion};

pub const VIRTIO_ACCEL_MAGIC: u32 = 0xAE7E_ACC1;
pub const VIRTIO_ACCEL_VERSION: u32 = 1;
pub const VIRTQ_SIZE: usize = 8;

pub const STATUS_ACK: u32 = 1;
pub const STATUS_DRIVER: u32 = 2;
pub const STATUS_DRIVER_OK: u32 = 4;
pub const STATUS_FAILED: u32 = 128;

#[derive(Clone, Copy, Debug)]
pub struct VirtqUsed {
    pub job_seq: u32,
    pub status: i32,
    pub cycles: u32,
}

/// Software virtqueue. A hardware device would DMA these structures.
#[derive(Clone, Debug)]
pub struct VirtioAccelQueue {
    pub magic: u32,
    pub version: u32,
    pub status: u32,
    pub avail: [Option<AccelJobDesc>; VIRTQ_SIZE],
    pub avail_idx: usize,
    pub used: [Option<VirtqUsed>; VIRTQ_SIZE],
    pub used_idx: usize,
    pub kicked: bool,
}

impl VirtioAccelQueue {
    pub const fn new() -> Self {
        Self {
            magic: VIRTIO_ACCEL_MAGIC,
            version: VIRTIO_ACCEL_VERSION,
            status: 0,
            avail: [None; VIRTQ_SIZE],
            avail_idx: 0,
            used: [None; VIRTQ_SIZE],
            used_idx: 0,
            kicked: false,
        }
    }

    pub fn negotiate(&mut self) -> bool {
        if self.magic != VIRTIO_ACCEL_MAGIC || self.version != VIRTIO_ACCEL_VERSION {
            self.status = STATUS_FAILED;
            return false;
        }
        self.status = STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK;
        true
    }

    pub fn submit(&mut self, job: AccelJobDesc) -> Result<u32, ()> {
        if self.status & STATUS_FAILED != 0 {
            return Err(());
        }
        if self.avail[self.avail_idx % VIRTQ_SIZE].is_some() {
            return Err(());
        }
        let slot = self.avail_idx;
        self.avail[slot % VIRTQ_SIZE] = Some(job);
        self.avail_idx += 1;
        self.kicked = true;
        Ok(slot as u32)
    }

    pub fn take_avail(&mut self) -> Option<(usize, AccelJobDesc)> {
        for i in 0..VIRTQ_SIZE {
            if let Some(job) = self.avail[i].take() {
                return Some((i, job));
            }
        }
        self.kicked = false;
        None
    }

    pub fn complete(&mut self, _slot: usize, cpl: Completion) {
        let i = self.used_idx % VIRTQ_SIZE;
        self.used[i] = Some(VirtqUsed {
            job_seq: cpl.job_seq,
            status: cpl.status,
            cycles: cpl.cycles,
        });
        self.used_idx += 1;
    }

    pub fn poll_used(&mut self) -> Option<Completion> {
        for i in 0..VIRTQ_SIZE {
            if let Some(u) = self.used[i].take() {
                return Some(Completion {
                    job_seq: u.job_seq,
                    status: u.status,
                    cycles: u.cycles,
                });
            }
        }
        None
    }
}

impl Default for VirtioAccelQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::accel::AccelJobDesc;
    use aether_core::types::PhysAddr;

    #[test]
    fn negotiate_and_ring() {
        let mut q = VirtioAccelQueue::new();
        assert!(q.negotiate());
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(0), PhysAddr(0), 1);
        q.submit(job).unwrap();
        let (slot, _j) = q.take_avail().unwrap();
        q.complete(
            slot,
            Completion {
                job_seq: 1,
                status: 0,
                cycles: 8,
            },
        );
        let c = q.poll_used().unwrap();
        assert_eq!(c.cycles, 8);
    }

    #[test]
    fn bad_magic_fails() {
        let mut q = VirtioAccelQueue::new();
        q.magic = 0;
        assert!(!q.negotiate());
        assert_eq!(q.status, STATUS_FAILED);
    }

    fn job(seq: u32) -> AccelJobDesc {
        AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(0), PhysAddr(0), seq)
    }

    #[test]
    fn submit_refused_after_failed_negotiate() {
        let mut q = VirtioAccelQueue::new();
        q.version = VIRTIO_ACCEL_VERSION + 1;
        assert!(!q.negotiate());
        assert!(q.submit(job(1)).is_err());
        assert_eq!(q.avail_idx, 0);
        assert!(!q.kicked);
        assert!(q.take_avail().is_none());
    }

    #[test]
    fn full_ring_refuses_then_accepts_after_drain() {
        let mut q = VirtioAccelQueue::new();
        assert!(q.negotiate());
        for i in 0..VIRTQ_SIZE {
            assert_eq!(q.submit(job(i as u32)).unwrap(), i as u32);
        }
        assert!(q.kicked);
        // Ring full: the next slot (0 mod VIRTQ_SIZE) is still occupied.
        assert!(q.submit(job(99)).is_err());
        assert_eq!(q.avail_idx, VIRTQ_SIZE);
        // Draining slot 0 frees exactly that slot; the slot number keeps counting.
        let (slot, _) = q.take_avail().unwrap();
        assert_eq!(slot, 0);
        assert_eq!(q.submit(job(8)).unwrap(), VIRTQ_SIZE as u32);
        assert!(q.submit(job(9)).is_err());
    }

    #[test]
    fn take_avail_on_empty_ring_clears_kick() {
        let mut q = VirtioAccelQueue::new();
        assert!(q.negotiate());
        q.submit(job(1)).unwrap();
        assert!(q.kicked);
        assert!(q.take_avail().is_some());
        assert!(q.kicked);
        assert!(q.take_avail().is_none());
        assert!(!q.kicked);
    }

    #[test]
    fn used_ring_returns_each_completion_once() {
        let mut q = VirtioAccelQueue::new();
        assert!(q.poll_used().is_none());
        for seq in 1..=3u32 {
            q.complete(
                0,
                Completion {
                    job_seq: seq,
                    status: 0,
                    cycles: seq * 10,
                },
            );
        }
        let mut seen = [false; 4];
        while let Some(c) = q.poll_used() {
            assert_eq!(c.cycles, c.job_seq * 10);
            assert!(!seen[c.job_seq as usize]);
            seen[c.job_seq as usize] = true;
        }
        assert_eq!(seen, [false, true, true, true]);
        assert!(q.poll_used().is_none());
    }

    #[test]
    fn frozen_bar_constants_match_mmio() {
        use crate::mmio::{
            GOLDEN_CFG_OFFS, REG_DOORBELL, REG_MAGIC, REG_QSIZE, REG_STATUS, REG_USED_IDX,
            REG_VERSION,
        };
        assert_eq!(VIRTIO_ACCEL_MAGIC, 0xAE7E_ACC1);
        assert_eq!(VIRTIO_ACCEL_VERSION, 1);
        assert_eq!(VIRTQ_SIZE, 8);
        assert_eq!(REG_MAGIC, 0x00);
        assert_eq!(REG_VERSION, 0x04);
        assert_eq!(REG_STATUS, 0x08);
        assert_eq!(REG_QSIZE, 0x0C);
        assert_eq!(REG_DOORBELL, 0x10);
        assert_eq!(REG_USED_IDX, 0x14);
        assert_eq!(
            GOLDEN_CFG_OFFS,
            &[
                REG_MAGIC,
                REG_VERSION,
                REG_STATUS,
                REG_QSIZE,
                REG_DOORBELL,
                REG_USED_IDX
            ]
        );
    }
}
