//! Supervisor timer via SBI TIME (fallback: legacy set_timer).
//!
//! QEMU virt timebase is 10 MHz. We aim for 100 Hz. Wrong frequency only
//! changes the tick rate; the demo does not depend on wall time.

use crate::println;

const SBI_EXT_TIME: i64 = 0x5449_4D45;
const SBI_EXT_LEGACY: i64 = 0;
const TIMEBASE_HZ: u64 = 10_000_000;
const TICK_HZ: u64 = 100;

pub fn init() {
    set_hz(TICK_HZ as u32);
    enable_stie();
    crate::arch::irq::enable();
    println!("[boot] SBI timer 100 Hz (supervisor timer IRQ)");
}

pub fn set_hz(hz: u32) {
    let hz = hz.max(1).min(1000) as u64;
    let delta = TIMEBASE_HZ / hz;
    let now = rdtime();
    sbi_set_timer(now.saturating_add(delta));
}

pub fn rearm() {
    set_hz(TICK_HZ as u32);
}

fn rdtime() -> u64 {
    let t: u64;
    unsafe {
        core::arch::asm!("rdtime {t}", t = out(reg) t, options(nomem, nostack));
    }
    t
}

fn sbi_set_timer(stime: u64) {
    let err = sbi_ecall(SBI_EXT_TIME, 0, stime);
    if err != 0 {
        let _ = sbi_ecall(SBI_EXT_LEGACY, 0, stime);
    }
}

fn sbi_ecall(ext: i64, fid: i64, arg0: u64) -> i64 {
    let err: i64;
    unsafe {
        core::arch::asm!(
            "ecall",
            inout("a0") arg0 => err,
            inout("a1") 0u64 => _,
            in("a6") fid,
            in("a7") ext,
            options(nostack)
        );
    }
    err
}

fn enable_stie() {
    unsafe {
        // sie.STIE = bit 5
        core::arch::asm!("csrs sie, {0}", in(reg) 1u64 << 5, options(nostack));
    }
}
