//! Aether kernel entry. x86_64 / RISC-V / aarch64 load `/init` and drop
//! to user (ring-3 / U-mode / EL0).

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

#[cfg(target_arch = "x86_64")]
// Fabric + two CapTables live on this stack in run_boot_demo.
// parent edges grew Capability; 64 KiB (and 128 KiB on GH rustc) overflowed.
const KERNEL_STACK_SIZE: usize = 256 * 1024;
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
        arch::x86_64::kpti::init();
    }

    arch::timer::init();

    {
        #[cfg(target_arch = "x86_64")]
        arch::irq::smp_start_aps();
        task::init();
        world::init();
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::console::write_str("[boot] PIT 100 Hz; ncpus=");
        crate::console::write_u64(arch::irq::ncpus() as u64);
        crate::console::write_str(" (APs kernel-only; per-task PML4 on BSP user tasks)");
        crate::console::nl();
        crate::mm::paging::enable_smep_smap();
        crate::mm::paging::enable_pcid();
    }
    #[cfg(target_arch = "riscv64")]
    println!("[boot] UP timer + PLIC armed (100 Hz); extra harts parked; U-mode /init");
    #[cfg(target_arch = "aarch64")]
    println!("[boot] UP timer armed (100 Hz); extra PEs parked; EL0 /init");
    nl();

    init::run_kernel_selfcheck();

    match elfload::mount_boot_ramfs().and_then(|fs| {
        elfload::load_init(&fs).map(|init| {
            #[cfg(target_arch = "x86_64")]
            let probe = elfload::load_probe(&fs).ok();
            #[cfg(not(target_arch = "x86_64"))]
            let probe: Option<elfload::LoadedImage> = None;
            (init, probe)
        })
    }) {
        Ok((init, probe)) => {
            let probe_cr3 = probe.as_ref().map(|p| p.cr3);
            #[cfg(target_arch = "x86_64")]
            if !crate::mm::paging::install_shared_cow(init.cr3, probe_cr3) {
                write_str("[boot] COW map failed");
                crate::console::nl();
                arch::exit_qemu(false);
                arch::idle();
            }
            crate::mm::paging::prove_aspace(init.cr3, probe_cr3);
            task::spawn_kthread();
            task::spawn_user(init.entry, init.cr3);
            #[cfg(target_arch = "x86_64")]
            if let Some(p) = probe {
                task::spawn_user_task(
                    task::TID_PROBE,
                    p.entry,
                    aether_core::USER_PROBE_STACK_TOP,
                    p.cr3,
                );
            }
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

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("KERNEL PANIC\r\n");
    let _ = write_u64;
    arch::idle();
}
