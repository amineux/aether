//! RISC-V S-mode HAL for QEMU virt (OpenSBI).
//!
//! S-mode kernel + U-mode `/init` via `sret` / `ecall`. Sv39 task
//! isolate (U-bit on one 2 MiB window). SoftNPU is the in-kernel
//! virtqueue (path B). Completions arrive on a PLIC software doorbell
//! (UART THRE → source 10), not a virtio-mmio `-device`. Extra harts
//! stay parked.

use core::arch::global_asm;

pub mod idt;
pub mod plic;
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
