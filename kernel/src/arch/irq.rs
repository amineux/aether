//! Interrupt flag helpers and the UP tick counter.

use core::sync::atomic::{AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "x86_64")]
pub fn save_disable() -> bool {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {0}", out(reg) flags);
        core::arch::asm!("cli");
    }
    flags & (1 << 9) != 0
}

#[cfg(target_arch = "riscv64")]
pub fn save_disable() -> bool {
    let prev: u64;
    unsafe {
        core::arch::asm!("csrrci {prev}, sstatus, 2", prev = out(reg) prev, options(nostack));
    }
    prev & 2 != 0
}

#[cfg(target_arch = "x86_64")]
pub fn restore(were_enabled: bool) {
    if were_enabled {
        unsafe {
            core::arch::asm!("sti");
        }
    }
}

#[cfg(target_arch = "riscv64")]
pub fn restore(were_enabled: bool) {
    if were_enabled {
        unsafe {
            core::arch::asm!("csrsi sstatus, 2", options(nostack));
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub fn enable() {
    unsafe {
        core::arch::asm!("sti");
    }
}

#[cfg(target_arch = "riscv64")]
pub fn enable() {
    unsafe {
        core::arch::asm!("csrsi sstatus, 2", options(nostack));
    }
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn inc_ticks() -> u64 {
    TICKS.fetch_add(1, Ordering::Relaxed) + 1
}

/// STUB: APIC INIT-SIPI-SIPI + per-CPU `gs` / RISC-V `tp`. UP only.
pub fn smp_start_aps() {
    // STUB: no AP bring-up
}
