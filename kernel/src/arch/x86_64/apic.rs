//! xAPIC MMIO (0xFEE00000) — INIT-SIPI and a single fixed IPI.
//!
//! Identity-mapped by the trampoline (4 GiB). x2APIC is left off so the
//! MMIO page stays valid. This is QEMU smoke, not a full APIC driver.

use super::io::{rdmsr, wrmsr};

const IA32_APIC_BASE: u32 = 0x1B;
const APIC_BASE_EN: u64 = 1 << 11;
const APIC_BASE_X2: u64 = 1 << 10;
const APIC_MMIO: u64 = 0xFEE0_0000;

const APIC_ID: u32 = 0x20;
const APIC_EOI: u32 = 0xB0;
const APIC_SIVR: u32 = 0xF0;
const APIC_ICR_LOW: u32 = 0x300;
const APIC_ICR_HIGH: u32 = 0x310;

const SIVR_ENABLE: u32 = 1 << 8;
const SPURIOUS_VEC: u32 = 0xFF;

/// Delivery-status bit in ICR low.
const ICR_PENDING: u32 = 1 << 12;
const ICR_INIT: u32 = 5 << 8;
const ICR_SIPI: u32 = 6 << 8;
const ICR_FIXED: u32 = 0 << 8;
const ICR_ASSERT: u32 = 1 << 14;
const ICR_LEVEL: u32 = 1 << 15;

pub const IPI_VECTOR: u8 = 0x30;

fn mmio(off: u32) -> *mut u32 {
    (APIC_MMIO + off as u64) as *mut u32
}

fn read(off: u32) -> u32 {
    unsafe { core::ptr::read_volatile(mmio(off)) }
}

fn write(off: u32, val: u32) {
    unsafe { core::ptr::write_volatile(mmio(off), val) }
}

pub fn enable() {
    let mut base = rdmsr(IA32_APIC_BASE);
    base |= APIC_BASE_EN;
    base &= !APIC_BASE_X2;
    wrmsr(IA32_APIC_BASE, base);
    write(APIC_SIVR, SIVR_ENABLE | SPURIOUS_VEC);
}

pub fn local_id() -> u32 {
    read(APIC_ID) >> 24
}

pub fn eoi() {
    write(APIC_EOI, 0);
}

fn icr_idle() {
    let mut spins = 0u32;
    while read(APIC_ICR_LOW) & ICR_PENDING != 0 && spins < 1_000_000 {
        spins += 1;
        core::hint::spin_loop();
    }
}

fn send_icr(apic_id: u32, low: u32) {
    icr_idle();
    write(APIC_ICR_HIGH, apic_id << 24);
    write(APIC_ICR_LOW, low);
    icr_idle();
}

fn delay_iters(n: u64) {
    let mut i = 0u64;
    while i < n {
        core::hint::spin_loop();
        i += 1;
    }
}

/// INIT + two SIPIs. `vector` is `trampoline_phys >> 12` (must be < 1 MiB).
pub fn init_sipi(apic_id: u32, trampoline_phys: u64) {
    let vector = ((trampoline_phys >> 12) & 0xFF) as u32;
    // INIT assert (level) then deassert — QEMU accepts the edge-only
    // variant too; this matches the SDM / Linux bring-up shape.
    send_icr(apic_id, ICR_INIT | ICR_ASSERT | ICR_LEVEL);
    delay_iters(4_000_000);
    send_icr(apic_id, ICR_INIT | ICR_LEVEL);
    delay_iters(1_000_000);
    send_icr(apic_id, ICR_SIPI | vector);
    delay_iters(400_000);
    send_icr(apic_id, ICR_SIPI | vector);
}

pub fn ipi(apic_id: u32, vector: u8) {
    send_icr(apic_id, ICR_FIXED | vector as u32);
}
