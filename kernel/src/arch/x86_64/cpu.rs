//! Per-CPU state via `IA32_GS_BASE`.
//!
//! APs stay in kernel mode; `swapgs` / `KERNEL_GS_BASE` are unused on this
//! path. The BSP still uses the existing TSS.RSP0 + static syscall stack.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::io::{rdmsr, wrmsr};

const IA32_GS_BASE: u32 = 0xC000_0101;

pub const MAX_CPUS: usize = 4;

#[repr(C)]
pub struct PerCpu {
    pub cpu_id: u32,
    _pad: u32,
    pub ticks: AtomicU64,
}

impl PerCpu {
    const fn empty(id: u32) -> Self {
        Self {
            cpu_id: id,
            _pad: 0,
            ticks: AtomicU64::new(0),
        }
    }
}

static mut PERCPU: [PerCpu; MAX_CPUS] = [
    PerCpu::empty(0),
    PerCpu::empty(1),
    PerCpu::empty(2),
    PerCpu::empty(3),
];

static NCPUS: AtomicU32 = AtomicU32::new(1);

pub fn ncpus() -> u32 {
    NCPUS.load(Ordering::Acquire)
}

pub fn set_ncpus(n: u32) {
    NCPUS.store(n, Ordering::Release);
}

fn slot(cpu: u32) -> &'static PerCpu {
    unsafe { &PERCPU[cpu as usize] }
}

pub fn set_gs(cpu: u32) {
    debug_assert!((cpu as usize) < MAX_CPUS);
    let ptr = slot(cpu) as *const PerCpu as u64;
    wrmsr(IA32_GS_BASE, ptr);
}

/// CPU id from `gs:0`. Falls back to 0 if `GS_BASE` is still unset.
pub fn id() -> u32 {
    let base = rdmsr(IA32_GS_BASE);
    if base == 0 {
        return 0;
    }
    unsafe { core::ptr::read_volatile(base as *const u32) }
}

#[allow(dead_code)]
pub fn local_ticks() -> u64 {
    let cpu = id();
    if (cpu as usize) >= MAX_CPUS {
        return 0;
    }
    slot(cpu).ticks.load(Ordering::Relaxed)
}

pub fn local_ticks_of(cpu: u32) -> u64 {
    if (cpu as usize) >= MAX_CPUS {
        return 0;
    }
    slot(cpu).ticks.load(Ordering::Relaxed)
}

pub fn inc_local_ticks() -> u64 {
    let cpu = id();
    if (cpu as usize) >= MAX_CPUS {
        return 0;
    }
    slot(cpu).ticks.fetch_add(1, Ordering::Relaxed) + 1
}

pub fn init_bsp() {
    set_gs(0);
    set_ncpus(1);
}
