//! Load static ELF64 images from the boot ramfs into per-task user windows.
//!
//! The ramfs is seeded from virtio-blk (x86, AETHFS01 image) when a
//! drive is present, otherwise from the embedded blobs (`include_bytes!`
//! of `build/init.elf` / `probe.elf`). The loader [`RamFs::open`]s
//! `/init` (and `/probe`) rather than touching those blobs at the load
//! site.

use aether_core::elf::{loads_in_window, parse_elf64};
#[cfg(target_arch = "x86_64")]
use aether_core::ramfs::PROBE_PATH;
use aether_core::ramfs::{RamFs, INIT_PATH};
#[cfg(target_arch = "x86_64")]
use aether_core::{USER_IMAGE_BASE, USER_IMAGE_END, USER_PROBE_BASE, USER_PROBE_END};
#[cfg(target_arch = "riscv64")]
use aether_core::{USER_RV_IMAGE_BASE, USER_RV_IMAGE_END};
#[cfg(target_arch = "aarch64")]
use aether_core::{USER_AA_IMAGE_BASE, USER_AA_IMAGE_END};

use crate::console::{self, write_hex, write_str, write_u64};
use crate::mm::{frame, paging};

#[cfg(target_arch = "x86_64")]
static INIT_ELF: &[u8] = include_bytes!(env!("AETHER_INIT_ELF"));
#[cfg(target_arch = "riscv64")]
static INIT_ELF: &[u8] = include_bytes!(env!("AETHER_INIT_ELF_RISCV"));
#[cfg(target_arch = "aarch64")]
static INIT_ELF: &[u8] = include_bytes!(env!("AETHER_INIT_ELF_AARCH64"));
#[cfg(target_arch = "x86_64")]
static PROBE_ELF: &[u8] = include_bytes!(env!("AETHER_PROBE_ELF"));

pub struct LoadedImage {
    pub entry: u64,
    pub cr3: u64,
}

/// Seed `/init` (and x86 `/probe`) from virtio-blk when present, else
/// the embedded blobs, then prove `open` + `read` on those names.
pub fn mount_boot_ramfs() -> Result<RamFs<'static>, &'static str> {
    let mut fs = RamFs::new();
    #[cfg(target_arch = "x86_64")]
    let from_blk = crate::virtio_blk::try_seed_ramfs(&mut fs);
    #[cfg(not(target_arch = "x86_64"))]
    let from_blk = false;

    if !from_blk {
        if INIT_ELF.is_empty() {
            return Err("user ELF missing (build user/ first)");
        }
        write_str("[ramfs] seed embedded");
        #[cfg(target_arch = "x86_64")]
        write_str(" (no virtio-blk)");
        console::nl();
        fs.seed(INIT_PATH, INIT_ELF)
            .map_err(|_| "ramfs seed /init failed")?;
        #[cfg(target_arch = "x86_64")]
        if !PROBE_ELF.is_empty() {
            fs.seed(PROBE_PATH, PROBE_ELF)
                .map_err(|_| "ramfs seed /probe failed")?;
        }
    }
    prove_open(&fs, INIT_PATH)?;
    #[cfg(target_arch = "x86_64")]
    if fs.contains(PROBE_PATH) {
        prove_open(&fs, PROBE_PATH)?;
    }
    Ok(fs)
}

fn prove_open(fs: &RamFs, path: &str) -> Result<(), &'static str> {
    let mut h = fs.open(path).map_err(|_| "ramfs open failed")?;
    let n = fs.size(h.fd()).map_err(|_| "ramfs stat failed")?;
    let mut mag = [0u8; 4];
    let got = fs.read(&mut h, &mut mag).map_err(|_| "ramfs read failed")?;
    if got < 4 || mag != *b"\x7fELF" {
        return Err("ramfs file is not ELF");
    }
    write_str("[ramfs] open ");
    write_str(path);
    write_str(" ok bytes=");
    write_u64(n as u64);
    console::nl();
    Ok(())
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
        core::ptr::write_bytes(paging::phys_va(base) as *mut u8, 0, (end - base) as usize);
    }
    for seg in image.loads() {
        let dst = paging::phys_va(seg.vaddr) as *mut u8;
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
    write_str(" (static non-PIE, ramfs)");
    console::nl();
    Ok(image.entry)
}

pub fn load_init(fs: &RamFs) -> Result<LoadedImage, &'static str> {
    #[cfg(target_arch = "x86_64")]
    let (base, end, unmap) = (USER_IMAGE_BASE, USER_IMAGE_END, [USER_PROBE_BASE]);
    #[cfg(target_arch = "riscv64")]
    let (base, end, unmap) = (
        USER_RV_IMAGE_BASE,
        USER_RV_IMAGE_END,
        [aether_core::USER_RV_MMAP_BASE],
    );
    #[cfg(target_arch = "aarch64")]
    let (base, end, unmap) = (
        USER_AA_IMAGE_BASE,
        USER_AA_IMAGE_END,
        [aether_core::USER_AA_MMAP_BASE],
    );
    let elf = fs.bytes(INIT_PATH).map_err(|_| "ramfs /init missing")?;
    let entry = load_into(elf, base, end, INIT_PATH)?;
    let cr3 =
        paging::clone_user_aspace(base, end, &unmap).ok_or("aspace clone failed for /init")?;
    Ok(LoadedImage { entry, cr3 })
}

#[cfg(target_arch = "x86_64")]
pub fn load_probe(fs: &RamFs) -> Result<LoadedImage, &'static str> {
    let elf = fs.bytes(PROBE_PATH).map_err(|_| "ramfs /probe missing")?;
    let entry = load_into(elf, USER_PROBE_BASE, USER_PROBE_END, PROBE_PATH)?;
    let cr3 = paging::clone_user_aspace(USER_PROBE_BASE, USER_PROBE_END, &[USER_IMAGE_BASE])
        .ok_or("PML4 clone failed for /probe")?;
    Ok(LoadedImage { entry, cr3 })
}
