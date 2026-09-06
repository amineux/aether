//! Supervisor trap vector. Timer is the only handled interrupt.

use core::arch::global_asm;

use crate::arch::irq;
use crate::arch::riscv64::timer;

#[repr(C)]
pub struct TrapFrame {
    pub regs: [u64; 31],
    pub sepc: u64,
    pub scause: u64,
    pub sstatus: u64,
}

extern "C" {
    fn trap_vector();
}

pub fn init() {
    unsafe {
        core::arch::asm!(
            "csrw stvec, {0}",
            in(reg) trap_vector as usize,
            options(nostack)
        );
    }
    crate::println!("[boot] stvec set (direct); PLIC unused in this port");
}

#[no_mangle]
pub extern "C" fn trap_dispatch(frame: &mut TrapFrame) {
    let interrupt = frame.scause >> 63 != 0;
    let code = frame.scause & 0xFF;
    if interrupt && code == 5 {
        irq::inc_ticks();
        timer::rearm();
        return;
    }
    if !interrupt {
        crate::console::write_str("[fault] scause=");
        crate::console::write_hex(frame.scause);
        crate::console::write_str(" sepc=");
        crate::console::write_hex(frame.sepc);
        crate::console::nl();
        if code == 2 || code == 1 || code == 5 || code == 7 {
            crate::arch::riscv64::idle();
        }
    }
}

global_asm!(
    r#"
    .align 2
    .globl trap_vector
    trap_vector:
        addi    sp, sp, -272
        sd      x1,   0(sp)
        sd      x3,  16(sp)
        sd      x4,  24(sp)
        sd      x5,  32(sp)
        sd      x6,  40(sp)
        sd      x7,  48(sp)
        sd      x8,  56(sp)
        sd      x9,  64(sp)
        sd      x10, 72(sp)
        sd      x11, 80(sp)
        sd      x12, 88(sp)
        sd      x13, 96(sp)
        sd      x14, 104(sp)
        sd      x15, 112(sp)
        sd      x16, 120(sp)
        sd      x17, 128(sp)
        sd      x18, 136(sp)
        sd      x19, 144(sp)
        sd      x20, 152(sp)
        sd      x21, 160(sp)
        sd      x22, 168(sp)
        sd      x23, 176(sp)
        sd      x24, 184(sp)
        sd      x25, 192(sp)
        sd      x26, 200(sp)
        sd      x27, 208(sp)
        sd      x28, 216(sp)
        sd      x29, 224(sp)
        sd      x30, 232(sp)
        sd      x31, 240(sp)
        csrr    t0, sepc
        sd      t0, 248(sp)
        csrr    t0, scause
        sd      t0, 256(sp)
        csrr    t0, sstatus
        sd      t0, 264(sp)
        mv      a0, sp
        call    trap_dispatch
        ld      t0, 248(sp)
        csrw    sepc, t0
        ld      t0, 264(sp)
        csrw    sstatus, t0
        ld      x1,   0(sp)
        ld      x3,  16(sp)
        ld      x4,  24(sp)
        ld      x5,  32(sp)
        ld      x6,  40(sp)
        ld      x7,  48(sp)
        ld      x8,  56(sp)
        ld      x9,  64(sp)
        ld      x10, 72(sp)
        ld      x11, 80(sp)
        ld      x12, 88(sp)
        ld      x13, 96(sp)
        ld      x14, 104(sp)
        ld      x15, 112(sp)
        ld      x16, 120(sp)
        ld      x17, 128(sp)
        ld      x18, 136(sp)
        ld      x19, 144(sp)
        ld      x20, 152(sp)
        ld      x21, 160(sp)
        ld      x22, 168(sp)
        ld      x23, 176(sp)
        ld      x24, 184(sp)
        ld      x25, 192(sp)
        ld      x26, 200(sp)
        ld      x27, 208(sp)
        ld      x28, 216(sp)
        ld      x29, 224(sp)
        ld      x30, 232(sp)
        ld      x31, 240(sp)
        addi    sp, sp, 272
        sret
    "#
);
