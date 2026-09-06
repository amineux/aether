//! Ring-3 `/probe` — second static ELF64 non-PIE at 0x0240_0000.
//!
//! Own PML4. Prints, yields, then blocks on recv so `/init` still owns
//! `SYS_EXIT`. Does not touch `/init`'s window.

#![no_std]
#![no_main]

use aether_core::sysnr::{SYS_DEBUG_PRINT, SYS_YIELD};

fn sys(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") nr => ret,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

fn debug_print(s: &[u8]) {
    let _ = sys(SYS_DEBUG_PRINT, s.as_ptr() as u64, s.len() as u64, 0);
}

#[link_section = ".text.boot"]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    debug_print(b"[probe] ring-3 /probe (static ELF64 non-PIE @ 0x2400000, own PML4)\r\n");
    for _ in 0..3 {
        debug_print(b"[probe] yield\r\n");
        let _ = sys(SYS_YIELD, 0, 0, 0);
    }
    // Stay Ready and yield. Do not SYS_EXIT (that isa-debug-exits QEMU)
    // and do not share /init's recv waiter.
    loop {
        let _ = sys(SYS_YIELD, 0, 0, 0);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    debug_print(b"[probe] PANIC\r\n");
    loop {
        unsafe {
            core::arch::asm!("pause");
        }
    }
}
