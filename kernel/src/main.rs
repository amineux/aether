//! Aether kernel entry. x86_64 loads `/init` and drops to ring-3.
//! RISC-V is a thin S-mode port: self-check + serial, no `sret`.

#![no_std]
#![no_main]

mod arch;
mod console;
#[cfg(target_arch = "x86_64")]
mod elfload;
mod init;
mod mm;
mod sync;
#[cfg(target_arch = "x86_64")]
mod syscall;
#[cfg(target_arch = "x86_64")]
mod task;
#[cfg(target_arch = "x86_64")]
mod world;

use core::panic::PanicInfo;

use aether_core::{NAME, VERSION};

use crate::console::{nl, write_str, write_u64};

#[cfg(target_arch = "x86_64")]
const KERNEL_STACK_SIZE: usize = 64 * 1024;
#[cfg(target_arch = "x86_64")]
#[repr(align(16))]
struct Stack([u8; KERNEL_STACK_SIZE]);
#[cfg(target_arch = "x86_64")]
static mut KERNEL_STACK: Stack = Stack([0; KERNEL_STACK_SIZE]);

/// x86_64: trampoline jumps here after long mode. Sets RSP and calls kmain.
#[cfg(target_arch = "x86_64")]
#[link_section = ".text.boot"]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        core::arch::asm!(
            "lea rsp, [{stack} + {size}]",
            "and rsp, -16",
            "call {kmain}",
            "2: hlt; jmp 2b",
            stack = sym KERNEL_STACK,
            size = const KERNEL_STACK_SIZE,
            kmain = sym kmain,
            options(noreturn)
        );
    }
}

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    arch::serial::init();
    write_str("\r\n====================================================\r\n");
    write_str("  ");
    write_str(NAME);
    write_str(" v");
    write_str(VERSION);
    write_str("  -- accelerator-first fabric kernel\r\n");
    write_str("====================================================\r\n");
    write_str("[boot] serial console online (");
    write_str(arch::console_name());
    write_str(")\r\n");

    mm::init();
    arch::idt::init();

    #[cfg(target_arch = "x86_64")]
    {
        arch::gdt::init();
        arch::syscall::init();
    }

    arch::timer::init();

    #[cfg(target_arch = "x86_64")]
    {
        arch::irq::smp_start_aps();
        task::init();
        world::init();
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::console::write_str("[boot] PIT 100 Hz; ncpus=");
        crate::console::write_u64(arch::irq::ncpus() as u64);
        crate::console::write_str(" (APs kernel-only; no per-task PML4)");
        crate::console::nl();
    }
    #[cfg(not(target_arch = "x86_64"))]
    println!("[boot] UP timer armed (100 Hz); RISC-V extra harts stay parked");
    nl();

    init::run_kernel_selfcheck();

    #[cfg(target_arch = "x86_64")]
    {
        match elfload::load() {
            Ok(entry) => {
                task::spawn_kthread();
                task::spawn_user(entry);
                task::enter_user();
            }
            Err(e) => {
                write_str("[boot] ELF load failed: ");
                write_str(e);
                crate::console::nl();
                arch::exit_qemu(false);
                arch::idle();
            }
        }
    }

    #[cfg(target_arch = "riscv64")]
    {
        // Thin port: no sret / ELF /init. The self-check *is* the demo.
        nl();
        println!("====================================================");
        println!("  FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE");
        println!("  CUT BIND + HODGE FLOW CLASS ENFORCED");
        println!("  TYPED SPACE + ACTIVITY ENDPOINT + FENCE-ORDERED JOB");
        println!("  RISC-V v0.1: kmain + serial (no ring-3)");
        println!("====================================================");
        arch::exit_qemu(true);
        println!("Aether idle. (research prototype -- halt loop)");
        arch::idle();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("KERNEL PANIC\r\n");
    let _ = write_u64;
    arch::idle();
}
