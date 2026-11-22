//! Ring-3 `/probe` — second static ELF64 non-PIE at 0x0240_0000.
//!
//! Own PML4 and private endpoint cap table. Checks bootstrap refusal and yields so `/init` owns
//! `SYS_EXIT`. Does not touch `/init`'s window.

#![no_std]
#![no_main]

use aether_core::sysnr::{
    UserIpcMsg, COW_TEMPLATE_WORD, SYS_ACCEL_SUBMIT, SYS_ACCEL_WAIT, SYS_ARENA_ALLOC,
    SYS_DEBUG_PRINT, SYS_MAP, SYS_RECV, SYS_SEND, SYS_UNMAP, SYS_YIELD, USER_COW_BASE,
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
    // CPtr 0 names a distinct private endpoint here, not /init's CPtr 0.
    // A successful round-trip checks actual table selection, badge and payload.
    let mut sent = UserIpcMsg::empty();
    sent.flags = 2;
    sent.set_payload(b"probe-private");
    let mut received = UserIpcMsg::empty();
    if sys(
        SYS_SEND,
        INIT_EP_CPTR as u64,
        core::ptr::addr_of!(sent) as u64,
        0,
    ) == 0
        && sys(
            SYS_RECV,
            INIT_EP_CPTR as u64,
            core::ptr::addr_of_mut!(received) as u64,
            0,
        ) == 0
        && received.badge == 0xB4
        && received.flags == sent.flags
        && received.payload() == sent.payload()
    {
        debug_print(b"[probe] private cap space IPC ok\r\n");
    } else {
        debug_print(b"[probe] private cap space IPC FAIL\r\n");
    }
    // Empty endpoint slot 1, queue slot 1, Memory slot 2 and allocator authority
    // are refused before copying even a null buffer; init caps are not inherited.
    let attempts = [
        (SYS_SEND, 1, 0, 0),
        (SYS_RECV, 1, 0, 0),
        (SYS_MAP, 2, 0, 0),
        (SYS_ARENA_ALLOC, 4096, 0, 0),
        (SYS_ACCEL_SUBMIT, INIT_QUEUE_CPTR as u64, 0, 0),
        (SYS_ACCEL_WAIT, INIT_QUEUE_CPTR as u64, 0, 0),
    ];
    let mut refused = true;
    for (nr, a0, a1, a2) in attempts {
        refused &= sys(nr, a0, a1, a2) == -2; // SysError::NoCap
    }
    // No foreign root may consume a tracked pin. An empty pin set returns
    // NoCap; a missing/interior/foreign pin in a live set returns Fault.
    let unmap = sys(SYS_UNMAP, 0, 0, 0);
    refused &= unmap == -2 || unmap == -3;
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
