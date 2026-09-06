//! Physical frames, a tiny kernel heap, and page-table helpers.
//!
//! On x86_64 the trampoline stashes Multiboot EAX/EBX at `0x7000`.
//! We parse the mmap (Multiboot1, or Multiboot2 if a loader handed
//! us that magic) and feed type-1 regions to the bitmap allocator.
//!
//! Documented subset, not a full MM:
//! - usable RAM below 16 MiB is printed, then clipped (boot tables,
//!   AP SIPI, trampoline, kernel image);
//! - regions above the 4 GiB identity map are ignored;
//! - the bitmap caps at 128 MiB of frames.
//!
//! If the mmap is missing or empty after clipping, we use the arch
//! window and say so on the serial line. RISC-V / aarch64 have no
//! Multiboot; they take that fallback on purpose (no FDT parser).

pub mod frame;
pub mod heap;
pub mod paging;

use crate::console::{self, write_hex, write_str, write_u64};
use crate::println;
use aether_core::mmap::{plan_frames, span, MemoryMap};

#[cfg(target_arch = "x86_64")]
use aether_core::mmap::{
    parse_boot_mmap, MB1_BOOT_MAGIC, MB1_FLAG_MMAP, MB2_BOOT_MAGIC,
};

/// Trampoline mailbox: magic at +0, info PA at +4. Between boot PDs
/// (`0x1000–0x6FFF`) and the AP SIPI page (`0x8000`).
#[cfg(target_arch = "x86_64")]
const MB_MAILBOX: u64 = 0x7000;
#[cfg(target_arch = "x86_64")]
const MAX_INFO: usize = 256;
#[cfg(target_arch = "x86_64")]
const MAX_MMAP: usize = 2048;

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
enum FramePlan {
    Mmap(MemoryMap),
    Fallback { why: &'static str, lo: u64, hi: u64 },
}

pub fn init() {
    let plan = discover();
    match &plan {
        FramePlan::Mmap(map) => {
            print_mmap(map);
            let planned = plan_frames(map);
            if planned.is_empty() {
                let (lo, hi) = crate::arch::frame_window();
                println!("[mm] mmap: no usable above 16 MiB; fallback arch window");
                print_window("fallback", lo, hi);
                frame::init(lo, hi);
            } else {
                print_planned(&planned);
                frame::init_from_regions(planned.regions());
            }
        }
        FramePlan::Fallback { why, lo, hi } => {
            write_str("[mm] mmap: fallback (");
            write_str(why);
            write_str(")");
            console::nl();
            print_window("fallback", *lo, *hi);
            frame::init(*lo, *hi);
        }
    }
    heap::init();
    #[cfg(target_arch = "x86_64")]
    paging::capture_kernel_cr3();
    #[cfg(target_arch = "riscv64")]
    paging::capture_kernel_satp();
    let Some(f) = frame::alloc() else {
        println!("[boot] frame allocator empty");
        return;
    };
    write_str("[boot] frame allocator: PA ");
    write_hex(f.0);
    write_str(" nframes=");
    write_u64(frame::nframes() as u64);
    write_str(" heap ");
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

fn discover() -> FramePlan {
    let (lo, hi) = crate::arch::frame_window();
    #[cfg(not(target_arch = "x86_64"))]
    {
        FramePlan::Fallback {
            why: "no Multiboot on this HAL",
            lo,
            hi,
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        match read_multiboot() {
            Ok(map) => FramePlan::Mmap(map),
            Err(why) => FramePlan::Fallback { why, lo, hi },
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn read_multiboot() -> Result<MemoryMap, &'static str> {
    let magic = unsafe { core::ptr::read_volatile(MB_MAILBOX as *const u32) };
    let info_pa = unsafe { core::ptr::read_volatile((MB_MAILBOX + 4) as *const u32) } as u64;
    if magic != MB1_BOOT_MAGIC && magic != MB2_BOOT_MAGIC {
        return Err("no Multiboot info");
    }
    if info_pa == 0 || info_pa >= 0x1_0000_0000 {
        return Err("info pointer out of identity map");
    }
    let mut info = [0u8; MAX_INFO];
    let info_len = if magic == MB2_BOOT_MAGIC {
        let total = unsafe { core::ptr::read_unaligned(info_pa as *const u32) } as usize;
        if total < 8 || total > MAX_INFO {
            return Err("multiboot2 info too large");
        }
        copy_phys(info_pa, total, &mut info)
    } else {
        copy_phys(info_pa, 64, &mut info)
    };
    if info_len < 8 {
        return Err("info truncated");
    }
    let mut mmap_buf = [0u8; MAX_MMAP];
    let mmap_slice: &[u8] = if magic == MB1_BOOT_MAGIC && info_len >= 52 {
        let flags = u32::from_le_bytes([info[0], info[1], info[2], info[3]]);
        if flags & MB1_FLAG_MMAP != 0 {
            let mmap_len = u32::from_le_bytes([info[44], info[45], info[46], info[47]]) as usize;
            let mmap_pa = u32::from_le_bytes([info[48], info[49], info[50], info[51]]) as u64;
            if mmap_pa == 0 || mmap_len == 0 {
                return Err("mmap pointer empty");
            }
            let n = copy_phys(mmap_pa, mmap_len.min(MAX_MMAP), &mut mmap_buf);
            if n == 0 {
                return Err("mmap unreadable");
            }
            &mmap_buf[..n]
        } else {
            &[]
        }
    } else {
        &[]
    };
    parse_boot_mmap(magic, &info[..info_len], mmap_slice).map_err(|e| match e {
        aether_core::mmap::MmapError::NoMmap => "no mmap tag",
        aether_core::mmap::MmapError::Empty => "mmap had no usable RAM",
        aether_core::mmap::MmapError::Truncated => "mmap truncated",
        aether_core::mmap::MmapError::BadMagic => "bad Multiboot magic",
    })
}

#[cfg(target_arch = "x86_64")]
fn copy_phys(pa: u64, len: usize, dst: &mut [u8]) -> usize {
    if pa == 0 {
        return 0;
    }
    let n = len.min(dst.len());
    let Some(end) = pa.checked_add(n as u64) else {
        return 0;
    };
    if end > 0x1_0000_0000 {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(pa as *const u8, dst.as_mut_ptr(), n);
    }
    n
}

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn print_mmap(map: &MemoryMap) {
    write_str("[mm] mmap: ");
    write_str(map.source.name());
    write_str(" entries=");
    write_u64(map.n_entries as u64);
    write_str(" usable=");
    write_u64(map.n_usable as u64);
    console::nl();
    for r in map.regions() {
        write_str("[mm] mmap usable ");
        write_hex(r.start);
        write_str("-");
        write_hex(r.end);
        console::nl();
    }
}

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn print_planned(map: &MemoryMap) {
    write_str("[mm] frames mmap clip=16MiB cap=128MiB");
    if let Some((lo, hi)) = span(map) {
        write_str(" ");
        write_hex(lo);
        write_str("-");
        write_hex(hi);
    }
    console::nl();
}

fn print_window(tag: &str, lo: u64, hi: u64) {
    write_str("[mm] frames ");
    write_str(tag);
    write_str(" ");
    write_hex(lo);
    write_str("-");
    write_hex(hi);
    console::nl();
}
