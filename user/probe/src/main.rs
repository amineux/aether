//! Ring-3 `/probe` — second static ELF64 non-PIE at 0x0240_0000.
//!
//! Own PML4 and private endpoint cap table. Checks bootstrap refusal and yields so `/init` owns
//! `SYS_EXIT`. Does not touch `/init`'s window.

#![no_std]
#![no_main]

use aether_core::sysnr::{
    UserIpcMsg, COW_TEMPLATE_WORD, SYS_ACCEL_SUBMIT, SYS_ACCEL_WAIT, SYS_ARENA_ALLOC,
    SYS_CAP_GRANT, SYS_CAP_REVOKE, SYS_DEBUG_PRINT, SYS_MAP, SYS_RECV, SYS_SEND, SYS_UNMAP,
    SYS_YIELD, USER_COW_BASE,
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
        (SYS_CAP_REVOKE, 0, 0, 0),
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
    // Boot protocol: poll slot 1 until /init explicitly copies WRITE on its
    // endpoint. No shared memory or guessed raw endpoint id is involved.
    let mut grant_phase = 0;
    loop {
        if grant_phase == 0 {
            let mut message = UserIpcMsg::empty();
            message.flags = 2;
            let limited = sys(SYS_RECV, 1, 0, 0) == -2
                && sys(SYS_CAP_GRANT, 1, 2, 2) == -2
                && sys(SYS_MAP, 1, 0, 0) == -2
                && sys(SYS_ACCEL_SUBMIT, 1, 0, 0) == -2;
            message.set_payload(if limited {
                b"probe-write-only"
            } else {
                b"grant-rights-FAIL"
            });
            let result = sys(SYS_SEND, 1, core::ptr::addr_of!(message) as u64, 0);
            if result == 0 {
                grant_phase = 1;
            } else if result != -2 {
                debug_print(b"[probe] endpoint grant send FAIL\r\n");
            }
        }
        if grant_phase == 1 && sys(SYS_RECV, 1, 0, 0) == -3 {
            // READ now exists. This valid buffer must remain untouched when
            // the blocked syscall is canceled by capability revocation.
            let mut blocked = UserIpcMsg::empty();
            blocked.badge = 0xCAFE;
            if sys(SYS_RECV, 1, core::ptr::addr_of_mut!(blocked) as u64, 0) == -2
                && blocked.badge == 0xCAFE
                && blocked.len == 0
            {
                debug_print(b"[probe] revoked blocked receive returns NoCap\r\n");
                grant_phase = 2;
            } else {
                debug_print(b"[probe] revoked blocked receive FAIL\r\n");
                grant_phase = 255;
            }
        }
        if grant_phase == 2 {
            // The unrelated private cap must survive. A fresh WRITE grant can
            // reuse slot 1, but still cannot authorize READ or onward GRANT.
            let mut private = UserIpcMsg::empty();
            private.set_payload(b"private-survived");
            let mut private_out = UserIpcMsg::empty();
            let intact = sys(SYS_SEND, 0, core::ptr::addr_of!(private) as u64, 0) == 0
                && sys(SYS_RECV, 0, core::ptr::addr_of_mut!(private_out) as u64, 0) == 0
                && private_out.badge == 0xB4
                && private_out.payload() == private.payload();
            let limited = sys(SYS_RECV, 1, 0, 0) == -2
                && sys(SYS_CAP_GRANT, 1, 2, 2) == -2
                && sys(SYS_CAP_REVOKE, 1, 0, 0) == -2;
            let mut message = UserIpcMsg::empty();
            message.set_payload(if intact && limited {
                b"probe-after-revoke"
            } else {
                b"revoke-FAIL"
            });
            if sys(SYS_SEND, 1, core::ptr::addr_of!(message) as u64, 0) == 0 {
                grant_phase = 3;
            }
        }
        // Stay Ready; /init retains SYS_EXIT and is the only shared receiver.
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
