//! RISC-V S-mode HAL for QEMU virt (OpenSBI).
//!
//! Thin v0.1 port: UART0 console, SBI timer, stvec, Sv39 walk. No PLIC
//! virtio, no ring-3, no SMP. `aether-core` is unchanged.

use core::arch::global_asm;

pub mod idt;
pub mod serial;
pub mod timer;

global_asm!(include_str!("../../../../boot/riscv64/trampoline.S"));

pub const UART0: usize = 0x1000_0000;
pub const TEST_FINISHER: usize = 0x0010_0000;
pub const KERNEL_VA: u64 = 0x8020_0000;
pub const FRAME_START: u64 = 0x8100_0000;
pub const FRAME_END: u64 = 0x8800_0000;

const FINISHER_PASS: u32 = 0x5555;
const FINISHER_FAIL: u32 = 0x3333;

pub fn console_name() -> &'static str {
    "UART0 0x10000000"
}

pub fn idle() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

pub fn exit_qemu(success: bool) {
    let code = if success { FINISHER_PASS } else { FINISHER_FAIL };
    unsafe {
        core::ptr::write_volatile(TEST_FINISHER as *mut u32, code);
    }
}
