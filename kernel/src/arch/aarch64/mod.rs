//! AArch64 EL1 HAL for QEMU virt (GICv2 + PL011).
//!
//! Thin v0.1 port: PL011 console, generic virtual timer, VBAR, TTBR0
//! walk. No EL0, no virtio, no SMP. `aether-core` is unchanged.

use core::arch::global_asm;

pub mod idt;
pub mod serial;
pub mod timer;

global_asm!(include_str!("../../../../boot/aarch64/trampoline.S"));

pub const UART0: usize = 0x0900_0000;
pub const GICD: usize = 0x0800_0000;
pub const GICC: usize = 0x0801_0000;
pub const KERNEL_VA: u64 = 0x4008_0000;
pub const FRAME_START: u64 = 0x4100_0000;
pub const FRAME_END: u64 = 0x4800_0000;

pub fn console_name() -> &'static str {
    "PL011 0x09000000"
}

pub fn idle() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

/// Exit QEMU via Angel semihosting SYS_EXIT (needs `-semihosting`).
/// ADP_Stopped_ApplicationExit + status 0 is a clean pass.
pub fn exit_qemu(success: bool) {
    let status: u64 = if success { 0 } else { 1 };
    let block = [0x20026u64, status];
    unsafe {
        core::arch::asm!(
            "mov x0, #0x18",
            "mov x1, {block}",
            "hlt #0xF000",
            block = in(reg) block.as_ptr(),
            options(nostack)
        );
    }
    // PSCI SYSTEM_OFF if the debugger ignored HLT.
    unsafe {
        core::arch::asm!(
            "movz x0, #0x0008",
            "movk x0, #0x8400, lsl #16",
            "hvc #0",
            options(nostack)
        );
    }
}
