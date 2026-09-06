//! Interrupt flag helpers (portable names; x86_64 implementation).

use core::sync::atomic::{AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn save_disable() -> bool {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {0}", out(reg) flags);
        core::arch::asm!("cli");
    }
    flags & (1 << 9) != 0
}

pub fn restore(were_enabled: bool) {
    if were_enabled {
        unsafe {
            core::arch::asm!("sti");
        }
    }
}

pub fn enable() {
    unsafe {
        core::arch::asm!("sti");
    }
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn inc_ticks() -> u64 {
    TICKS.fetch_add(1, Ordering::Relaxed) + 1
}

/// STUB: APIC INIT-SIPI-SIPI + per-CPU `gs` base. UP only in v0.1.
pub fn smp_start_aps() {
    // STUB: no AP bring-up
}
