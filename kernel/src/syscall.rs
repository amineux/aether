//! Minimal syscall surface. v0.1 is invoked as kernel functions from the
//! built-in init task. The numbers are the ABI a future `syscall` gate will use.
//!
//! The kernel is a submission shim + resource solver. Compilers own the ISA;
//! there is no ML graph IR or fusion here. Host objects are PJRT/IREE-shaped
//! (Device, MemorySpace, Buffer, Executable, Event).
//!
//! ```text
//! 0 debug_print(ptr, len)
//! 1 yield()
//! 2 send(ep_cptr, msg_ptr)
//! 3 recv(ep_cptr, msg_out)
//! 4 map(mem_cptr, vaddr, flags)
//! 5 unmap(vaddr, len)
//! 6 accel_submit(queue_cptr, job_ptr)
//! 7 accel_wait(queue_cptr, completion_out)
//! 8 arena_alloc(size, flags, bank) -> mem_cptr
//! ```

#![allow(dead_code)]

use aether_core::caps::CPtr;

pub const SYS_DEBUG_PRINT: u64 = 0;
pub const SYS_YIELD: u64 = 1;
pub const SYS_SEND: u64 = 2;
pub const SYS_RECV: u64 = 3;
pub const SYS_MAP: u64 = 4;
pub const SYS_UNMAP: u64 = 5;
pub const SYS_ACCEL_SUBMIT: u64 = 6;
pub const SYS_ACCEL_WAIT: u64 = 7;
pub const SYS_ARENA_ALLOC: u64 = 8;

#[derive(Clone, Copy, Debug)]
pub enum SysError {
    Inval = 1,
    NoCap = 2,
    Fault = 3,
    Again = 4,
}

/// STUB: SYSCALL/SYSRET + user page tables. Init calls these directly.
pub fn debug_print(bytes: &[u8]) {
    for &b in bytes {
        crate::arch::serial::write_byte(b);
    }
}

pub fn yield_now() {
    // Cooperative: a real scheduler would pick the next thread.
    unsafe {
        core::arch::asm!("pause");
    }
}

pub fn dispatch(nr: u64, a0: u64, a1: u64, _a2: u64) -> Result<u64, SysError> {
    match nr {
        SYS_DEBUG_PRINT => {
            if a0 == 0 {
                return Err(SysError::Inval);
            }
            let len = a1.min(256) as usize;
            let slice = unsafe { core::slice::from_raw_parts(a0 as *const u8, len) };
            debug_print(slice);
            Ok(0)
        }
        SYS_YIELD => {
            yield_now();
            Ok(0)
        }
        SYS_SEND | SYS_RECV | SYS_MAP | SYS_UNMAP | SYS_ACCEL_SUBMIT | SYS_ACCEL_WAIT
        | SYS_ARENA_ALLOC => {
            // Wired in init.rs against fabric/caps; this branch is the ABI stub
            // for a future user trap.
            let _ = CPtr(a0 as u16);
            Err(SysError::Inval)
        }
        _ => {
            crate::console::write_str("[sys] unknown nr=");
            crate::console::write_u64(nr);
            crate::console::nl();
            Err(SysError::Inval)
        }
    }
}
