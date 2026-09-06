//! Bump heap for optional driver growth. The fabric demo is allocation-free.

use crate::sync::SpinLock;

pub const HEAP_SIZE: usize = 64 * 1024;
#[repr(align(16))]
struct HeapBuf([u8; HEAP_SIZE]);
static mut HEAP_BUF: HeapBuf = HeapBuf([0; HEAP_SIZE]);

struct Bump {
    off: usize,
}

static BUMP: SpinLock<Bump> = SpinLock::new(Bump { off: 0 });

pub fn init() {
    BUMP.lock().off = 0;
}

/// Allocate `size` bytes, 16-byte aligned. No free (bump). Returns null on OOM.
pub unsafe fn alloc(size: usize, align: usize) -> *mut u8 {
    let align = align.max(16);
    let mut b = BUMP.lock();
    let base = unsafe { HEAP_BUF.0.as_mut_ptr() as usize };
    let addr = (base + b.off + align - 1) & !(align - 1);
    let Some(end) = addr.checked_add(size) else {
        return core::ptr::null_mut();
    };
    let start = base;
    if end > start + HEAP_SIZE {
        return core::ptr::null_mut();
    }
    b.off = end - start;
    addr as *mut u8
}

pub fn remaining() -> usize {
    HEAP_SIZE.saturating_sub(BUMP.lock().off)
}
