//! SoftNPU used-ring doorbell via GICv2 SPI (path B).
//!
//! SoftNPU stays the in-kernel [`aether_drivers::mmio::AccelMmio`] BAR.
//! Completions need a **real interrupt path** on this HAL. x86 uses a
//! LAPIC self-IPI; RISC-V raises UART THRE → PLIC. On aarch64 the
//! doorbell is a **software-pended GICv2 SPI** (intid 40): write
//! `GICD_ISPENDR`, claim via `GICC_IAR`, retire the same used ring.
//!
//! SPI 40 sits between platform devices (~33–37) and virtio-mmio
//! transports (SPI 16+ / intid 48+); nothing on stock QEMU virt
//! `gic-version=2` drives it. Still path B: not virtio-mmio, not
//! GICv3, not a gated `-device` port.

use super::GICD;
use crate::println;

/// SoftNPU completion SPI (GICv2 intid). Software-pended only.
pub const SOFTNPU_IRQ: u32 = 40;

const GICD_ISENABLER: usize = 0x100;
const GICD_ISPENDR: usize = 0x200;
const GICD_ICPENDR: usize = 0x280;
const GICD_IPRIORITY: usize = 0x400;
const GICD_ITARGETS: usize = 0x800;
const GICD_ICFGR: usize = 0xc00;

#[inline]
fn gicd_write(off: usize, val: u32) {
    unsafe {
        core::ptr::write_volatile((GICD + off) as *mut u32, val);
    }
}

#[inline]
fn gicd_read(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((GICD + off) as *const u32) }
}

#[inline]
fn gicd_write8(off: usize, val: u8) {
    unsafe {
        core::ptr::write_volatile((GICD + off) as *mut u8, val);
    }
}

fn irq_word(irq: u32) -> (usize, u32) {
    ((irq / 32) as usize, 1u32 << (irq % 32))
}

fn enable_spi(irq: u32) {
    // Edge-triggered so a software pend clears on claim (ICPENDR still
    // written in ack as a belt-and-braces for level configs).
    let cfg_idx = (irq / 16) as usize;
    let shift = (irq % 16) * 2;
    let cfg_off = GICD_ICFGR + cfg_idx * 4;
    let mut cfg = gicd_read(cfg_off);
    cfg = (cfg & !(0b11 << shift)) | (0b10 << shift);
    gicd_write(cfg_off, cfg);

    gicd_write8(GICD_IPRIORITY + irq as usize, 0x80);
    gicd_write8(GICD_ITARGETS + irq as usize, 0x01);

    let (word, bit) = irq_word(irq);
    let en_off = GICD_ISENABLER + word * 4;
    gicd_write(en_off, gicd_read(en_off) | bit);
}

pub fn init() {
    enable_spi(SOFTNPU_IRQ);
    println!("[boot] GIC SoftNPU doorbell = SPI 40 (path B BAR, GICv2 ISPENDR)");
}

/// Kick SoftNPU completion. Call after AccelMmio doorbell (GIC must be on).
pub fn raise_softnpu_doorbell() {
    let (word, bit) = irq_word(SOFTNPU_IRQ);
    gicd_write(GICD_ISPENDR + word * 4, bit);
}

/// Drop the software pend so SPI 40 goes idle after claim.
pub fn ack_softnpu_doorbell() {
    let (word, bit) = irq_word(SOFTNPU_IRQ);
    gicd_write(GICD_ICPENDR + word * 4, bit);
}
