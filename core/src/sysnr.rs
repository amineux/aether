//! Syscall numbers and the ring-3 C ABI.
//!
//! Numbers 0–8 are frozen (kernel/src/syscall.rs). Additive slots:
//! `SYS_EXIT` (9) and `SYS_CLONE` (10).

/// Userspace message blob for `SYS_SEND` / `SYS_RECV`.
/// Layout is the contract; the kernel copies it across the user/kernel cut.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserIpcMsg {
    pub badge: u64,
    pub flags: u16,
    pub len: u16,
    pub payload: [u8; 64],
}

impl UserIpcMsg {
    pub const fn empty() -> Self {
        Self {
            badge: 0,
            flags: 0,
            len: 0,
            payload: [0; 64],
        }
    }

    pub fn set_payload(&mut self, data: &[u8]) -> bool {
        if data.len() > self.payload.len() {
            return false;
        }
        self.payload[..data.len()].copy_from_slice(data);
        self.len = data.len() as u16;
        true
    }

    pub fn payload(&self) -> &[u8] {
        let n = (self.len as usize).min(self.payload.len());
        &self.payload[..n]
    }
}

/// Compact accel job the kernel copies from ring-3 (`SYS_ACCEL_SUBMIT`).
/// Addresses are user VAs; the kernel identity-maps the init image.
///
/// No `dtype` field: this wire is unchanged. `/init` submits I32.
/// F16/F32 live on [`crate::accel::AccelJobDesc`] / `CpCmd` (additive
/// values 1 and 2). Do not reshape this struct without an ACCEL.md bump.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserAccelJob {
    pub op: u32,
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub a: u64,
    pub b: u64,
    pub c: u64,
}

/// SoftNPU completion written by `SYS_ACCEL_WAIT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserCompletion {
    pub job_seq: u32,
    pub status: i32,
    pub cycles: u32,
}

/// Well-known CPtrs minted into `/init` before the ring-3 drop.
pub const INIT_EP_CPTR: u16 = 0;
pub const INIT_QUEUE_CPTR: u16 = 1;

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
/// Spawn a user thread that shares the caller's aspace (PML4 / satp).
///
/// `clone(entry, stack, flags) → tid`. `flags` must be 0. The child
/// does not return from clone: it starts at `entry` with arg0 = tid
/// and `rsp`/`sp` = `stack`. Not Linux `clone`, not `fork`, no new
/// address space, no TLS, no files.
pub const SYS_CLONE: u64 = 10;

/// Static non-PIE `/init` link address (identity-mapped, USER pages).
pub const USER_IMAGE_BASE: u64 = 0x0200_0000;
/// Exclusive end of the 2 MiB user window (image + stack).
pub const USER_IMAGE_END: u64 = 0x0220_0000;
pub const USER_STACK_TOP: u64 = USER_IMAGE_END;

/// Optional second static ELF (`/probe`), own 2 MiB window + own PML4.
pub const USER_PROBE_BASE: u64 = 0x0240_0000;
pub const USER_PROBE_END: u64 = 0x0260_0000;
pub const USER_PROBE_STACK_TOP: u64 = USER_PROBE_END;

/// RISC-V `/init` window. QEMU virt RAM starts at `0x8000_0000`; the
/// x86 `0x0200_0000` hole is not RAM. Identity-mapped 2 MiB, U-bit
/// only on this leaf in the task satp. Not a second ABI.
pub const USER_RV_IMAGE_BASE: u64 = 0x8200_0000;
pub const USER_RV_IMAGE_END: u64 = 0x8220_0000;
pub const USER_RV_STACK_TOP: u64 = USER_RV_IMAGE_END;

pub fn user_range_ok_in(lo: u64, hi: u64, ptr: u64, len: u64) -> bool {
    if ptr < lo {
        return false;
    }
    match ptr.checked_add(len) {
        Some(end) => end <= hi,
        None => false,
    }
}

pub fn user_range_ok(ptr: u64, len: u64) -> bool {
    user_range_ok_in(USER_IMAGE_BASE, USER_IMAGE_END, ptr, len)
}

/// Software range check across known user windows. Hardware USER leaves
/// are still task-local; a VA that passes this but is unmapped in the
/// current PML4 will #PF.
pub fn user_range_known(ptr: u64, len: u64) -> bool {
    user_range_ok(ptr, len)
        || user_range_ok_in(USER_PROBE_BASE, USER_PROBE_END, ptr, len)
        || user_range_ok_in(USER_RV_IMAGE_BASE, USER_RV_IMAGE_END, ptr, len)
}

const USER_WINDOWS: [(u64, u64); 3] = [
    (USER_IMAGE_BASE, USER_IMAGE_END),
    (USER_PROBE_BASE, USER_PROBE_END),
    (USER_RV_IMAGE_BASE, USER_RV_IMAGE_END),
];

/// `SYS_CLONE` entry and stack-top must sit in the same known user window.
/// Stack-top may equal the exclusive window end (grows down).
pub fn user_clone_pair_ok(entry: u64, stack: u64) -> bool {
    if entry == 0 || stack == 0 || stack & 0xF != 0 {
        return false;
    }
    USER_WINDOWS.iter().any(|&(lo, hi)| {
        user_range_ok_in(lo, hi, entry, 1) && stack > lo && stack <= hi
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_match_historical_abi() {
        assert_eq!(SYS_DEBUG_PRINT, 0);
        assert_eq!(SYS_YIELD, 1);
        assert_eq!(SYS_SEND, 2);
        assert_eq!(SYS_RECV, 3);
        assert_eq!(SYS_MAP, 4);
        assert_eq!(SYS_UNMAP, 5);
        assert_eq!(SYS_ACCEL_SUBMIT, 6);
        assert_eq!(SYS_ACCEL_WAIT, 7);
        assert_eq!(SYS_ARENA_ALLOC, 8);
        assert_eq!(SYS_EXIT, 9);
        assert_eq!(SYS_CLONE, 10);
    }

    #[test]
    fn user_window() {
        assert!(user_range_ok(USER_IMAGE_BASE, 16));
        assert!(!user_range_ok(USER_IMAGE_BASE - 1, 1));
        assert!(!user_range_ok(USER_IMAGE_END - 8, 16));
        assert!(user_range_ok(USER_IMAGE_END - 8, 8));
        assert!(user_range_known(USER_PROBE_BASE, 16));
        assert!(!user_range_ok(USER_PROBE_BASE, 16));
        assert!(!user_range_known(USER_IMAGE_END, 1));
        assert!(user_range_known(USER_RV_IMAGE_BASE, 16));
        assert!(!user_range_ok(USER_RV_IMAGE_BASE, 16));
        assert!(!user_range_known(USER_RV_IMAGE_END, 1));
    }

    #[test]
    fn clone_pair_same_window() {
        assert!(user_clone_pair_ok(USER_IMAGE_BASE + 0x100, USER_STACK_TOP));
        assert!(user_clone_pair_ok(
            USER_RV_IMAGE_BASE + 0x100,
            USER_RV_STACK_TOP
        ));
        assert!(!user_clone_pair_ok(USER_IMAGE_BASE + 0x100, USER_PROBE_STACK_TOP));
        assert!(!user_clone_pair_ok(USER_IMAGE_BASE, 0));
        assert!(!user_clone_pair_ok(0, USER_STACK_TOP));
        assert!(!user_clone_pair_ok(USER_IMAGE_BASE, USER_STACK_TOP - 1));
    }

    #[test]
    fn ipc_payload_roundtrip() {
        let mut m = UserIpcMsg::empty();
        assert!(m.set_payload(b"ping-fabric"));
        assert_eq!(m.payload(), b"ping-fabric");
        assert!(!m.set_payload(&[0u8; 65]));
    }
}
