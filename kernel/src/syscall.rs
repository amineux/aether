//! Syscall ABI. Numbers 0–8 are frozen; 9 is `SYS_EXIT`; 10 is
//! `SYS_CLONE`; 11 is `SYS_MMAP`.
//!
//! User enters here through `syscall`/`sysret` (x86), `ecall`/`sret`
//! (RISC-V), or `svc`/`eret` (aarch64). Cap checks sit on
//! send / recv / map / accel before any fabric or SoftNPU work.

#![allow(dead_code)]

use aether_core::sysnr::{
    user_chunks, user_clone_pair_ok, user_mmap_ok, user_pages_ok, user_range_known, UserAccelJob,
    UserIpcMsg,
};
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
pub const SYS_CLONE: u64 = 10;
pub const SYS_MMAP: u64 = 11;

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
        #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
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

/// Visit `[ptr, ptr + len)` one 4 KiB page at a time as `(kva, off, n)`.
/// The window check runs first, then *every* page is translated in `root`
/// (present + USER, + writable for `write`) before `f` sees any of them,
/// so a refused copy reads or writes nothing. Translation is per page:
/// a straddling range never runs off the first page's frame.
fn for_user_pages(
    root: u64,
    ptr: u64,
    len: usize,
    write: bool,
    mut f: impl FnMut(u64, usize, usize),
) -> Result<(), SysError> {
    use crate::mm::paging::user_page_kva_in;
    copy_from_user(ptr, len as u64)?;
    user_pages_ok(ptr, len, |va| user_page_kva_in(root, va, write).is_some())
        .map_err(|_| SysError::Fault)?;
    for c in user_chunks(ptr, len) {
        let kva = user_page_kva_in(root, c.va, write).ok_or(SysError::Fault)?;
        f(kva, c.off, c.len);
    }
    Ok(())
}

/// Copy user bytes at `ptr` in aspace `root` (0 = live) into `out`.
pub fn read_user_in(root: u64, ptr: u64, out: &mut [u8]) -> Result<(), SysError> {
    let dst = out.as_mut_ptr();
    for_user_pages(root, ptr, out.len(), false, |kva, off, n| {
        crate::mm::paging::with_user_access(|| unsafe {
            core::ptr::copy_nonoverlapping(kva as *const u8, dst.add(off), n);
        });
    })
}

/// Copy `data` to user `ptr` in aspace `root` (0 = live). All-or-nothing:
/// a read-only or missing page anywhere in the range writes no byte.
pub fn write_user_in(root: u64, ptr: u64, data: &[u8]) -> Result<(), SysError> {
    for_user_pages(root, ptr, data.len(), true, |kva, off, n| {
        crate::mm::paging::with_user_access(|| unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr().add(off), kva as *mut u8, n);
        });
    })
}

// Byte offsets the copy helpers below (and `world::copy_ipc_out_in`) rely on.
const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(offset_of!(UserIpcMsg, flags) == 8 && offset_of!(UserIpcMsg, len) == 10);
    assert!(offset_of!(UserIpcMsg, payload) == 12 && size_of::<UserIpcMsg>() >= 76);
    assert!(offset_of!(UserAccelJob, a) == 16 && offset_of!(UserAccelJob, c) == 32);
    assert!(size_of::<UserAccelJob>() == 40);
    assert!(size_of::<aether_core::sysnr::UserCompletion>() == 12);
};

pub fn copy_user_ipc(ptr: u64) -> Result<UserIpcMsg, SysError> {
    let mut b = [0u8; core::mem::size_of::<UserIpcMsg>()];
    read_user_in(0, ptr, &mut b)?;
    let mut m = UserIpcMsg::empty();
    m.badge = u64::from_ne_bytes(b[0..8].try_into().unwrap());
    m.flags = u16::from_ne_bytes([b[8], b[9]]);
    m.len = u16::from_ne_bytes([b[10], b[11]]);
    m.payload.copy_from_slice(&b[12..76]);
    Ok(m)
}

pub fn copy_user_job(ptr: u64) -> Result<UserAccelJob, SysError> {
    let mut b = [0u8; core::mem::size_of::<UserAccelJob>()];
    read_user_in(0, ptr, &mut b)?;
    let u32_at = |o: usize| u32::from_ne_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_ne_bytes(b[o..o + 8].try_into().unwrap());
    Ok(UserAccelJob {
        op: u32_at(0),
        m: u32_at(4),
        n: u32_at(8),
        k: u32_at(12),
        a: u64_at(16),
        b: u64_at(24),
        c: u64_at(32),
    })
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
            let mut buf = [0u8; 256];
            read_user_in(0, a0, &mut buf[..len])?;
            debug_print(&buf[..len]);
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
            let len = a1.min(256) as usize;
            let mut buf = [0u8; 256];
            read_user_in(0, a0, &mut buf[..len])?;
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
        SYS_CLONE => {
            // flags must be 0 (documented subset: share aspace, no TLS).
            if a2 != 0 {
                return Err(SysError::Inval);
            }
            if !user_clone_pair_ok(a0, a1) {
                return Err(SysError::Fault);
            }
            task::clone_user(a0, a1).ok_or(SysError::Again)
        }
        SYS_MMAP => sys_mmap(a0, a1, a2),
        SYS_EXIT => {
            crate::console::write_str("[sys] exit status=");
            crate::console::write_u64(a0);
            crate::console::nl();
            #[cfg(target_arch = "x86_64")]
            outb(0xF4, a0 as u8);
            #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
            crate::arch::exit_qemu(a0 == 0);
            loop {
                unsafe {
                    #[cfg(target_arch = "x86_64")]
                    core::arch::asm!("hlt");
                    #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
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

fn mmap_window() -> (u64, u64) {
    #[cfg(target_arch = "x86_64")]
    {
        (aether_core::USER_MMAP_BASE, aether_core::USER_MMAP_END)
    }
    #[cfg(target_arch = "riscv64")]
    {
        (aether_core::USER_RV_MMAP_BASE, aether_core::USER_RV_MMAP_END)
    }
    #[cfg(target_arch = "aarch64")]
    {
        (aether_core::USER_AA_MMAP_BASE, aether_core::USER_AA_MMAP_END)
    }
}

fn mmap_first_fit(root: u64, len: u64) -> Option<u64> {
    use aether_core::sysnr::user_mmap_first_fit;
    let (lo, hi) = mmap_window();
    let mut taken = [(0u64, 0u64); 16];
    let mut n = 0usize;
    let mut p = lo;
    while p < hi && n < taken.len() {
        if crate::mm::paging::user_mapped(root, p) {
            taken[n] = (p, 0x1000);
            n += 1;
        }
        p += 0x1000;
    }
    user_mmap_first_fit(lo, hi, len, &taken[..n])
}

fn sys_mmap(addr: u64, len: u64, flags: u64) -> Result<u64, SysError> {
    if !user_mmap_ok(addr, len, flags) {
        return Err(SysError::Inval);
    }
    let Some(root) = task::current_user_root() else {
        return Err(SysError::Fault);
    };
    let (lo, hi) = mmap_window();
    let va = if addr == 0 {
        mmap_first_fit(root, len).ok_or(SysError::Again)?
    } else {
        if !aether_core::sysnr::user_range_ok_in(lo, hi, addr, len) {
            return Err(SysError::Fault);
        }
        let mut p = addr;
        while p < addr + len {
            if crate::mm::paging::user_mapped(root, p) {
                return Err(SysError::Fault);
            }
            p += 0x1000;
        }
        addr
    };
    crate::mm::paging::map_anon_pages(root, va, len)
}
