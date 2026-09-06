//! Load embedded static ELF64 images into per-task user windows.

use aether_core::elf::{loads_in_window, parse_elf64};
#[cfg(target_arch = "x86_64")]
use aether_core::{USER_IMAGE_BASE, USER_IMAGE_END, USER_PROBE_BASE, USER_PROBE_END};
#[cfg(target_arch = "riscv64")]
use aether_core::{USER_RV_IMAGE_BASE, USER_RV_IMAGE_END};

use crate::console::{self, write_hex, write_str, write_u64};
use crate::mm::{frame, paging};

#[cfg(target_arch = "x86_64")]
pub static INIT_ELF: &[u8] = include_bytes!(env!("AETHER_INIT_ELF"));
#[cfg(target_arch = "riscv64")]
pub static INIT_ELF: &[u8] = include_bytes!(env!("AETHER_INIT_ELF_RISCV"));
#[cfg(target_arch = "x86_64")]
pub static PROBE_ELF: &[u8] = include_bytes!(env!("AETHER_PROBE_ELF"));

pub struct LoadedImage {
    pub entry: u64,
    pub cr3: u64,
}

fn load_into(elf: &[u8], base: u64, end: u64, name: &str) -> Result<u64, &'static str> {
    if elf.is_empty() {
        return Err("user ELF missing (build user/ first)");
    }
    let image = parse_elf64(elf).map_err(|_| "ELF parse failed")?;
    if !loads_in_window(&image, base, end) {
        return Err("ELF loads outside its 2 MiB window");
    }
    frame::reserve_range(base, end);
    unsafe {
        core::ptr::write_bytes(base as *mut u8, 0, (end - base) as usize);
    }
    for seg in image.loads() {
        let dst = seg.vaddr as *mut u8;
        let src_off = seg.offset as usize;
        let n = seg.filesz as usize;
        if n > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(elf.as_ptr().add(src_off), dst, n);
            }
        }
    }
    write_str("[boot] loaded ");
    write_str(name);
    write_str(" ELF entry=");
    write_hex(image.entry);
    write_str(" loads=");
    write_u64(image.n_loads as u64);
    write_str(" bytes=");
    write_u64(elf.len() as u64);
    write_str(" (static non-PIE, embedded blob)");
    console::nl();
    Ok(image.entry)
}

pub fn load_init() -> Result<LoadedImage, &'static str> {
    #[cfg(target_arch = "x86_64")]
    let (base, end, unmap) = (USER_IMAGE_BASE, USER_IMAGE_END, [USER_PROBE_BASE]);
    #[cfg(target_arch = "riscv64")]
    let (base, end, unmap) = (USER_RV_IMAGE_BASE, USER_RV_IMAGE_END, [0u64; 0]);
    let entry = load_into(INIT_ELF, base, end, "/init")?;
    let cr3 = paging::clone_user_aspace(base, end, &unmap).ok_or("aspace clone failed for /init")?;
    Ok(LoadedImage { entry, cr3 })
}

#[cfg(target_arch = "x86_64")]
pub fn load_probe() -> Result<LoadedImage, &'static str> {
    let entry = load_into(PROBE_ELF, USER_PROBE_BASE, USER_PROBE_END, "/probe")?;
    let cr3 = paging::clone_user_aspace(USER_PROBE_BASE, USER_PROBE_END, &[USER_IMAGE_BASE])
        .ok_or("PML4 clone failed for /probe")?;
    Ok(LoadedImage { entry, cr3 })
}
