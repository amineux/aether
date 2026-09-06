//! PL011 UART at 0x09000000 (QEMU virt).

use super::UART0;

const DR: usize = 0x00;
const FR: usize = 0x18;
const IBRD: usize = 0x24;
const FBRD: usize = 0x28;
const LCRH: usize = 0x2c;
const CR: usize = 0x30;
const IMSC: usize = 0x38;
const ICR: usize = 0x44;

const FR_TXFF: u32 = 1 << 5;

#[inline]
fn write_reg(off: usize, val: u32) {
    unsafe {
        core::ptr::write_volatile((UART0 + off) as *mut u32, val);
    }
}

#[inline]
fn read_reg(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((UART0 + off) as *const u32) }
}

pub fn init() {
    write_reg(CR, 0);
    write_reg(IMSC, 0);
    write_reg(ICR, 0x7FF);
    write_reg(IBRD, 13);
    write_reg(FBRD, 1);
    write_reg(LCRH, 0x70);
    write_reg(CR, 0x301);
}

fn tx_ready() -> bool {
    read_reg(FR) & FR_TXFF == 0
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
    write_reg(DR, b as u32);
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}
