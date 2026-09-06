//! x86_64 KPTI subset: user CR3 has no kernel HH and no identity DMA.
//!
//! Kernel CR3 keeps identity *islands* (low 2 MiB SIPI / mailbox /
//! trampoline, virtio-blk, APIC) plus higher-half. SoftNPU uses
//! `KernelDma` + Soft SMMU (IOVA → PA → HH), not a 4 GiB identity
//! window. Page-table walks use `phys_va`. User CR3 maps the task
//! ELF window (USER) plus four supervisor 4 KiB pages at
//! [`KPTI_TRAMP_VA`] (syscall/IRQ trampoline, shadow IDT, entry
//! stack). CR3 switches to the kernel map on enter and back on
//! exit. SoftNPU kthread-B stays on kernel CR3.
//!
//! Not Meltdown-complete (trampoline pages remain mapped). The unused
//! KASLR canonical alias is unmapped on the kernel map after PIE
//! relocs. PCID tags the KPTI `mov cr3` when CPUID advertises it;
//! otherwise each switch is still a full flush. RISC-V / aarch64 are
//! unchanged.

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use aether_core::{
    KPTI_SLOT_BASE, KPTI_TRAMP_IDT, KPTI_TRAMP_STACK, KPTI_TRAMP_STACK_TOP, KPTI_TRAMP_VA,
};

use super::gdt::{KCODE, TSS_SEL, USER_CS};
use super::idt::InterruptFrame;
use super::io::wrmsr;
use crate::mm::paging;
use crate::println;

const IA32_LSTAR: u32 = 0xC000_0082;

const SLOT_KCR3: u64 = KPTI_SLOT_BASE;
const SLOT_UCR3: u64 = KPTI_SLOT_BASE + 8;
const SLOT_URSP: u64 = KPTI_SLOT_BASE + 16;
const SLOT_SCRATCH: u64 = KPTI_SLOT_BASE + 24;
const SLOT_ISR_COMMON: u64 = KPTI_SLOT_BASE + 32;
const SLOT_SYSCALL_CONT: u64 = KPTI_SLOT_BASE + 40;
const SLOT_IRET_USER: u64 = KPTI_SLOT_BASE + 48;

const GDT_PA: u64 = 0x7_3E00;
const TSS_PA: u64 = 0x7_3E80;
const GDTR_PA: u64 = 0x7_3EE0;
const IDTR_PA: u64 = 0x7_3EF0;

const FRAME_QWORDS: usize = 22;
const FRAME_SIZE: u64 = (FRAME_QWORDS * 8) as u64;

static ARMED: AtomicBool = AtomicBool::new(false);
static USER_CR3: AtomicU64 = AtomicU64::new(0);

extern "C" {
    fn kpti_tramp_start();
    fn kpti_tramp_end();
    fn kpti_syscall_entry();
    fn kpti_iret_user();
    fn kpti_isr_generic();
    fn kpti_isr_0();
    fn kpti_isr_8();
    fn kpti_isr_13();
    fn kpti_isr_14();
    fn kpti_isr_32();
    fn kpti_isr_33();
    fn kpti_isr_48();
    fn isr_common();
    fn syscall_entry();
}

fn write64(pa: u64, v: u64) {
    unsafe {
        core::ptr::write_volatile(pa as *mut u64, v);
    }
}

fn tramp_off(sym: unsafe extern "C" fn()) -> u64 {
    (sym as usize as u64).wrapping_sub(kpti_tramp_start as usize as u64)
}

fn tramp_pa(sym: unsafe extern "C" fn()) -> u64 {
    KPTI_TRAMP_VA + tramp_off(sym)
}

pub fn armed() -> bool {
    ARMED.load(Ordering::Acquire)
}

pub fn user_cr3() -> u64 {
    USER_CR3.load(Ordering::Acquire)
}

pub fn set_user_cr3(cr3: u64) {
    let v = cr3 & !0xFFF;
    USER_CR3.store(v, Ordering::Release);
    if ARMED.load(Ordering::Relaxed) {
        write64(SLOT_UCR3, paging::tagged_cr3(v));
    }
}

/// Rewrite trampoline CR3 slots after PCIDE is armed (or on AP bring-up).
pub fn sync_cr3_slots() {
    if !ARMED.load(Ordering::Relaxed) {
        return;
    }
    write64(SLOT_KCR3, paging::tagged_cr3(paging::kernel_cr3()));
    let u = USER_CR3.load(Ordering::Acquire);
    write64(SLOT_UCR3, if u == 0 { 0 } else { paging::tagged_cr3(u) });
}

pub fn set_rsp0(rsp0: u64) {
    if ARMED.load(Ordering::Relaxed) {
        write64(TSS_PA + 4, rsp0);
    }
}

/// Copy `frame` onto the trampoline stack and `iretq` into ring-3
/// after switching to the user CR3. Caller must be on kernel CR3.
pub unsafe fn enter_user(frame: &InterruptFrame) -> ! {
    let dst = (KPTI_TRAMP_STACK_TOP - FRAME_SIZE) as *mut InterruptFrame;
    core::ptr::write(dst, *frame);
    let iret = tramp_pa(kpti_iret_user);
    core::arch::asm!(
        "mov rsp, {dst}",
        "jmp {iret}",
        dst = in(reg) dst as u64,
        iret = in(reg) iret,
        options(noreturn)
    );
}

#[repr(C, packed)]
struct DescPtr {
    limit: u16,
    base: u64,
}

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

fn write_idt_gate(vec: usize, handler: u64, trap: bool) {
    let flags = if trap { 0x8Fu8 } else { 0x8Eu8 };
    let e = KPTI_TRAMP_IDT + (vec as u64) * 16;
    unsafe {
        core::ptr::write_unaligned(e as *mut u16, handler as u16);
        core::ptr::write_unaligned((e + 2) as *mut u16, KCODE);
        core::ptr::write((e + 4) as *mut u8, 0u8);
        core::ptr::write((e + 5) as *mut u8, flags);
        core::ptr::write_unaligned((e + 6) as *mut u16, (handler >> 16) as u16);
        core::ptr::write_unaligned((e + 8) as *mut u32, (handler >> 32) as u32);
        core::ptr::write_unaligned((e + 12) as *mut u32, 0);
    }
}

fn install_identity_gdt() {
    unsafe {
        core::ptr::write_bytes(GDT_PA as *mut u8, 0, 256);
        core::ptr::write_bytes(TSS_PA as *mut u8, 0, 128);
    }
    write64(GDT_PA, 0);
    write64(GDT_PA + 8, gdt_code(0));
    write64(GDT_PA + 16, gdt_data(0));
    write64(GDT_PA + 24, gdt_data(3));
    write64(GDT_PA + 32, gdt_code(3));
    let (lo, hi) = gdt_tss(TSS_PA, 103);
    write64(GDT_PA + 40, lo);
    write64(GDT_PA + 48, hi);
    write64(TSS_PA + 4, KPTI_TRAMP_STACK_TOP);
    // iomap_base at offset 102
    unsafe {
        core::ptr::write_unaligned((TSS_PA + 102) as *mut u16, 104);
    }
    let gdtr = DescPtr {
        limit: 8 * 8 - 1,
        base: GDT_PA,
    };
    unsafe {
        core::ptr::write_unaligned(GDTR_PA as *mut DescPtr, gdtr);
        core::arch::asm!(
            "lgdt [{0}]",
            "ltr {1:x}",
            in(reg) GDTR_PA,
            in(reg) TSS_SEL,
            options(nostack, preserves_flags)
        );
    }
}

fn install_identity_idt() {
    unsafe {
        core::ptr::write_bytes(KPTI_TRAMP_IDT as *mut u8, 0, 4096);
    }
    let generic = tramp_pa(kpti_isr_generic);
    for i in 0..256 {
        write_idt_gate(i, generic, false);
    }
    write_idt_gate(0, tramp_pa(kpti_isr_0), true);
    write_idt_gate(8, tramp_pa(kpti_isr_8), true);
    write_idt_gate(13, tramp_pa(kpti_isr_13), true);
    write_idt_gate(14, tramp_pa(kpti_isr_14), true);
    write_idt_gate(32, tramp_pa(kpti_isr_32), false);
    write_idt_gate(33, tramp_pa(kpti_isr_33), false);
    write_idt_gate(48, tramp_pa(kpti_isr_48), false);
    let idtr = DescPtr {
        limit: 4096 - 1,
        base: KPTI_TRAMP_IDT,
    };
    unsafe {
        core::ptr::write_unaligned(IDTR_PA as *mut DescPtr, idtr);
        core::arch::asm!(
            "lidt [{0}]",
            in(reg) IDTR_PA,
            options(readonly, nostack, preserves_flags)
        );
    }
}

pub fn init() {
    let start = kpti_tramp_start as usize;
    let end = kpti_tramp_end as usize;
    let n = end.saturating_sub(start);
    if n == 0 || n > 0xE00 {
        println!("[mm] kpti trampoline size bad; refusing");
        crate::arch::exit_qemu(false);
        crate::arch::idle();
    }
    unsafe {
        core::ptr::copy_nonoverlapping(start as *const u8, KPTI_TRAMP_VA as *mut u8, n);
        core::ptr::write_bytes((KPTI_TRAMP_VA + n as u64) as *mut u8, 0, 0x1000 - n);
    }

    write64(SLOT_KCR3, paging::tagged_cr3(paging::kernel_cr3()));
    write64(SLOT_UCR3, 0);
    write64(SLOT_URSP, 0);
    write64(SLOT_SCRATCH, 0);
    write64(SLOT_ISR_COMMON, isr_common as usize as u64);
    write64(SLOT_SYSCALL_CONT, syscall_entry as usize as u64);
    write64(SLOT_IRET_USER, tramp_pa(kpti_iret_user));

    install_identity_gdt();
    install_identity_idt();
    wrmsr(IA32_LSTAR, tramp_pa(kpti_syscall_entry));
    ARMED.store(true, Ordering::Release);
    set_rsp0(KPTI_TRAMP_STACK_TOP);

    println!(
        "[mm] kpti trampoline @ 0x73000 (user CR3: no HH, no identity DMA; PCID if CPUID; not Meltdown-complete)"
    );
}

/// APs share the identity GDT/IDT. They stay on kernel CR3.
pub fn load_ap() {
    if !ARMED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        core::arch::asm!(
            "lgdt [{0}]",
            "lidt [{1}]",
            in(reg) GDTR_PA,
            in(reg) IDTR_PA,
            options(nostack, preserves_flags)
        );
    }
}

global_asm!(
    r#"
    .global kpti_tramp_start
    kpti_tramp_start:

    .global kpti_syscall_entry
    kpti_syscall_entry:
        mov qword ptr [{slot_ursp}], rsp
        mov qword ptr [{slot_scratch}], rax
        mov rax, qword ptr [{slot_kcr3}]
        mov cr3, rax
        mov rax, qword ptr [{slot_scratch}]
        jmp qword ptr [{slot_syscall}]

    .macro KPTI_ISR_ENTER
        push rax
        mov rax, cr3
        xor rax, qword ptr [{slot_kcr3}]
        and rax, 0xFFFFFFFFFFFFF000
        jz 91f
        mov rax, qword ptr [{slot_kcr3}]
        mov cr3, rax
    91:
        pop rax
        jmp qword ptr [{slot_isr}]
    .endm

    .global kpti_isr_generic
    kpti_isr_generic:
        push 0
        push 0xFF
        KPTI_ISR_ENTER

    .global kpti_isr_0
    kpti_isr_0:
        push 0
        push 0
        KPTI_ISR_ENTER

    .global kpti_isr_8
    kpti_isr_8:
        push 8
        KPTI_ISR_ENTER

    .global kpti_isr_13
    kpti_isr_13:
        push 13
        KPTI_ISR_ENTER

    .global kpti_isr_14
    kpti_isr_14:
        push 14
        KPTI_ISR_ENTER

    .global kpti_isr_32
    kpti_isr_32:
        push 0
        push 32
        KPTI_ISR_ENTER

    .global kpti_isr_33
    kpti_isr_33:
        push 0
        push 33
        KPTI_ISR_ENTER

    .global kpti_isr_48
    kpti_isr_48:
        push 0
        push 48
        KPTI_ISR_ENTER

    .global kpti_iret_user
    kpti_iret_user:
        mov rax, qword ptr [{slot_ucr3}]
        test rax, rax
        jz 92f
        mov cr3, rax
    92:
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
        add rsp, 16
        iretq

    .global kpti_tramp_end
    kpti_tramp_end:

    .global kpti_exit
    kpti_exit:
        cmp qword ptr [rsp + 144], {user_cs}
        jne 93f
        mov rax, rsp
        cmp rax, {stack_lo}
        jb 94f
        cmp rax, {stack_hi}
        jae 94f
        jmp qword ptr [{slot_iret}]
    94:
        mov rsi, rsp
        mov rdi, {frame_dst}
        mov rcx, 22
        cld
        rep movsq
        mov rsp, {frame_dst}
        jmp qword ptr [{slot_iret}]
    93:
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
        add rsp, 16
        iretq
    "#,
    slot_kcr3 = const SLOT_KCR3,
    slot_ucr3 = const SLOT_UCR3,
    slot_ursp = const SLOT_URSP,
    slot_scratch = const SLOT_SCRATCH,
    slot_isr = const SLOT_ISR_COMMON,
    slot_syscall = const SLOT_SYSCALL_CONT,
    slot_iret = const SLOT_IRET_USER,
    user_cs = const USER_CS as u64,
    stack_lo = const KPTI_TRAMP_STACK,
    stack_hi = const KPTI_TRAMP_STACK_TOP,
    frame_dst = const KPTI_TRAMP_STACK_TOP - FRAME_SIZE,
);
