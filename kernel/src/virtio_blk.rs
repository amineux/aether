//! Legacy virtio-blk (PCI I/O) → AETHFS01 → ramfs seed.
//!
//! Documented x86 subset — **not** a block layer, **not** virtio 1.0
//! modern MMIO, **not** RISC-V / aarch64. Polls the used ring (no IRQ).
//! SoftNPU path B is the in-kernel AccelMmio BAR; this driver uses PCI
//! I/O ports and DMA into [`BLK_WINDOW_BASE`], which does not overlap
//! the SoftNPU arena at `0x0100_0000`.
//!
//! No device → caller seeds the embedded blobs.

use aether_core::bootfs::parse_bootfs;
use aether_core::ramfs::{RamFs, INIT_PATH, PROBE_PATH};
use aether_core::{BLK_WINDOW_BASE, BLK_WINDOW_END};

use crate::arch::x86_64::io::{inb, inl, inw, outb, outl, outw};
use crate::arch::x86_64::pci::{self, PciFn};
use crate::console::{self, write_hex, write_str, write_u64};
use crate::mm::frame;

const VRING_ALIGN: usize = 4096;
const VRING_OFF: u64 = 0;
const VRING_BYTES: usize = 0x8000;
const HDR_OFF: u64 = 0x8000;
const STATUS_OFF: u64 = 0x8010;
const DATA_OFF: u64 = 0x9000;

const VIRTIO_ACK: u8 = 1;
const VIRTIO_DRIVER: u8 = 2;
const VIRTIO_DRIVER_OK: u8 = 4;
const VIRTIO_FAILED: u8 = 128;

const REG_HOST_FEATURES: u16 = 0;
const REG_GUEST_FEATURES: u16 = 4;
const REG_QUEUE_PFN: u16 = 8;
const REG_QUEUE_NUM: u16 = 12;
const REG_QUEUE_SEL: u16 = 14;
const REG_QUEUE_NOTIFY: u16 = 16;
const REG_STATUS: u16 = 18;
const REG_ISR: u16 = 19;
const REG_CONFIG: u16 = 20;

const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;
const VRING_AVAIL_F_NO_INTERRUPT: u16 = 1;
const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_S_OK: u8 = 0;

const DESC_SIZE: usize = 16;
const CHUNK_SECS: u32 = 32;
const POLL_SPINS: u32 = 1_000_000;

struct BlkDev {
    iobase: u16,
    qsize: u16,
    avail_idx: u16,
    used_seen: u16,
}

/// Probe PCI for virtio-blk and seed `/init` (and `/probe` if present).
/// Returns true only when `/init` was seeded from the drive.
pub fn try_seed_ramfs(fs: &mut RamFs<'static>) -> bool {
    let Some(pci) = pci::find_virtio_blk() else {
        return false;
    };
    write_str("[blk] virtio-blk pci ");
    write_u64(pci.bus as u64);
    write_str(":");
    write_u64(pci.slot as u64);
    write_str(".");
    write_u64(pci.func as u64);
    console::nl();
    match seed_from_pci(pci, fs) {
        Ok(()) => true,
        Err(e) => {
            write_str("[blk] virtio-blk failed: ");
            write_str(e);
            console::nl();
            false
        }
    }
}

fn seed_from_pci(pci: PciFn, fs: &mut RamFs<'static>) -> Result<(), &'static str> {
    pci.enable_io_master();
    let iobase = pci.io_bar0().ok_or("BAR0 is not I/O (need legacy virtio-pci)")?;
    write_str("[blk] virtio-blk iobase=");
    write_hex(iobase as u64);
    console::nl();

    frame::reserve_range(BLK_WINDOW_BASE, BLK_WINDOW_END);
    unsafe {
        core::ptr::write_bytes(
            BLK_WINDOW_BASE as *mut u8,
            0,
            (BLK_WINDOW_END - BLK_WINDOW_BASE) as usize,
        );
    }

    let mut dev = init_device(iobase)?;
    let cap = read_capacity(iobase);
    write_str("[blk] virtio-blk qsize=");
    write_u64(dev.qsize as u64);
    write_str(" sectors=");
    write_u64(cap);
    console::nl();
    if cap == 0 {
        return Err("capacity is 0");
    }

    let data_pa = BLK_WINDOW_BASE + DATA_OFF;
    let data_max = (BLK_WINDOW_END - data_pa) as u64;
    let want = cap.saturating_mul(512).min(data_max);
    let nsec = ((want + 511) / 512) as u32;
    read_sectors(&mut dev, 0, nsec, data_pa)?;

    let nbytes = (nsec as usize) * 512;
    let image: &'static [u8] = unsafe { core::slice::from_raw_parts(data_pa as *const u8, nbytes) };
    let boot = parse_bootfs(image).map_err(|_| "AETHFS01 parse failed")?;
    let init = boot.get(INIT_PATH).ok_or("bootfs missing /init")?;
    fs.seed(INIT_PATH, init).map_err(|_| "ramfs seed /init from blk failed")?;
    write_str("[blk] virtio-blk seed /init ok bytes=");
    write_u64(init.len() as u64);
    console::nl();
    if let Some(probe) = boot.get(PROBE_PATH) {
        fs.seed(PROBE_PATH, probe)
            .map_err(|_| "ramfs seed /probe from blk failed")?;
        write_str("[blk] virtio-blk seed /probe ok bytes=");
        write_u64(probe.len() as u64);
        console::nl();
    }
    Ok(())
}

fn init_device(iobase: u16) -> Result<BlkDev, &'static str> {
    outb(iobase + REG_STATUS, 0);
    outb(iobase + REG_STATUS, VIRTIO_ACK);
    outb(iobase + REG_STATUS, VIRTIO_ACK | VIRTIO_DRIVER);
    let _host = inl(iobase + REG_HOST_FEATURES);
    outl(iobase + REG_GUEST_FEATURES, 0);

    outw(iobase + REG_QUEUE_SEL, 0);
    let qsize = inw(iobase + REG_QUEUE_NUM);
    if qsize == 0 || qsize > 512 {
        outb(iobase + REG_STATUS, VIRTIO_FAILED);
        return Err("bad virtqueue size");
    }
    let need = vring_bytes(qsize as usize);
    if need > VRING_BYTES {
        outb(iobase + REG_STATUS, VIRTIO_FAILED);
        return Err("virtqueue does not fit window");
    }

    let vring_pa = BLK_WINDOW_BASE + VRING_OFF;
    if vring_pa & (VRING_ALIGN as u64 - 1) != 0 {
        return Err("vring not aligned");
    }
    let pfn = (vring_pa / 4096) as u32;
    outl(iobase + REG_QUEUE_PFN, pfn);
    outb(iobase + REG_STATUS, VIRTIO_ACK | VIRTIO_DRIVER | VIRTIO_DRIVER_OK);

    Ok(BlkDev {
        iobase,
        qsize,
        avail_idx: 0,
        used_seen: 0,
    })
}

fn read_capacity(iobase: u16) -> u64 {
    let lo = inl(iobase + REG_CONFIG);
    let hi = inl(iobase + REG_CONFIG + 4);
    ((hi as u64) << 32) | (lo as u64)
}

fn read_sectors(dev: &mut BlkDev, start: u32, count: u32, dest_pa: u64) -> Result<(), &'static str> {
    let mut done = 0u32;
    while done < count {
        let n = (count - done).min(CHUNK_SECS);
        read_chunk(dev, start + done, n, dest_pa + (done as u64) * 512)?;
        done += n;
    }
    Ok(())
}

fn read_chunk(dev: &mut BlkDev, sector: u32, nsec: u32, dest_pa: u64) -> Result<(), &'static str> {
    let hdr_pa = BLK_WINDOW_BASE + HDR_OFF;
    let st_pa = BLK_WINDOW_BASE + STATUS_OFF;
    unsafe {
        let hdr = hdr_pa as *mut u8;
        core::ptr::write_unaligned(hdr as *mut u32, VIRTIO_BLK_T_IN);
        core::ptr::write_unaligned(hdr.add(4) as *mut u32, 0);
        core::ptr::write_unaligned(hdr.add(8) as *mut u64, sector as u64);
        core::ptr::write_volatile(st_pa as *mut u8, 0xFF);
    }

    let q = dev.qsize as usize;
    let desc = (BLK_WINDOW_BASE + VRING_OFF) as *mut u8;
    write_desc(desc, 0, hdr_pa, 16, VRING_DESC_F_NEXT, 1);
    write_desc(
        desc,
        1,
        dest_pa,
        nsec * 512,
        VRING_DESC_F_NEXT | VRING_DESC_F_WRITE,
        2,
    );
    write_desc(desc, 2, st_pa, 1, VRING_DESC_F_WRITE, 0);

    let avail = desc.wrapping_add(q * DESC_SIZE);
    let slot = (dev.avail_idx as usize) % q;
    unsafe {
        core::ptr::write_volatile(avail as *mut u16, VRING_AVAIL_F_NO_INTERRUPT);
        core::ptr::write_volatile(avail.add(4 + slot * 2) as *mut u16, 0);
    }
    dev.avail_idx = dev.avail_idx.wrapping_add(1);
    unsafe {
        core::ptr::write_volatile(avail.add(2) as *mut u16, dev.avail_idx);
    }
    fence();
    outw(dev.iobase + REG_QUEUE_NOTIFY, 0);

    let used = used_ptr(q);
    let mut spins = 0u32;
    loop {
        let idx = unsafe { core::ptr::read_volatile(used.add(2) as *const u16) };
        if idx != dev.used_seen {
            dev.used_seen = idx;
            break;
        }
        spins += 1;
        if spins > POLL_SPINS {
            return Err("used-ring timeout");
        }
        core::hint::spin_loop();
    }
    let _ = inb(dev.iobase + REG_ISR);
    let status = unsafe { core::ptr::read_volatile(st_pa as *const u8) };
    if status != VIRTIO_BLK_S_OK {
        return Err("virtio-blk status not OK");
    }
    Ok(())
}

fn write_desc(base: *mut u8, idx: usize, addr: u64, len: u32, flags: u16, next: u16) {
    let p = base.wrapping_add(idx * DESC_SIZE);
    unsafe {
        core::ptr::write_unaligned(p as *mut u64, addr);
        core::ptr::write_unaligned(p.add(8) as *mut u32, len);
        core::ptr::write_unaligned(p.add(12) as *mut u16, flags);
        core::ptr::write_unaligned(p.add(14) as *mut u16, next);
    }
}

fn used_ptr(qsize: usize) -> *mut u8 {
    let desc = (BLK_WINDOW_BASE + VRING_OFF) as usize;
    let avail_end = desc + qsize * DESC_SIZE + 6 + qsize * 2;
    let used = (avail_end + VRING_ALIGN - 1) & !(VRING_ALIGN - 1);
    used as *mut u8
}

fn vring_bytes(qsize: usize) -> usize {
    let avail_end = qsize * DESC_SIZE + 6 + qsize * 2;
    let used = (avail_end + VRING_ALIGN - 1) & !(VRING_ALIGN - 1);
    used + 6 + qsize * 8
}

fn fence() {
    unsafe {
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
}
