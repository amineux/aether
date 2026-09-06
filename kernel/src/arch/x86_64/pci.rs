//! PCI config via CF8/CFC. Enough to find a legacy virtio-blk I/O BAR.
//!
//! Not a general PCI stack. SoftNPU path B stays the in-kernel
//! AccelMmio array — this only talks config-space I/O ports.

use super::io::{inl, outl};

const CONFIG_ADDR: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

pub const VENDOR_VIRTIO: u16 = 0x1AF4;
/// Legacy / transitional virtio-blk.
pub const DEVICE_VIRTIO_BLK: u16 = 0x1001;

#[derive(Clone, Copy, Debug)]
pub struct PciFn {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
}

impl PciFn {
    pub fn read32(self, offset: u8) -> u32 {
        read32(self.bus, self.slot, self.func, offset)
    }

    pub fn write32(self, offset: u8, val: u32) {
        write32(self.bus, self.slot, self.func, offset, val)
    }

    pub fn vendor_device(self) -> (u16, u16) {
        let id = self.read32(0);
        (id as u16, (id >> 16) as u16)
    }

    /// BAR0 if it is an I/O BAR. Memory BARs (modern virtio-pci) return None.
    pub fn io_bar0(self) -> Option<u16> {
        let bar = self.read32(0x10);
        if bar & 1 == 0 {
            return None;
        }
        let base = (bar & 0xFFFC) as u16;
        if base == 0 {
            return None;
        }
        Some(base)
    }

    /// Enable I/O space + bus-master so virtio can DMA into the identity window.
    /// Bit 10 (Interrupt Disable) stays set — we poll the used ring.
    /// High 16 bits of this dword are PCI status (RW1C) — write 0 there.
    pub fn enable_io_master(self) {
        let cmd = self.read32(0x04) as u16;
        self.write32(0x04, (cmd | 0x5 | (1 << 10)) as u32);
    }
}

fn addr(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC)
}

pub fn read32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    outl(CONFIG_ADDR, addr(bus, slot, func, offset));
    inl(CONFIG_DATA)
}

pub fn write32(bus: u8, slot: u8, func: u8, offset: u8, val: u32) {
    outl(CONFIG_ADDR, addr(bus, slot, func, offset));
    outl(CONFIG_DATA, val);
}

/// Scan buses 0..=7 for legacy virtio-blk (vendor 0x1AF4, device 0x1001).
pub fn find_virtio_blk() -> Option<PciFn> {
    for bus in 0..=7u8 {
        for slot in 0..32u8 {
            let id = read32(bus, slot, 0, 0);
            if id & 0xFFFF == 0xFFFF {
                continue;
            }
            let header = read32(bus, slot, 0, 0x0C);
            let multifn = (header >> 16) & 0x80 != 0;
            let last = if multifn { 8 } else { 1 };
            for func in 0..last {
                let f = PciFn { bus, slot, func };
                let (ven, dev) = f.vendor_device();
                if ven == VENDOR_VIRTIO && dev == DEVICE_VIRTIO_BLK {
                    return Some(f);
                }
            }
        }
    }
    None
}
