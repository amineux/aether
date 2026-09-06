//! Kernel GDT with ring-3 code/data and a 64-bit TSS (RSP0).
//!
//! STAR.SYSRET uses: user SS = STAR[63:48]+8, user CS = STAR[63:48]+16.
//! Layout: 0x08 kernel CS, 0x10 kernel DS, 0x18 user DS, 0x20 user CS.

use core::arch::global_asm;
use core::mem::size_of;

pub const KCODE: u16 = 0x08;
pub const KDATA: u16 = 0x10;
pub const UDATA: u16 = 0x18;
pub const UCODE: u16 = 0x20;
pub const TSS_SEL: u16 = 0x28;

pub const USER_CS: u16 = UCODE | 3;
pub const USER_DS: u16 = UDATA | 3;

const GDT_LEN: usize = 8;

#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

#[repr(C, packed)]
struct Gdtr {
    limit: u16,
    base: u64,
}

static mut GDT: [u64; GDT_LEN] = [0; GDT_LEN];
static mut TSS: Tss = Tss {
    reserved0: 0,
    rsp0: 0,
    rsp1: 0,
    rsp2: 0,
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: size_of::<Tss>() as u16,
};

static mut GDTR: Gdtr = Gdtr { limit: 0, base: 0 };

fn gdt_code(dpl: u64) -> u64 {
    let access = 0x9A | (dpl << 5);
    0x00AF_0000_0000_FFFF | (access << 40)
}

fn gdt_data(dpl: u64) -> u64 {
    let access = 0x92 | (dpl << 5);
    0x00AF_0000_0000_FFFF | (access << 40)
}

fn gdt_tss(base: u64, limit: u64) -> (u64, u64) {
    let access = 0x89u64;
    let lo = (limit & 0xFFFF)
        | ((base & 0xFFFF) << 16)
        | ((base >> 16) & 0xFF) << 32
        | (access << 40)
        | ((limit >> 16) & 0xF) << 48
        | ((base >> 24) & 0xFF) << 56;
    let hi = base >> 32;
    (lo, hi)
}

extern "C" {
    fn gdt_load(gdtr: *const Gdtr);
}

pub fn init() {
    unsafe {
        GDT[0] = 0;
        GDT[1] = gdt_code(0);
        GDT[2] = gdt_data(0);
        GDT[3] = gdt_data(3);
        GDT[4] = gdt_code(3);
        let tss_base = core::ptr::addr_of!(TSS) as u64;
        let (lo, hi) = gdt_tss(tss_base, (size_of::<Tss>() - 1) as u64);
        GDT[5] = lo;
        GDT[6] = hi;

        GDTR = Gdtr {
            limit: (size_of::<[u64; GDT_LEN]>() - 1) as u16,
            base: GDT.as_ptr() as u64,
        };
        gdt_load(core::ptr::addr_of!(GDTR));
        core::arch::asm!("ltr {0:x}", in(reg) TSS_SEL, options(nomem, nostack, preserves_flags));
    }
    crate::println!("[boot] GDT reloaded (user CS/DS DPL=3) + TSS");
}

pub fn set_rsp0(rsp0: u64) {
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!(TSS.rsp0), rsp0);
    }
}

pub fn rsp0() -> u64 {
    unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(TSS.rsp0)) }
}

global_asm!(
    r#"
    .global gdt_load
    gdt_load:
        lgdt [rdi]
        push 0x08
        lea rax, [rip + 1f]
        push rax
        retfq
    1:
        mov ax, 0x10
        mov ds, ax
        mov es, ax
        mov ss, ax
        mov fs, ax
        mov gs, ax
        ret
    "#
);
