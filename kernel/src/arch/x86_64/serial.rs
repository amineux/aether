//! 16550 UART on COM1 (0x3F8).

use super::io::{inb, outb};

const COM1: u16 = 0x3F8;

pub fn init() {
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x80);
    outb(COM1 + 0, 0x01); // 115200
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x03);
    outb(COM1 + 2, 0xC7);
    outb(COM1 + 4, 0x0B);
}

fn tx_ready() -> bool {
    inb(COM1 + 5) & 0x20 != 0
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
    outb(COM1, b);
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}
