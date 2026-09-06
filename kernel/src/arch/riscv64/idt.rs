//! Supervisor trap vector. Timer, PLIC (SoftNPU doorbell), U-mode `ecall`.
//!
//! `sscratch` is the kernel stack top in U-mode and 0 in S-mode, so a
//! trap from user does not write the user stack (SUM is off).

use core::arch::global_asm;

use crate::arch::irq;
use crate::arch::riscv64::{plic, timer};

/// sstatus.SPP — previous privilege (1 = S, 0 = U).
pub const SSTATUS_SPP: u64 = 1 << 8;
/// sstatus.SPIE — previous SIE, restored by `sret`.
pub const SSTATUS_SPIE: u64 = 1 << 5;

const SCAUSE_U_ECALL: u64 = 8;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InterruptFrame {
    /// x1 … x31
    pub regs: [u64; 31],
    pub sepc: u64,
    pub scause: u64,
    pub sstatus: u64,
}

impl InterruptFrame {
    fn x(&self, n: usize) -> u64 {
        self.regs[n - 1]
    }

    fn set_x(&mut self, n: usize, v: u64) {
        self.regs[n - 1] = v;
    }

    pub fn syscall_nr(&self) -> u64 {
        self.x(17)
    }
    pub fn arg0(&self) -> u64 {
        self.x(10)
    }
    pub fn arg1(&self) -> u64 {
        self.x(11)
    }
    pub fn arg2(&self) -> u64 {
        self.x(12)
    }
    pub fn set_ret(&mut self, v: u64) {
        self.set_x(10, v);
    }
    pub fn set_sp(&mut self, v: u64) {
        self.set_x(2, v);
    }
}

extern "C" {
    fn trap_vector();
}

pub fn init() {
    unsafe {
        core::arch::asm!(
            "csrw stvec, {0}",
            "csrw sscratch, zero",
            in(reg) trap_vector as usize,
            options(nostack)
        );
    }
    crate::arch::riscv64::plic::init();
    crate::println!("[boot] stvec set (direct); U-mode ecall + sscratch + PLIC");
}

#[no_mangle]
pub extern "C" fn trap_dispatch(frame: &mut InterruptFrame) {
    let interrupt = frame.scause >> 63 != 0;
    let code = frame.scause & 0xFF;
    if interrupt && code == 5 {
        irq::inc_ticks();
        timer::rearm();
        // Last-resort SoftNPU drain if the PLIC/SSIP doorbell was missed.
        crate::world::run_pending_accel();
        crate::task::on_timer(frame);
        return;
    }
    if interrupt && code == 9 {
        let irq_id = plic::claim();
        if irq_id == plic::SOFTNPU_IRQ {
            plic::ack_softnpu_doorbell();
            crate::console::write_str("[plic] claim irq=");
            crate::console::write_u64(irq_id as u64);
            crate::console::write_str(" SoftNPU used-ring");
            crate::console::nl();
            crate::world::run_pending_accel();
        }
        plic::complete(irq_id);
        return;
    }
    if interrupt && code == 1 {
        plic::clear_ssip();
        crate::world::run_pending_accel();
        return;
    }
    if !interrupt && code == SCAUSE_U_ECALL {
        frame.sepc = frame.sepc.wrapping_add(4);
        crate::syscall::from_user_trap(frame);
        return;
    }
    if !interrupt {
        crate::console::write_str("[fault] scause=");
        crate::console::write_hex(frame.scause);
        crate::console::write_str(" sepc=");
        crate::console::write_hex(frame.sepc);
        crate::console::nl();
        if code == 2 || code == 1 || code == 5 || code == 7 || code == 12 || code == 13 || code == 15
        {
            crate::arch::riscv64::idle();
        }
    }
}

global_asm!(
    r#"
    .align 2
    .globl trap_vector
    trap_vector:
        csrrw   sp, sscratch, sp
        bnez    sp, .Lsave
        csrrw   sp, sscratch, sp
    .Lsave:
        addi    sp, sp, -272
        sd      x1,    0(sp)
        sd      x3,   16(sp)
        sd      x4,   24(sp)
        sd      x5,   32(sp)
        sd      x6,   40(sp)
        sd      x7,   48(sp)
        sd      x8,   56(sp)
        sd      x9,   64(sp)
        sd      x10,  72(sp)
        sd      x11,  80(sp)
        sd      x12,  88(sp)
        sd      x13,  96(sp)
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
        csrr    t0, sscratch
        bnez    t0, 1f
        addi    t0, sp, 272
    1:
        sd      t0, 8(sp)
        csrw    sscratch, zero
        csrr    t0, sepc
        sd      t0, 248(sp)
        csrr    t0, scause
        sd      t0, 256(sp)
        csrr    t0, sstatus
        sd      t0, 264(sp)
        mv      a0, sp
        call    trap_dispatch
        .globl trap_return
    trap_return:
        ld      t0, 248(sp)
        csrw    sepc, t0
        ld      t0, 264(sp)
        csrw    sstatus, t0
        andi    t1, t0, 0x100
        bnez    t1, 2f
        csrr    t1, sscratch
        bnez    t1, 2f
        addi    t1, sp, 272
        csrw    sscratch, t1
    2:
        ld      x1,    0(sp)
        ld      x3,   16(sp)
        ld      x4,   24(sp)
        ld      x5,   32(sp)
        ld      x6,   40(sp)
        ld      x7,   48(sp)
        ld      x8,   56(sp)
        ld      x9,   64(sp)
        ld      x10,  72(sp)
        ld      x11,  80(sp)
        ld      x12,  88(sp)
        ld      x13,  96(sp)
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
        ld      sp,    8(sp)
        sret
    "#
);
