//! 16550 UART0 at 0x10000000 (QEMU virt).
//!
//! Console is polled TX. IER stays 0 except when `plic` raises the
//! SoftNPU doorbell (THRE → PLIC source 10). Do not enable RX here.

use super::UART0;

const IER: usize = 1;
const FCR: usize = 2;
const LCR: usize = 3;
const MCR: usize = 4;
const LSR: usize = 5;

#[inline]
fn write_reg(off: usize, val: u8) {
    unsafe {
        core::ptr::write_volatile((UART0 + off) as *mut u8, val);
    }
}

#[inline]
fn read_reg(off: usize) -> u8 {
    unsafe { core::ptr::read_volatile((UART0 + off) as *const u8) }
}

pub fn init() {
    write_reg(IER, 0x00);
    write_reg(LCR, 0x80);
    write_reg(0, 0x01);
    write_reg(IER, 0x00);
    write_reg(LCR, 0x03);
    write_reg(FCR, 0xC7);
    write_reg(MCR, 0x0B);
}

fn tx_ready() -> bool {
    read_reg(LSR) & 0x20 != 0
}

pub fn write_byte(b: u8) {
    let c = if b == b'\n' {
        write_byte_raw(b'\r');
        b'\n'
    } else {
        b
    };
    write_byte_raw(c);
}

fn write_byte_raw(b: u8) {
    let mut spins = 0u32;
    while !tx_ready() && spins < 100_000 {
        spins += 1;
        core::hint::spin_loop();
    }
    write_reg(0, b);
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}
