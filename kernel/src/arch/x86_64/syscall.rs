//! SYSCALL / SYSRET gate: EFER.SCE, STAR, LSTAR, SFMASK.

use core::arch::global_asm;

use aether_core::KPTI_SLOT_BASE;

use super::gdt::{KCODE, KDATA, USER_CS};
use super::idt::InterruptFrame;
use super::io::{rdmsr, wrmsr};

/// User RSP saved by the identity trampoline before the kernel CR3 switch.
const KPTI_SLOT_URSP: u64 = KPTI_SLOT_BASE + 16;

const IA32_EFER: u32 = 0xC000_0080;
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;
const EFER_SCE: u64 = 1;

/// IF | DF | AC — syscalls run with interrupts disabled and SMAP armed
/// (RFLAGS.AC cleared; STAC only around copy_from/to_user).
const SFMASK: u64 = (1 << 9) | (1 << 10) | (1 << 18);

/// Per-CPU (UP) slots the syscall trampoline uses. Updated on every switch.
#[no_mangle]
pub static mut SYSCALL_USER_RSP: u64 = 0;
#[no_mangle]
pub static mut SYSCALL_KSTACK: u64 = 0;

extern "C" {
    fn syscall_entry();
}

pub fn init() {
    let star = ((KDATA as u64) << 48) | ((KCODE as u64) << 32);
    wrmsr(IA32_STAR, star);
    wrmsr(IA32_LSTAR, syscall_entry as usize as u64);
    wrmsr(IA32_FMASK, SFMASK);
    wrmsr(IA32_EFER, rdmsr(IA32_EFER) | EFER_SCE);
    crate::println!("[boot] SYSCALL/SYSRET armed (STAR/LSTAR/SFMASK, EFER.SCE)");
}

pub fn set_kstack(top: u64) {
    unsafe {
        SYSCALL_KSTACK = top;
    }
}

#[no_mangle]
pub extern "C" fn syscall_from_user(frame: &mut InterruptFrame) {
    crate::syscall::from_user_trap(frame);
}

global_asm!(
    r#"
    .macro SYSCALL_PUSH_REGS
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

    .macro SYSCALL_POP_REGS
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

    .global syscall_entry
    syscall_entry:
        mov rsp, [rip + SYSCALL_KSTACK]
        push {user_ss}
        push qword ptr [{slot_ursp}]
        push r11
        push {user_cs}
        push rcx
        push 0
        push 0x80
        SYSCALL_PUSH_REGS
        mov rdi, rsp
        call syscall_from_user
        jmp kpti_exit
    "#,
    user_cs = const USER_CS as u64,
    user_ss = const super::gdt::USER_DS as u64,
    slot_ursp = const KPTI_SLOT_URSP,
);
