//! Ring-3 `/probe` — second static ELF64 non-PIE at 0x0240_0000.
//!
//! Own PML4. Checks bootstrap capability refusal, prints and yields so `/init` owns
//! `SYS_EXIT`. Does not touch `/init`'s window.

#![no_std]
#![no_main]

use aether_core::sysnr::{
    COW_TEMPLATE_WORD, SYS_ACCEL_SUBMIT, SYS_ACCEL_WAIT, SYS_ARENA_ALLOC, SYS_DEBUG_PRINT, SYS_MAP,
    SYS_RECV, SYS_SEND, SYS_UNMAP, SYS_YIELD, USER_COW_BASE,
};
use aether_core::{INIT_EP_CPTR, INIT_QUEUE_CPTR};

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
    // Live bootstrap endpoint/queue and user-image Memory CPtr 2 must not
    // authorize a separate root. Null buffers also prove refusal precedes copy.
    let attempts = [
        (SYS_SEND, INIT_EP_CPTR as u64, 0, 0),
        (SYS_RECV, INIT_EP_CPTR as u64, 0, 0),
        (SYS_MAP, 2, 0, 0),
        (SYS_UNMAP, 0, 0, 0),
        (SYS_ARENA_ALLOC, 4096, 0, 0),
        (SYS_ACCEL_SUBMIT, INIT_QUEUE_CPTR as u64, 0, 0),
        (SYS_ACCEL_WAIT, INIT_QUEUE_CPTR as u64, 0, 0),
    ];
    let mut refused = true;
    for (nr, a0, a1, a2) in attempts {
        refused &= sys(nr, a0, a1, a2) == -2; // SysError::NoCap
    }
    if refused {
        debug_print(b"[probe] bootstrap capabilities refused across roots\r\n");
    } else {
        debug_print(b"[probe] bootstrap capabilities FAIL\r\n");
    }
    let cow = unsafe { core::ptr::read_volatile(USER_COW_BASE as *const u64) };
    if cow == COW_TEMPLATE_WORD {
        debug_print(b"[probe] cow still template\r\n");
    } else {
        debug_print(b"[probe] cow FAIL\r\n");
    }
    for _ in 0..3 {
        debug_print(b"[probe] yield\r\n");
        let _ = sys(SYS_YIELD, 0, 0, 0);
    }
    let cow = unsafe { core::ptr::read_volatile(USER_COW_BASE as *const u64) };
    if cow == COW_TEMPLATE_WORD {
        debug_print(b"[probe] cow still template\r\n");
    } else {
        debug_print(b"[probe] cow FAIL\r\n");
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
