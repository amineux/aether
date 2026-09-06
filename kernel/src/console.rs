//! Kernel console. String + integer only — no `core::fmt::write` (it
//! triple-faulted on this target before IDT install; keep I/O obvious).

use crate::arch::serial;

pub fn write_str(s: &str) {
    serial::write_str(s);
}

pub fn write_u64(mut n: u64) {
    if n == 0 {
        serial::write_byte(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 20;
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    for &b in &buf[i..] {
        serial::write_byte(b);
    }
}

pub fn write_hex(n: u64) {
    serial::write_str("0x");
    let mut started = false;
    for shift in (0..16).rev() {
        let nib = ((n >> (shift * 4)) & 0xF) as u8;
        if nib != 0 || started || shift == 0 {
            started = true;
            serial::write_byte(if nib < 10 { b'0' + nib } else { b'a' + nib - 10 });
        }
    }
}

pub fn write_i32(n: i32) {
    if n < 0 {
        serial::write_byte(b'-');
        write_u64((-n as i64) as u64);
    } else {
        write_u64(n as u64);
    }
}

pub fn nl() {
    serial::write_str("\r\n");
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::console::nl()
    };
    ($s:literal) => {{
        $crate::console::write_str($s);
        $crate::console::nl();
    }};
}
