//! Software NPU tile bound to the VirtIO-Accel queue.
//!
//! This is the QEMU demo backend: no custom QEMU device is required. The
//! same `AccelDevice` impl is what a partner replaces with MMIO + IRQ.

use aether_core::accel::{AccelJobDesc, Completion, DmaView, SoftNpu};
use aether_core::types::PhysAddr;
use aether_hal::{AccelDevice, AccelInfo, HalError};

use crate::virtio_accel::VirtioAccelQueue;

/// Identity-mapped physical memory (kernel). Host tests use `SliceDma`.
pub struct IdentityDma;

impl DmaView for IdentityDma {
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, aether_core::accel::AccelError> {
        let p = addr.0 as *const i32;
        // SAFETY: caller mapped + owns the arena (cap + transfer).
        Ok(unsafe { core::ptr::read_volatile(p) })
    }
    fn store_i32(
        &mut self,
        addr: PhysAddr,
        val: i32,
    ) -> Result<(), aether_core::accel::AccelError> {
        let p = addr.0 as *mut i32;
        unsafe { core::ptr::write_volatile(p, val) };
        Ok(())
    }
}

/// DMA through a caller-provided buffer (demo / tests).
pub struct FnDma<L, S>
where
    L: Fn(PhysAddr) -> i32,
    S: FnMut(PhysAddr, i32),
{
    pub load: L,
    pub store: S,
}

impl<L, S> DmaView for FnDma<L, S>
where
    L: Fn(PhysAddr) -> i32,
    S: FnMut(PhysAddr, i32),
{
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, aether_core::accel::AccelError> {
        Ok((self.load)(addr))
    }
    fn store_i32(
        &mut self,
        addr: PhysAddr,
        val: i32,
    ) -> Result<(), aether_core::accel::AccelError> {
        (self.store)(addr, val);
        Ok(())
    }
}

pub struct SoftNpuDevice<M: DmaView> {
    pub info: AccelInfo,
    pub queue: VirtioAccelQueue,
    pub npu: SoftNpu,
    pub mem: M,
    mapped: bool,
}

impl<M: DmaView> SoftNpuDevice<M> {
    pub fn new(mem: M) -> Self {
        Self {
            info: AccelInfo {
                vendor: 0xAE7E,
                device: 0x0001,
                n_queues: 1,
                max_wave: 64,
                backend: 0,
            },
            queue: VirtioAccelQueue::new(),
            npu: SoftNpu::new(),
            mem,
            mapped: false,
        }
    }

    /// Drain the avail ring into the reference NPU (the "device thread").
    pub fn service(&mut self) -> Option<Completion> {
        let (_slot, job) = self.queue.take_avail()?;
        match self.npu.execute(&job, &mut self.mem) {
            Ok(cpl) => {
                self.queue.complete(_slot, cpl);
                Some(cpl)
            }
            Err(_) => {
                let cpl = Completion {
                    job_seq: self.npu.seq,
                    status: -1,
                    cycles: 0,
                };
                self.queue.complete(_slot, cpl);
                Some(cpl)
            }
        }
    }
}

impl<M: DmaView> AccelDevice for SoftNpuDevice<M> {
    fn probe(&mut self) -> Result<AccelInfo, HalError> {
        if !self.queue.negotiate() {
            return Err(HalError::NotFound);
        }
        Ok(self.info)
    }

    fn submit(&mut self, job: &AccelJobDesc) -> Result<u32, HalError> {
        self.queue.submit(*job).map_err(|_| HalError::Busy)
    }

    fn poll(&mut self) -> Option<Completion> {
        if let Some(c) = self.queue.poll_used() {
            return Some(c);
        }
        let _ = self.service();
        self.queue.poll_used()
    }

    fn map(&mut self, _base: PhysAddr, _size: u64) -> Result<(), HalError> {
        self.mapped = true;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "softnpu+virtio-accel"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::accel::SliceMem;
    use aether_hal::AccelDevice;

    #[test]
    fn submit_poll_matmul() {
        let mut buf = [0u8; 256];
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            buf[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut buf,
        };
        // SoftNpuDevice needs to own mem — rebuild with a raw copy path
        drop(mem);
        let mut backing = buf;
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut dev = SoftNpuDevice::new(mem);
        assert!(dev.probe().is_ok());
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        dev.submit(&job).unwrap();
        let c = dev.poll().unwrap();
        assert_eq!(c.status, 0);
        let out0 = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(out0, 19);
    }
}
