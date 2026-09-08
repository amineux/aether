//! EL1 exception vectors: CNTV IRQ, EL0 `svc`, `eret` return.
//!
//! `tpidr_el1` holds the user thread's kernel stack top so a trap from
//! EL0 does not write the user stack. PAN is not available on
//! cortex-a72 (v8.0); EL1 can touch AP_EL0 pages without a SUM analogue.
//!
//! The trampoline enables FP/SIMD (`CPACR_EL1.FPEN`) so rustc `memcpy`
//! does not UNDEF. IRQ/sync therefore save q0–q31 + FPSR/FPCR. Without
//! that, a 100 Hz CNTV tick clobbers in-flight NEON (OperatorInject
//! hot-add and SoftNPU F16/F32 clips have failed that way on virt).

use core::arch::global_asm;

use crate::arch::aarch64::timer;
use crate::arch::irq;

/// SPSR_EL1.M EL1h (SP_ELx).
pub const SPSR_EL1H: u64 = 0x5;
/// ESR_EL1.EC = SVC from AArch64.
const ESR_EC_SVC64: u64 = 0x15;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InterruptFrame {
    /// x0 … x30
    pub regs: [u64; 31],
    pub elr: u64,
    pub spsr: u64,
    pub esr: u64,
    /// SP_EL0 if the trap came from EL0; pre-trap SP_EL1 otherwise.
    pub sp: u64,
    pub _pad: u64,
}

impl InterruptFrame {
    pub fn syscall_nr(&self) -> u64 {
        self.regs[8]
    }
    pub fn arg0(&self) -> u64 {
        self.regs[0]
    }
    pub fn arg1(&self) -> u64 {
        self.regs[1]
    }
    pub fn arg2(&self) -> u64 {
        self.regs[2]
    }
    pub fn set_ret(&mut self, v: u64) {
        self.regs[0] = v;
    }
    pub fn set_sp(&mut self, v: u64) {
        self.sp = v;
    }
}

const _: () = assert!(core::mem::size_of::<InterruptFrame>() == 288);

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
    crate::println!("[boot] VBAR_EL1 set; EL0 svc + GICv2");
}

#[no_mangle]
pub extern "C" fn trap_dispatch(frame: &mut InterruptFrame) {
    let esr = frame.esr;
    // IRQ path: the vector stub stores esr=0xFFFF_FFFF as a sentinel.
    if esr == 0xFFFF_FFFF {
        let iar = timer::ack();
        let id = iar & 0x3FF;
        if id == timer::TIMER_IRQ {
            irq::inc_ticks();
            timer::rearm();
            crate::world::run_pending_accel();
            crate::task::on_timer(frame);
        }
        if id < 1020 {
            timer::eoi(iar);
        }
        return;
    }
    let ec = (esr >> 26) & 0x3F;
    if ec == ESR_EC_SVC64 {
        // ELR already points past `svc`.
        crate::syscall::from_user_trap(frame);
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
    /* 32×16-byte Q regs + FPSR/FPCR. 16-byte aligned. */
    .macro SAVE_FPSIMD
    sub     sp, sp, #528
    stp     q0,  q1,  [sp, #0]
    stp     q2,  q3,  [sp, #32]
    stp     q4,  q5,  [sp, #64]
    stp     q6,  q7,  [sp, #96]
    stp     q8,  q9,  [sp, #128]
    stp     q10, q11, [sp, #160]
    stp     q12, q13, [sp, #192]
    stp     q14, q15, [sp, #224]
    stp     q16, q17, [sp, #256]
    stp     q18, q19, [sp, #288]
    stp     q20, q21, [sp, #320]
    stp     q22, q23, [sp, #352]
    stp     q24, q25, [sp, #384]
    stp     q26, q27, [sp, #416]
    stp     q28, q29, [sp, #448]
    stp     q30, q31, [sp, #480]
    mrs     x16, fpsr
    mrs     x17, fpcr
    str     x16, [sp, #512]
    str     x17, [sp, #520]
    .endm

    .macro RESTORE_FPSIMD
    ldr     x16, [sp, #512]
    ldr     x17, [sp, #520]
    msr     fpsr, x16
    msr     fpcr, x17
    ldp     q0,  q1,  [sp, #0]
    ldp     q2,  q3,  [sp, #32]
    ldp     q4,  q5,  [sp, #64]
    ldp     q6,  q7,  [sp, #96]
    ldp     q8,  q9,  [sp, #128]
    ldp     q10, q11, [sp, #160]
    ldp     q12, q13, [sp, #192]
    ldp     q14, q15, [sp, #224]
    ldp     q16, q17, [sp, #256]
    ldp     q18, q19, [sp, #288]
    ldp     q20, q21, [sp, #320]
    ldp     q22, q23, [sp, #352]
    ldp     q24, q25, [sp, #384]
    ldp     q26, q27, [sp, #416]
    ldp     q28, q29, [sp, #448]
    ldp     q30, q31, [sp, #480]
    add     sp, sp, #528
    .endm

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

    /* Lower EL, AArch64 */
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
    sub     sp, sp, #288
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
    and     x3, x1, #0xf
    cbnz    x3, 1f
    mrs     x3, sp_el0
    b       2f
1:
    add     x3, sp, #288
2:
    str     x3, [sp, #272]
    SAVE_FPSIMD
    add     x0, sp, #528
    bl      trap_dispatch
    RESTORE_FPSIMD
    b       trap_return

irq_el1:
    sub     sp, sp, #288
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
    and     x3, x1, #0xf
    cbnz    x3, 3f
    mrs     x3, sp_el0
    b       4f
3:
    add     x3, sp, #288
4:
    str     x3, [sp, #272]
    SAVE_FPSIMD
    add     x0, sp, #528
    bl      trap_dispatch
    RESTORE_FPSIMD
    /* fall through */

    .globl trap_return
trap_return:
    ldp     x0, x1, [sp, #248]
    msr     elr_el1, x0
    msr     spsr_el1, x1
    ldr     x2, [sp, #272]
    msr     sp_el0, x2
    and     x3, x1, #0xf
    cbnz    x3, trap_return_el1
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
    add     sp, sp, #288
    mrs     x16, tpidr_el1
    cbz     x16, 5f
    mov     sp, x16
5:
    eret

trap_return_el1:
    ldr     x16, [sp, #272]
    ldp     x0,  x1,  [sp, #0]
    ldp     x2,  x3,  [sp, #16]
    ldp     x4,  x5,  [sp, #32]
    ldp     x6,  x7,  [sp, #48]
    ldp     x8,  x9,  [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    /* keep x16 = resume SP */
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    ldr     x30,      [sp, #240]
    ldp     x14, x15, [sp, #112]
    mov     sp, x16
    eret
    "#
);
