//! Bitmap frame allocator. 4 KiB frames, no coalescing.

use crate::sync::SpinLock;
use aether_core::types::PhysAddr;

const FRAME: u64 = 4096;
const MAX_FRAMES: usize = 32768; // 128 MiB
const BITMAP_WORDS: usize = MAX_FRAMES / 64;

struct Inner {
    bits: [u64; BITMAP_WORDS],
    base: u64,
    nframes: usize,
    used: usize,
}

static ALLOC: SpinLock<Inner> = SpinLock::new(Inner {
    bits: [u64::MAX; BITMAP_WORDS],
    base: 0,
    nframes: 0,
    used: 0,
});

pub fn init(phys_start: u64, phys_end: u64) {
    let start = (phys_start + FRAME - 1) & !(FRAME - 1);
    let end = phys_end & !(FRAME - 1);
    let n = ((end - start) / FRAME) as usize;
    let n = n.min(MAX_FRAMES);
    let mut a = ALLOC.lock();
    a.bits = [0; BITMAP_WORDS];
    a.base = start;
    a.nframes = n;
    a.used = 0;
}

fn test(bits: &[u64; BITMAP_WORDS], i: usize) -> bool {
    bits[i / 64] & (1u64 << (i % 64)) != 0
}

fn set(bits: &mut [u64; BITMAP_WORDS], i: usize) {
    bits[i / 64] |= 1u64 << (i % 64);
}

fn clear(bits: &mut [u64; BITMAP_WORDS], i: usize) {
    bits[i / 64] &= !(1u64 << (i % 64));
}

pub fn alloc() -> Option<PhysAddr> {
    let mut a = ALLOC.lock();
    for i in 0..a.nframes {
        if !test(&a.bits, i) {
            set(&mut a.bits, i);
            a.used += 1;
            return Some(PhysAddr(a.base + i as u64 * FRAME));
        }
    }
    None
}

pub fn free(p: PhysAddr) {
    let mut a = ALLOC.lock();
    if p.0 < a.base {
        return;
    }
    let i = ((p.0 - a.base) / FRAME) as usize;
    if i < a.nframes && test(&a.bits, i) {
        clear(&mut a.bits, i);
        a.used -= 1;
    }
}

pub fn used() -> usize {
    ALLOC.lock().used
}
