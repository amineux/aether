//! EL1 exception vectors. The virtual timer is the only handled IRQ.

use core::arch::global_asm;

use crate::arch::aarch64::timer;
use crate::arch::irq;

#[repr(C)]
pub struct TrapFrame {
    pub regs: [u64; 31],
    pub elr: u64,
    pub spsr: u64,
    pub esr: u64,
}

extern "C" {
    fn exception_vectors();
}

pub fn init() {
    unsafe {
        core::arch::asm!(
            "msr vbar_el1, {0}",
            "isb",
            in(reg) exception_vectors as usize,
            options(nostack)
        );
    }
    crate::println!("[boot] VBAR_EL1 set; GICv2 only (no EL0 vectors used)");
}

#[no_mangle]
pub extern "C" fn trap_dispatch(frame: &mut TrapFrame) {
    let esr = frame.esr;
    let ec = (esr >> 26) & 0x3F;
    // IRQ path: the vector stub stores esr=0xFFFF_FFFF as a sentinel.
    if esr == 0xFFFF_FFFF {
        let iar = timer::ack();
        let id = iar & 0x3FF;
        if id == timer::TIMER_IRQ {
            irq::inc_ticks();
            timer::rearm();
        }
        if id < 1020 {
            timer::eoi(iar);
        }
        return;
    }
    crate::console::write_str("[fault] esr=");
    crate::console::write_hex(esr);
    crate::console::write_str(" ec=");
    crate::console::write_hex(ec);
    crate::console::write_str(" elr=");
    crate::console::write_hex(frame.elr);
    crate::console::nl();
    crate::arch::aarch64::idle();
}

global_asm!(
    r#"
    .align 11
    .globl exception_vectors
exception_vectors:
    /* Current EL, SP_EL0 */
    .align 7
    b       sync_el1
    .align 7
    b       irq_el1
    .align 7
    b       sync_el1
    .align 7
    b       sync_el1

    /* Current EL, SP_ELx */
    .align 7
    b       sync_el1
    .align 7
    b       irq_el1
    .align 7
    b       sync_el1
    .align 7
    b       sync_el1

    /* Lower EL, AArch64 — unused (no EL0) */
    .align 7
    b       sync_el1
    .align 7
    b       irq_el1
    .align 7
    b       sync_el1
    .align 7
    b       sync_el1

    /* Lower EL, AArch32 */
    .align 7
    b       sync_el1
    .align 7
    b       irq_el1
    .align 7
    b       sync_el1
    .align 7
    b       sync_el1

sync_el1:
    sub     sp, sp, #272
    stp     x0,  x1,  [sp, #0]
    stp     x2,  x3,  [sp, #16]
    stp     x4,  x5,  [sp, #32]
    stp     x6,  x7,  [sp, #48]
    stp     x8,  x9,  [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    str     x30,      [sp, #240]
    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    mrs     x2, esr_el1
    stp     x0, x1, [sp, #248]
    str     x2,     [sp, #264]
    mov     x0, sp
    bl      trap_dispatch
    b       trap_return

irq_el1:
    sub     sp, sp, #272
    stp     x0,  x1,  [sp, #0]
    stp     x2,  x3,  [sp, #16]
    stp     x4,  x5,  [sp, #32]
    stp     x6,  x7,  [sp, #48]
    stp     x8,  x9,  [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    str     x30,      [sp, #240]
    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    mov     x2, #0xffffffff
    stp     x0, x1, [sp, #248]
    str     x2,     [sp, #264]
    mov     x0, sp
    bl      trap_dispatch
    /* fall through */

trap_return:
    ldp     x0, x1, [sp, #248]
    msr     elr_el1, x0
    msr     spsr_el1, x1
    ldp     x0,  x1,  [sp, #0]
    ldp     x2,  x3,  [sp, #16]
    ldp     x4,  x5,  [sp, #32]
    ldp     x6,  x7,  [sp, #48]
    ldp     x8,  x9,  [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    ldp     x16, x17, [sp, #128]
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    ldr     x30,      [sp, #240]
    add     sp, sp, #272
    eret
    "#
);
