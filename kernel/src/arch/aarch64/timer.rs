//! GICv2 + CNTV (virtual timer) at 100 Hz.
//!
//! QEMU virt `gic-version=2`. CNTFRQ_EL0 is the timebase (typically
//! 62.5 MHz). Wrong frequency only changes the tick rate.

use crate::println;

use super::{GICC, GICD};

const GICD_CTLR: usize = 0x000;
const GICD_ISENABLER0: usize = 0x100;
const GICD_IPRIORITY: usize = 0x400;
const GICD_ITARGETS: usize = 0x800;

const GICC_CTLR: usize = 0x000;
const GICC_PMR: usize = 0x004;
const GICC_IAR: usize = 0x00c;
const GICC_EOIR: usize = 0x010;

/// PPI 27: virtual timer.
pub const TIMER_IRQ: u32 = 27;
const TICK_HZ: u64 = 100;

#[inline]
fn gicd_write(off: usize, val: u32) {
    unsafe {
        core::ptr::write_volatile((GICD + off) as *mut u32, val);
    }
}

#[inline]
fn gicd_write8(off: usize, val: u8) {
    unsafe {
        core::ptr::write_volatile((GICD + off) as *mut u8, val);
    }
}

#[inline]
fn gicc_write(off: usize, val: u32) {
    unsafe {
        core::ptr::write_volatile((GICC + off) as *mut u32, val);
    }
}

#[inline]
fn gicc_read(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((GICC + off) as *const u32) }
}

fn cntfrq() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mrs {v}, cntfrq_el0", v = out(reg) v, options(nomem, nostack));
    }
    if v == 0 {
        62_500_000
    } else {
        v
    }
}

fn cntv_tval_set(ticks: u64) {
    unsafe {
        core::arch::asm!("msr cntv_tval_el0, {t}", t = in(reg) ticks, options(nostack));
    }
}

fn cntv_enable() {
    unsafe {
        // bit0 enable, bit1 IMASK=0
        core::arch::asm!("msr cntv_ctl_el0, {v}", v = in(reg) 1u64, options(nostack));
        core::arch::asm!("isb", options(nostack));
    }
}

fn gic_init() {
    gicd_write(GICD_CTLR, 1);
    gicc_write(GICC_PMR, 0xFF);
    gicc_write(GICC_CTLR, 1);
    gicd_write8(GICD_IPRIORITY + TIMER_IRQ as usize, 0x80);
    gicd_write8(GICD_ITARGETS + TIMER_IRQ as usize, 0x01);
    gicd_write(GICD_ISENABLER0, 1 << TIMER_IRQ);
}

pub fn init() {
    gic_init();
    set_hz(TICK_HZ as u32);
    cntv_enable();
    crate::arch::irq::enable();
    println!("[boot] CNTV 100 Hz (GICv2 PPI 27)");
}

pub fn set_hz(hz: u32) {
    let hz = hz.max(1).min(1000) as u64;
    let delta = cntfrq() / hz;
    cntv_tval_set(delta);
}

pub fn rearm() {
    set_hz(TICK_HZ as u32);
}

pub fn ack() -> u32 {
    gicc_read(GICC_IAR)
}

pub fn eoi(iar: u32) {
    gicc_write(GICC_EOIR, iar);
}
