//! Bitmap frame allocator. 4 KiB frames, no coalescing.
//!
//! The bitmap covers a single span (holes marked used). Usable ranges
//! come from the Multiboot mmap planner, or the arch fallback window.

use crate::sync::SpinLock;
use aether_core::mmap::{PhysRegion, FRAME_SIZE as FRAME, MAX_MANAGED_FRAMES as MAX_FRAMES};
use aether_core::types::PhysAddr;

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

fn reset_empty(a: &mut Inner) {
    a.bits = [u64::MAX; BITMAP_WORDS];
    a.base = 0;
    a.nframes = 0;
    a.used = 0;
}

/// Single contiguous window. Prefer [`init_from_regions`] when the
/// bootloader provided a real map.
pub fn init(phys_start: u64, phys_end: u64) {
    match PhysRegion::new(phys_start, phys_end) {
        Some(r) => init_from_regions(&[r]),
        None => {
            let mut a = ALLOC.lock();
            reset_empty(&mut a);
        }
    }
}

/// Mark the span used, then free each usable region that intersects it.
/// Holes (reserved / ACPI / clipped) stay allocated so they are never
/// handed out.
pub fn init_from_regions(regions: &[PhysRegion]) {
    let mut a = ALLOC.lock();
    let mut lo = u64::MAX;
    let mut hi = 0u64;
    for r in regions {
        if r.end > r.start {
            lo = lo.min(r.start);
            hi = hi.max(r.end);
        }
    }
    if lo >= hi {
        reset_empty(&mut a);
        return;
    }
    let start = (lo + FRAME - 1) & !(FRAME - 1);
    let end = hi & !(FRAME - 1);
    if end <= start {
        reset_empty(&mut a);
        return;
    }
    let n = ((end - start) / FRAME) as usize;
    let n = n.min(MAX_FRAMES);
    a.bits = [u64::MAX; BITMAP_WORDS];
    a.base = start;
    a.nframes = n;
    a.used = n;
    let span_end = start + n as u64 * FRAME;
    for r in regions {
        let rs = r.start.max(start);
        let re = r.end.min(span_end);
        let mut addr = (rs + FRAME - 1) & !(FRAME - 1);
        let re = re & !(FRAME - 1);
        while addr < re {
            let i = ((addr - start) / FRAME) as usize;
            if i < n && test(&a.bits, i) {
                clear(&mut a.bits, i);
                a.used -= 1;
            }
            addr += FRAME;
        }
    }
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

pub fn nframes() -> usize {
    ALLOC.lock().nframes
}

pub fn base() -> u64 {
    ALLOC.lock().base
}

/// Mark `[start, end)` used so the ELF image is not handed out as frames.
pub fn reserve_range(phys_start: u64, phys_end: u64) {
    let mut a = ALLOC.lock();
    if a.nframes == 0 {
        return;
    }
    let start = phys_start.max(a.base);
    let end = phys_end;
    let mut addr = start & !(FRAME - 1);
    while addr < end {
        if addr >= a.base {
            let i = ((addr - a.base) / FRAME) as usize;
            if i < a.nframes && !test(&a.bits, i) {
                set(&mut a.bits, i);
                a.used += 1;
            }
        }
        addr += FRAME;
    }
}
