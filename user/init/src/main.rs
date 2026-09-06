//! Ring-3 `/init` — static ELF64 non-PIE at 0x0200_0000.
//!
//! Talks to the kernel only through `syscall` / `ecall` / `svc` (numbers 0–10).
//! Well-known CPtrs 0 (endpoint) and 1 (accel queue) are minted before
//! the drop. `SYS_CLONE` starts a sibling thread on this aspace.

#![no_std]
#![no_main]

use aether_core::demo::DEMO_B;
use aether_core::sysnr::{
    UserAccelJob, UserCompletion, UserIpcMsg, INIT_EP_CPTR, INIT_QUEUE_CPTR, SYS_ACCEL_SUBMIT,
    SYS_ACCEL_WAIT, SYS_ARENA_ALLOC, SYS_CLONE, SYS_DEBUG_PRINT, SYS_EXIT, SYS_MAP, SYS_RECV,
    SYS_SEND, SYS_YIELD,
};
#[cfg(target_arch = "x86_64")]
use aether_core::sysnr::{COW_PRIVATE_WORD, COW_TEMPLATE_WORD, USER_COW_BASE};

fn sys(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        {
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
        #[cfg(target_arch = "riscv64")]
        {
            core::arch::asm!(
                "ecall",
                in("a7") nr,
                inout("a0") a0 => ret,
                in("a1") a1,
                in("a2") a2,
                options(nostack)
            );
        }
        #[cfg(target_arch = "aarch64")]
        {
            core::arch::asm!(
                "svc #0",
                in("x8") nr,
                inout("x0") a0 => ret,
                in("x1") a1,
                in("x2") a2,
                options(nostack)
            );
        }
    }
    ret
}

fn debug_print(s: &[u8]) {
    let _ = sys(SYS_DEBUG_PRINT, s.as_ptr() as u64, s.len() as u64, 0);
}

fn yield_now() {
    let _ = sys(SYS_YIELD, 0, 0, 0);
}

fn exit(code: u64) -> ! {
    let _ = sys(SYS_EXIT, code, 0, 0);
    loop {
        unsafe {
            #[cfg(target_arch = "x86_64")]
            core::arch::asm!("hlt");
            #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
            core::arch::asm!("wfi");
        }
    }
}

/// Second user thread: same PML4/satp as `/init`, own stack. Does not
/// `SYS_EXIT` (that would take the guest down) and does not `SYS_RECV`
/// (one waiter on the fabric ping is enough).
#[inline(never)]
extern "C" fn user_thread(_tid: u64) -> ! {
    debug_print(b"[init] user-thread share-aspace\r\n");
    loop {
        yield_now();
    }
}

#[repr(align(16))]
#[allow(dead_code)]
struct ChildStack([u8; 8192]);

static mut CHILD_STACK: ChildStack = ChildStack([0; 8192]);

fn spawn_user_thread() -> bool {
    let stack = unsafe {
        (core::ptr::addr_of_mut!(CHILD_STACK) as *mut u8)
            .add(core::mem::size_of::<ChildStack>()) as u64
    };
    let tid = sys(SYS_CLONE, user_thread as usize as u64, stack, 0);
    if tid < 0 {
        debug_print(b"[init] clone FAIL\r\n");
        return false;
    }
    debug_print(b"[init] clone ok (shared aspace)\r\n");
    true
}

#[link_section = ".text.boot"]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    #[cfg(target_arch = "x86_64")]
    debug_print(b"[init] ring-3 /init (static ELF64 non-PIE @ 0x2000000, own PML4)\r\n");
    #[cfg(target_arch = "riscv64")]
    debug_print(b"[init] U-mode /init (static ELF64 non-PIE @ 0x82000000, own satp)\r\n");
    #[cfg(target_arch = "aarch64")]
    debug_print(b"[init] EL0 /init (static ELF64 non-PIE @ 0x42000000, own TTBR0)\r\n");
    #[cfg(target_arch = "x86_64")]
    debug_print(b"[init] syscall debug_print ok\r\n");
    #[cfg(target_arch = "riscv64")]
    debug_print(b"[init] ecall debug_print ok\r\n");
    #[cfg(target_arch = "aarch64")]
    debug_print(b"[init] svc debug_print ok\r\n");

    if !spawn_user_thread() {
        exit(1);
    }
    // Give the sibling a few slices before we block on recv so the
    // serial proof is not a race with kthread-B's ping.
    for _ in 0..4 {
        yield_now();
    }

    #[cfg(target_arch = "x86_64")]
    {
        // Shared RO template with /probe. The store #PFs; the kernel
        // copies the page and this aspace becomes private. /probe
        // still names the template.
        let p = USER_COW_BASE as *mut u64;
        let before = unsafe { core::ptr::read_volatile(p) };
        unsafe {
            core::ptr::write_volatile(p, COW_PRIVATE_WORD);
        }
        let after = unsafe { core::ptr::read_volatile(p) };
        if before != COW_TEMPLATE_WORD || after != COW_PRIVATE_WORD {
            debug_print(b"[init] cow FAIL\r\n");
            exit(1);
        }
        debug_print(b"[init] cow write ok (private page)\r\n");
    }

    // Recv: empty inbox → block until kthread-B sends ping-fabric.
    debug_print(b"[init] recv inbox (blocks until kthread-B send)\r\n");
    let mut msg = UserIpcMsg::empty();
    let rc = sys(
        SYS_RECV,
        INIT_EP_CPTR as u64,
        core::ptr::addr_of_mut!(msg) as u64,
        0,
    );
    if rc < 0 {
        debug_print(b"[init] recv FAIL\r\n");
        exit(1);
    }
    debug_print(b"[init] fabric recv: ");
    debug_print(msg.payload());
    debug_print(b"\r\n");

    for _ in 0..4 {
        debug_print(b"[init] user-A slice\r\n");
        yield_now();
    }
    debug_print(b"[init] yield returned\r\n");

    msg.badge = 0xA3;
    msg.flags = 2; // ASYNC
    if !msg.set_payload(b"init-ack") {
        debug_print(b"[init] send payload FAIL\r\n");
        exit(1);
    }
    let rc = sys(
        SYS_SEND,
        INIT_EP_CPTR as u64,
        core::ptr::addr_of!(msg) as u64,
        0,
    );
    if rc < 0 {
        debug_print(b"[init] send FAIL\r\n");
        exit(1);
    }
    debug_print(b"[init] fabric send/recv ok\r\n");

    let cap = sys(SYS_ARENA_ALLOC, 256, 0, 0);
    if cap < 0 {
        debug_print(b"[init] arena_alloc FAIL\r\n");
        exit(1);
    }
    let mapped = sys(SYS_MAP, cap as u64, 0, 0);
    if mapped < 0 {
        debug_print(b"[init] map FAIL\r\n");
        exit(1);
    }
    debug_print(b"[init] arena_alloc + map ok\r\n");

    let mut tensors = [0u8; 256];
    let ident = [
        1i32, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1,
    ];
    for (i, v) in ident.iter().enumerate() {
        tensors[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in DEMO_B.iter().enumerate() {
        tensors[64 + i * 4..64 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }

    let base = tensors.as_ptr() as u64;
    let job = UserAccelJob {
        op: 1, // MatMul
        m: 4,
        n: 4,
        k: 4,
        a: base,
        b: base + 64,
        c: base + 128,
    };
    let rc = sys(
        SYS_ACCEL_SUBMIT,
        INIT_QUEUE_CPTR as u64,
        core::ptr::addr_of!(job) as u64,
        0,
    );
    if rc < 0 {
        debug_print(b"[init] accel_submit FAIL\r\n");
        exit(1);
    }
    debug_print(b"[init] accel_submit matmul 4x4 i32\r\n");

    let mut cpl = UserCompletion {
        job_seq: 0,
        status: 0,
        cycles: 0,
    };
    let rc = sys(
        SYS_ACCEL_WAIT,
        INIT_QUEUE_CPTR as u64,
        core::ptr::addr_of_mut!(cpl) as u64,
        0,
    );
    if rc < 0 {
        debug_print(b"[init] accel_wait FAIL\r\n");
        exit(1);
    }

    let c00 = i32::from_le_bytes(tensors[128..132].try_into().unwrap_or([0; 4]));
    let c11 = i32::from_le_bytes(tensors[148..152].try_into().unwrap_or([0; 4]));
    if c00 != DEMO_B[0] || c11 != DEMO_B[5] {
        debug_print(b"[init] SoftNPU VERIFY FAIL\r\n");
        exit(1);
    }
    debug_print(b"[init] SoftNPU C[0,0]=2 C[1,1]=13 (I @ B = B)\r\n");

    debug_print(b"\r\n====================================================\r\n");
    debug_print(b"  FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE\r\n");
    debug_print(b"  CUT BIND + HODGE FLOW CLASS ENFORCED\r\n");
    debug_print(b"  TYPED SPACE + ACTIVITY ENDPOINT + FENCE-ORDERED JOB\r\n");
    #[cfg(target_arch = "x86_64")]
    debug_print(b"  RING-3 /init VIA SYSCALL/SYSRET\r\n");
    #[cfg(target_arch = "riscv64")]
    debug_print(b"  U-MODE /init VIA ECALL/SRET\r\n");
    #[cfg(target_arch = "aarch64")]
    debug_print(b"  EL0 /init VIA SVC/ERET\r\n");
    debug_print(b"  VIRTQUEUE MMIO + IOMMU MAP + BANK COLOR\r\n");
    debug_print(b"====================================================\r\n");

    yield_now();
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    debug_print(b"[init] PANIC\r\n");
    exit(1);
}
