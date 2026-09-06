//! Syscall ABI. Numbers 0–8 are frozen; 9 is `SYS_EXIT`.
//!
//! User enters here through `syscall`/`sysret` (x86) or `ecall`/`sret`
//! (RISC-V). Cap checks sit on
//! send / recv / map / accel before any fabric or SoftNPU work.

#![allow(dead_code)]

use aether_core::sysnr::{user_range_known, UserAccelJob, UserIpcMsg};
use aether_core::CPtr;

use crate::arch::idt::InterruptFrame;
#[cfg(target_arch = "x86_64")]
use crate::arch::x86_64::io::outb;
use crate::task;
use crate::world;

pub const SYS_DEBUG_PRINT: u64 = 0;
pub const SYS_YIELD: u64 = 1;
pub const SYS_SEND: u64 = 2;
pub const SYS_RECV: u64 = 3;
pub const SYS_MAP: u64 = 4;
pub const SYS_UNMAP: u64 = 5;
pub const SYS_ACCEL_SUBMIT: u64 = 6;
pub const SYS_ACCEL_WAIT: u64 = 7;
pub const SYS_ARENA_ALLOC: u64 = 8;
pub const SYS_EXIT: u64 = 9;

#[derive(Clone, Copy, Debug)]
pub enum SysError {
    Inval = 1,
    NoCap = 2,
    Fault = 3,
    Again = 4,
}

fn err_rax(e: SysError) -> u64 {
    (e as i64).wrapping_neg() as u64
}

pub fn debug_print(bytes: &[u8]) {
    for &b in bytes {
        crate::arch::serial::write_byte(b);
    }
}

pub fn yield_now() {
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!("pause");
        #[cfg(target_arch = "riscv64")]
        core::arch::asm!("nop");
    }
}

pub fn copy_from_user(ptr: u64, len: u64) -> Result<(), SysError> {
    if !user_range_known(ptr, len) {
        return Err(SysError::Fault);
    }
    Ok(())
}

pub fn copy_to_user(ptr: u64, len: u64) -> Result<(), SysError> {
    copy_from_user(ptr, len)
}

pub fn copy_user_ipc(ptr: u64) -> Result<UserIpcMsg, SysError> {
    copy_from_user(ptr, core::mem::size_of::<UserIpcMsg>() as u64)?;
    Ok(crate::mm::paging::with_user_access(|| unsafe {
        core::ptr::read_volatile(ptr as *const UserIpcMsg)
    }))
}

pub fn copy_user_job(ptr: u64) -> Result<UserAccelJob, SysError> {
    copy_from_user(ptr, core::mem::size_of::<UserAccelJob>() as u64)?;
    Ok(crate::mm::paging::with_user_access(|| unsafe {
        core::ptr::read_volatile(ptr as *const UserAccelJob)
    }))
}

pub fn from_user_trap(frame: &mut InterruptFrame) {
    task::clear_switched();
    let nr = frame.syscall_nr();
    let a0 = frame.arg0();
    let a1 = frame.arg1();
    let a2 = frame.arg2();
    let r = dispatch_trap(nr, a0, a1, a2, frame);
    if task::took_switch() {
        return;
    }
    frame.set_ret(match r {
        Ok(v) => v,
        Err(e) => err_rax(e),
    });
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
            let _ = CPtr(a0 as u16);
            Err(SysError::Inval)
        }
        _ => Err(SysError::Inval),
    }
}

fn dispatch_trap(
    nr: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    frame: &mut InterruptFrame,
) -> Result<u64, SysError> {
    match nr {
        SYS_DEBUG_PRINT => {
            copy_from_user(a0, a1.min(256))?;
            let len = a1.min(256) as usize;
            let mut buf = [0u8; 256];
            crate::mm::paging::with_user_access(|| unsafe {
                core::ptr::copy_nonoverlapping(a0 as *const u8, buf.as_mut_ptr(), len);
            });
            debug_print(&buf[..len]);
            Ok(0)
        }
        SYS_YIELD => {
            frame.set_ret(0);
            task::resched_from_trap(frame, None, 0);
            Ok(0)
        }
        SYS_SEND => world::sys_send(a0, a1),
        SYS_RECV => world::sys_recv(a0, a1, frame),
        SYS_MAP => world::sys_map(a0, a1, a2),
        SYS_UNMAP => world::sys_unmap(a0, a1),
        SYS_ACCEL_SUBMIT => world::sys_accel_submit(a0, a1),
        SYS_ACCEL_WAIT => world::sys_accel_wait(a0, a1, frame),
        SYS_ARENA_ALLOC => world::sys_arena_alloc(a0, a1, a2),
        SYS_EXIT => {
            crate::console::write_str("[sys] exit status=");
            crate::console::write_u64(a0);
            crate::console::nl();
            #[cfg(target_arch = "x86_64")]
            outb(0xF4, a0 as u8);
            #[cfg(target_arch = "riscv64")]
            crate::arch::exit_qemu(a0 == 0);
            loop {
                unsafe {
                    #[cfg(target_arch = "x86_64")]
                    core::arch::asm!("hlt");
                    #[cfg(target_arch = "riscv64")]
                    core::arch::asm!("wfi");
                }
            }
        }
        _ => {
            crate::console::write_str("[sys] unknown nr=");
            crate::console::write_u64(nr);
            crate::console::nl();
            Err(SysError::Inval)
        }
    }
}
