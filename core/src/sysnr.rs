//! Syscall numbers and the ring-3 C ABI.
//!
//! Numbers 0–8 are frozen (kernel/src/syscall.rs). `SYS_EXIT` is the one
//! additive slot so `/init` can ask the kernel to isa-debug-exit QEMU.

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

/// Static non-PIE `/init` link address (identity-mapped, USER pages).
pub const USER_IMAGE_BASE: u64 = 0x0200_0000;
/// Exclusive end of the 2 MiB user window (image + stack).
pub const USER_IMAGE_END: u64 = 0x0220_0000;
pub const USER_STACK_TOP: u64 = USER_IMAGE_END;

/// Optional second static ELF (`/probe`), own 2 MiB window + own PML4.
pub const USER_PROBE_BASE: u64 = 0x0240_0000;
pub const USER_PROBE_END: u64 = 0x0260_0000;
pub const USER_PROBE_STACK_TOP: u64 = USER_PROBE_END;

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
    }

    #[test]
    fn ipc_payload_roundtrip() {
        let mut m = UserIpcMsg::empty();
        assert!(m.set_payload(b"ping-fabric"));
        assert_eq!(m.payload(), b"ping-fabric");
        assert!(!m.set_payload(&[0u8; 65]));
    }
}
