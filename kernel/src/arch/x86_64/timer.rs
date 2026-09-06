//! PIT channel 0 @ 100 Hz. Used as the fabric scheduler's time base.

use super::io::outb;
use crate::println;

const PIT_CMD: u16 = 0x43;
const PIT_CH0: u16 = 0x40;
const PIT_HZ: u32 = 1_193_182;

pub fn init() {
    set_hz(100);
    println!("[boot] PIT 100 Hz (IRQ0)");
}

pub fn set_hz(hz: u32) {
    let hz = hz.max(18).min(1000);
    let div = PIT_HZ / hz;
    outb(PIT_CMD, 0x36);
    outb(PIT_CH0, (div & 0xFF) as u8);
    outb(PIT_CH0, ((div >> 8) & 0xFF) as u8);
}
