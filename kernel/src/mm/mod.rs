//! Physical frames, a tiny kernel heap, and page-table helpers.
//!
//! Boot already identity-maps 4 GiB. We treat RAM above 16 MiB as free
//! frames (QEMU `-m 128`). A real port parses the multiboot mmap.

pub mod frame;
pub mod heap;
pub mod paging;

use crate::console::{self, write_hex, write_str, write_u64};
use crate::println;

pub fn init() {
    let (lo, hi) = crate::arch::frame_window();
    frame::init(lo, hi);
    heap::init();
    let Some(f) = frame::alloc() else {
        println!("[boot] frame allocator empty");
        return;
    };
    write_str("[boot] frame allocator: PA ");
    write_hex(f.0);
    write_str(" (16-128 MiB window), heap ");
    write_u64((heap::HEAP_SIZE / 1024) as u64);
    write_str(" KiB");
    console::nl();
    frame::free(f);
    unsafe {
        let p = heap::alloc(32, 16);
        if !p.is_null() {
            core::ptr::write_bytes(p, 0xAE, 32);
        }
    }
    crate::console::write_str(crate::arch::identity_map_note());
    crate::console::nl();
}
