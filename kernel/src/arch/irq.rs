//! Interrupt flag helpers, the global PIT tick counter, and SMP entry.

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

#[cfg(target_arch = "aarch64")]
pub fn save_disable() -> bool {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {daif}, daif", daif = out(reg) daif, options(nomem, nostack));
        core::arch::asm!("msr daifset, #2", options(nostack));
    }
    daif & (1 << 7) == 0
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

#[cfg(target_arch = "aarch64")]
pub fn restore(were_enabled: bool) {
    if were_enabled {
        unsafe {
            core::arch::asm!("msr daifclr, #2", options(nostack));
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

#[cfg(target_arch = "aarch64")]
pub fn enable() {
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nostack));
    }
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn inc_ticks() -> u64 {
    let n = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    #[cfg(target_arch = "x86_64")]
    crate::arch::x86_64::cpu::inc_local_ticks();
    n
}

/// Bring up APIC ID 1 via INIT-SIPI when QEMU `-smp 2` (or more) is present.
/// Times out and stays UP otherwise. RISC-V / aarch64 extra PEs stay parked.
#[allow(dead_code)]
pub fn smp_start_aps() {
    #[cfg(target_arch = "x86_64")]
    crate::arch::x86_64::smp::start_aps();
}

#[allow(dead_code)]
pub fn ncpus() -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::cpu::ncpus()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        1
    }
}

#[allow(dead_code)]
pub fn cpu_id() -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::cpu::id()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}
