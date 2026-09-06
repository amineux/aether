//! SiFive PLIC on QEMU virt + a software SoftNPU doorbell.
//!
//! SoftNPU stays the in-kernel [`aether_drivers::mmio::AccelMmio`] BAR
//! (path B). A real virtio-mmio `-device` is still open. Completions
//! still need a **real interrupt path** on this HAL, so the doorbell
//! raises UART0 THRE → PLIC source 10 (QEMU virt). The trap claims
//! that source and `World` services the same AccelMmio used ring.
//!
//! Supervisor software interrupt (SSIP) is a second real trap if the
//! PLIC line is late. Extra harts stay parked (context 1 = hart 0 S).

use super::UART0;
use crate::println;

/// QEMU virt SiFive PLIC.
pub const PLIC_BASE: usize = 0x0c00_0000;
/// UART0 on QEMU virt (PLIC source 10). SoftNPU doorbell, not console RX.
pub const SOFTNPU_IRQ: u32 = 10;
/// Hart 0 S-mode context (M-mode is 0; OpenSBI keeps M).
const CONTEXT_S: usize = 1;

const PRIORITY_BASE: usize = 0x0000;
const ENABLE_BASE: usize = 0x2000;
const ENABLE_STRIDE: usize = 0x80;
const CONTEXT_BASE: usize = 0x20_0000;
const CONTEXT_STRIDE: usize = 0x1000;
const THRESHOLD: usize = 0x00;
const CLAIM: usize = 0x04;

const UART_IER: usize = 1;
const UART_IIR: usize = 2;
const UART_IER_THREI: u8 = 0x02;

const SIE_SSIE: u64 = 1 << 1;
const SIE_SEIE: u64 = 1 << 9;
const SIP_SSIP: u64 = 1 << 1;

#[inline]
fn mmio_write(addr: usize, val: u32) {
    unsafe {
        core::ptr::write_volatile(addr as *mut u32, val);
    }
}

#[inline]
fn mmio_read(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn priority_addr(irq: u32) -> usize {
    PLIC_BASE + PRIORITY_BASE + (irq as usize) * 4
}

fn enable_addr(context: usize, irq: u32) -> usize {
    PLIC_BASE + ENABLE_BASE + context * ENABLE_STRIDE + (irq as usize / 32) * 4
}

fn context_addr(context: usize, off: usize) -> usize {
    PLIC_BASE + CONTEXT_BASE + context * CONTEXT_STRIDE + off
}

fn uart_write8(off: usize, val: u8) {
    unsafe {
        core::ptr::write_volatile((UART0 + off) as *mut u8, val);
    }
}

fn uart_read8(off: usize) -> u8 {
    unsafe { core::ptr::read_volatile((UART0 + off) as *const u8) }
}

fn enable_irq(irq: u32) {
    mmio_write(priority_addr(irq), 1);
    let en = enable_addr(CONTEXT_S, irq);
    let bit = 1u32 << (irq % 32);
    mmio_write(en, mmio_read(en) | bit);
}

fn enable_seie() {
    unsafe {
        core::arch::asm!("csrs sie, {0}", in(reg) SIE_SEIE | SIE_SSIE, options(nostack));
    }
}

pub fn init() {
    mmio_write(context_addr(CONTEXT_S, THRESHOLD), 0);
    enable_irq(SOFTNPU_IRQ);
    enable_seie();
    println!("[boot] PLIC hart0 S-mode; SoftNPU doorbell = UART THRE IRQ 10 (path B BAR)");
}

pub fn claim() -> u32 {
    mmio_read(context_addr(CONTEXT_S, CLAIM))
}

pub fn complete(irq: u32) {
    if irq != 0 {
        mmio_write(context_addr(CONTEXT_S, CLAIM), irq);
    }
}

/// Kick a PLIC-visible line (UART THRE) plus SSIP. Call after AccelMmio doorbell.
pub fn raise_softnpu_doorbell() {
    uart_write8(UART_IER, UART_IER_THREI);
    unsafe {
        core::arch::asm!("csrs sip, {0}", in(reg) SIP_SSIP, options(nostack));
    }
}

/// Drop the UART THRE line so PLIC source 10 goes idle.
pub fn ack_softnpu_doorbell() {
    uart_write8(UART_IER, 0);
    let _ = uart_read8(UART_IIR);
}

pub fn clear_ssip() {
    unsafe {
        core::arch::asm!("csrc sip, {0}", in(reg) SIP_SSIP, options(nostack));
    }
}
