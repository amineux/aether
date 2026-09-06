//! IDT + PIC remap. Handlers are `extern "C"` + `global_asm` (stable rustc).

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use super::io::{inb, io_wait, outb};
use crate::arch::irq;

const PIC1: u16 = 0x20;
const PIC2: u16 = 0xA0;
const PIC1_DATA: u16 = 0x21;
const PIC2_DATA: u16 = 0xA1;

static FAULTS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct IdtEntry {
    off_lo: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    off_mid: u16,
    off_hi: u32,
    zero: u32,
}

#[repr(C, packed)]
struct IdtPtr {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry {
    off_lo: 0,
    selector: 0,
    ist: 0,
    flags: 0,
    off_mid: 0,
    off_hi: 0,
    zero: 0,
}; 256];

fn set_gate(vec: usize, handler: unsafe extern "C" fn(), trap: bool) {
    let addr = handler as u64;
    let flags = if trap { 0x8F } else { 0x8E };
    unsafe {
        IDT[vec] = IdtEntry {
            off_lo: addr as u16,
            selector: 0x08,
            ist: 0,
            flags,
            off_mid: (addr >> 16) as u16,
            off_hi: (addr >> 32) as u32,
            zero: 0,
        };
    }
}

extern "C" {
    fn isr_stub_0();
    fn isr_stub_8();
    fn isr_stub_13();
    fn isr_stub_14();
    fn isr_stub_32();
    fn isr_stub_33();
    fn isr_stub_generic();
}

pub fn init() {
    for i in 0..256 {
        set_gate(i, isr_stub_generic, false);
    }
    set_gate(0, isr_stub_0, true);
    set_gate(8, isr_stub_8, true);
    set_gate(13, isr_stub_13, true);
    set_gate(14, isr_stub_14, true);
    set_gate(32, isr_stub_32, false);
    set_gate(33, isr_stub_33, false);

    remap_pic();

    unsafe {
        let ptr = IdtPtr {
            limit: (core::mem::size_of_val(&IDT) - 1) as u16,
            base: IDT.as_ptr() as u64,
        };
        core::arch::asm!("lidt [{0}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
    }
    irq::enable();
    crate::println!("[boot] IDT loaded, PIC remapped (IRQ0-15 -> 32-47)");
}

fn remap_pic() {
    let m1 = inb(PIC1_DATA);
    let m2 = inb(PIC2_DATA);
    let _ = (m1, m2);
    outb(PIC1, 0x11);
    io_wait();
    outb(PIC2, 0x11);
    io_wait();
    outb(PIC1_DATA, 32);
    io_wait();
    outb(PIC2_DATA, 40);
    io_wait();
    outb(PIC1_DATA, 4);
    io_wait();
    outb(PIC2_DATA, 2);
    io_wait();
    outb(PIC1_DATA, 0x01);
    io_wait();
    outb(PIC2_DATA, 0x01);
    io_wait();
    // Unmask IRQ0 (timer) only.
    outb(PIC1_DATA, 0xFE);
    outb(PIC2_DATA, 0xFF);
}

fn eoi(irq: u8) {
    if irq >= 8 {
        outb(PIC2, 0x20);
    }
    outb(PIC1, 0x20);
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub vec: u64,
    pub err: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[no_mangle]
pub extern "C" fn isr_dispatch(frame: &mut InterruptFrame) {
    match frame.vec {
        32 => {
            irq::inc_ticks();
            eoi(0);
            crate::task::on_timer(frame);
        }
        33 => {
            let _sc = inb(0x60);
            eoi(1);
        }
        0..=31 => {
            FAULTS.fetch_add(1, Ordering::Relaxed);
            crate::console::write_str("[fault] vec=");
            crate::console::write_u64(frame.vec);
            crate::console::write_str(" err=");
            crate::console::write_hex(frame.err);
            crate::console::write_str(" rip=");
            crate::console::write_hex(frame.rip);
            crate::console::write_str(" cr2=");
            crate::console::write_hex(read_cr2());
            crate::console::nl();
            if frame.vec == 8 {
                loop {
                    unsafe {
                        core::arch::asm!("hlt");
                    }
                }
            }
        }
        _ => eoi((frame.vec.saturating_sub(32)) as u8),
    }
}

fn read_cr2() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    v
}

global_asm!(
    r#"
    .macro PUSH_REGS
        push rax
        push rcx
        push rdx
        push rbx
        push rbp
        push rsi
        push rdi
        push r8
        push r9
        push r10
        push r11
        push r12
        push r13
        push r14
        push r15
    .endm

    .macro POP_REGS
        pop r15
        pop r14
        pop r13
        pop r12
        pop r11
        pop r10
        pop r9
        pop r8
        pop rdi
        pop rsi
        pop rbp
        pop rbx
        pop rdx
        pop rcx
        pop rax
    .endm

    .global isr_stub_generic
    isr_stub_generic:
        push 0
        push 0xFF
        jmp isr_common

    .global isr_stub_0
    isr_stub_0:
        push 0
        push 0
        jmp isr_common

    .global isr_stub_8
    isr_stub_8:
        push 8
        jmp isr_common

    .global isr_stub_13
    isr_stub_13:
        push 13
        jmp isr_common

    .global isr_stub_14
    isr_stub_14:
        push 14
        jmp isr_common

    .global isr_stub_32
    isr_stub_32:
        push 0
        push 32
        jmp isr_common

    .global isr_stub_33
    isr_stub_33:
        push 0
        push 33
        jmp isr_common

    isr_common:
        PUSH_REGS
        mov rdi, rsp
        call isr_dispatch
        POP_REGS
        add rsp, 16
        iretq
    "#
);
