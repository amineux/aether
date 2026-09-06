//! Load the embedded static ELF64 `/init` into the user window.

use aether_core::elf::{loads_in_window, parse_elf64};
use aether_core::{USER_IMAGE_BASE, USER_IMAGE_END};

use crate::console::{self, write_hex, write_str, write_u64};
use crate::mm::{frame, paging};

pub static INIT_ELF: &[u8] = include_bytes!(env!("AETHER_INIT_ELF"));

pub fn load() -> Result<u64, &'static str> {
    if INIT_ELF.is_empty() {
        return Err("init ELF missing (build user/init first)");
    }
    let image = parse_elf64(INIT_ELF).map_err(|_| "ELF parse failed")?;
    if !loads_in_window(&image, USER_IMAGE_BASE, USER_IMAGE_END) {
        return Err("ELF loads outside 0x2000000-0x2200000");
    }
    frame::reserve_range(USER_IMAGE_BASE, USER_IMAGE_END);
    paging::allow_user_walk_low();
    paging::allow_user_2m(USER_IMAGE_BASE);

    // Zero the window so BSS is clean, then copy each PT_LOAD.
    unsafe {
        core::ptr::write_bytes(USER_IMAGE_BASE as *mut u8, 0, (USER_IMAGE_END - USER_IMAGE_BASE) as usize);
    }
    for seg in image.loads() {
        let dst = seg.vaddr as *mut u8;
        let src_off = seg.offset as usize;
        let n = seg.filesz as usize;
        if n > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(INIT_ELF.as_ptr().add(src_off), dst, n);
            }
        }
    }

    write_str("[boot] loaded /init ELF entry=");
    write_hex(image.entry);
    write_str(" loads=");
    write_u64(image.n_loads as u64);
    write_str(" bytes=");
    write_u64(INIT_ELF.len() as u64);
    write_str(" (static non-PIE, embedded blob)");
    console::nl();
    Ok(image.entry)
}
