//! Aether kernel entry. Loads `/init` and drops to ring-3.

#![no_std]
#![no_main]

mod arch;
mod console;
mod elfload;
mod init;
mod mm;
mod sync;
mod syscall;
mod task;
mod world;

use core::panic::PanicInfo;

use aether_core::{NAME, VERSION};

use crate::console::{nl, write_str, write_u64};

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

const KERNEL_STACK_SIZE: usize = 64 * 1024;
#[repr(align(16))]
struct Stack([u8; KERNEL_STACK_SIZE]);
static mut KERNEL_STACK: Stack = Stack([0; KERNEL_STACK_SIZE]);

fn kmain() -> ! {
    arch::serial::init();
    write_str("\r\n====================================================\r\n");
    write_str("  ");
    write_str(NAME);
    write_str(" v");
    write_str(VERSION);
    write_str("  -- accelerator-first fabric kernel\r\n");
    write_str("====================================================\r\n");
    println!("[boot] serial console online (COM1 115200)");

    mm::init();
    arch::idt::init();
    arch::gdt::init();
    arch::syscall::init();
    arch::timer::init();
    task::init();
    world::init();

    println!("[boot] UP timer armed (100 Hz); SMP AP bring-up is STUB");
    nl();

    init::run_kernel_selfcheck();

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
            crate::arch::x86_64::io::outb(0xF4, 0x01);
            loop {
                unsafe {
                    core::arch::asm!("hlt");
                }
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("KERNEL PANIC\r\n");
    let _ = write_u64;
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}
