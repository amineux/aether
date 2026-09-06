//! CPU threads, PIT preemption, and blocking wait.
//!
//! Two runnables after boot: ring-3 `/init` and kernel `kthread-B`.
//! Switching copies an [`InterruptFrame`] so IRQ and syscall share one path.

use aether_core::preempt::{CpuQueue, WaitWhy};
#[cfg(target_arch = "x86_64")]
use aether_core::{USER_IMAGE_BASE, USER_STACK_TOP};
#[cfg(target_arch = "riscv64")]
use aether_core::{USER_IMAGE_BASE, USER_RV_STACK_TOP};

use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_arch = "x86_64")]
use crate::arch::gdt::{self, KCODE, KDATA, USER_CS, USER_DS};
use crate::arch::idt::InterruptFrame;
use crate::arch::irq;
#[cfg(target_arch = "x86_64")]
use crate::arch::x86_64::syscall as sc;
use crate::console::{self, write_str, write_u64};
use crate::println;

static SWITCHED: AtomicBool = AtomicBool::new(false);

pub fn clear_switched() {
    SWITCHED.store(false, Ordering::Relaxed);
}

pub fn took_switch() -> bool {
    SWITCHED.load(Ordering::Relaxed)
}

fn mark_switched() {
    SWITCHED.store(true, Ordering::Relaxed);
}

pub const TID_KTHREAD: u32 = 1;
pub const TID_USER: u32 = 2;
pub const TID_PROBE: u32 = 3;
const KSTACK_SIZE: usize = 16 * 1024;
const MAX: usize = 4;

#[repr(align(16))]
struct KStack([u8; KSTACK_SIZE]);

struct Thread {
    saved: InterruptFrame,
    kstack: KStack,
    kstack_top: u64,
    user_buf: u64,
    cr3: u64,
    used: bool,
}

struct Tasks {
    queue: CpuQueue,
    threads: [Thread; MAX],
    current: u32,
    started: bool,
    ping_sent: bool,
}

#[cfg(target_arch = "x86_64")]
const EMPTY_FRAME: InterruptFrame = InterruptFrame {
    r15: 0,
    r14: 0,
    r13: 0,
    r12: 0,
    r11: 0,
    r10: 0,
    r9: 0,
    r8: 0,
    rdi: 0,
    rsi: 0,
    rbp: 0,
    rbx: 0,
    rdx: 0,
    rcx: 0,
    rax: 0,
    vec: 0,
    err: 0,
    rip: 0,
    cs: 0,
    rflags: 0,
    rsp: 0,
    ss: 0,
};
#[cfg(target_arch = "riscv64")]
const EMPTY_FRAME: InterruptFrame = InterruptFrame {
    regs: [0; 31],
    sepc: 0,
    scause: 0,
    sstatus: 0,
};

fn empty_thread() -> Thread {
    Thread {
        saved: EMPTY_FRAME,
        kstack: KStack([0; KSTACK_SIZE]),
        kstack_top: 0,
        user_buf: 0,
        cr3: 0,
        used: false,
    }
}

static mut TASKS: Tasks = Tasks {
    queue: CpuQueue::new(),
    threads: [
        Thread {
            saved: EMPTY_FRAME,
            kstack: KStack([0; KSTACK_SIZE]),
            kstack_top: 0,
            user_buf: 0,
            cr3: 0,
            used: false,
        },
        Thread {
            saved: EMPTY_FRAME,
            kstack: KStack([0; KSTACK_SIZE]),
            kstack_top: 0,
            user_buf: 0,
            cr3: 0,
            used: false,
        },
        Thread {
            saved: EMPTY_FRAME,
            kstack: KStack([0; KSTACK_SIZE]),
            kstack_top: 0,
            user_buf: 0,
            cr3: 0,
            used: false,
        },
        Thread {
            saved: EMPTY_FRAME,
            kstack: KStack([0; KSTACK_SIZE]),
            kstack_top: 0,
            user_buf: 0,
            cr3: 0,
            used: false,
        },
    ],
    current: 0,
    started: false,
    ping_sent: false,
};

fn tasks() -> &'static mut Tasks {
    unsafe { &mut *core::ptr::addr_of_mut!(TASKS) }
}

fn slot_index(id: u32) -> usize {
    id as usize
}

fn kstack_top(t: &Thread) -> u64 {
    let p = t.kstack.0.as_ptr() as u64 + KSTACK_SIZE as u64;
    p & !0xF
}

fn apply_hw(t: &Tasks, from: u32, id: u32) {
    let th = &t.threads[slot_index(id)];
    #[cfg(target_arch = "x86_64")]
    {
        gdt::set_rsp0(th.kstack_top);
        sc::set_kstack(th.kstack_top);
    }
    #[cfg(target_arch = "riscv64")]
    {
        let user = th.saved.sstatus & crate::arch::riscv64::idt::SSTATUS_SPP == 0;
        crate::mm::paging::write_sscratch(if user { th.kstack_top } else { 0 });
    }
    crate::mm::paging::switch_cr3(th.cr3, from, id);
}

#[cfg(target_arch = "x86_64")]
fn kernel_frame(rip: u64, rsp: u64) -> InterruptFrame {
    let mut f = EMPTY_FRAME;
    f.rip = rip;
    f.cs = KCODE as u64;
    f.rflags = 0x202;
    f.rsp = rsp;
    f.ss = KDATA as u64;
    f
}

#[cfg(target_arch = "riscv64")]
fn kernel_frame(rip: u64, rsp: u64) -> InterruptFrame {
    let mut f = EMPTY_FRAME;
    f.sepc = rip;
    f.set_sp(rsp);
    f.sstatus = crate::arch::riscv64::idt::SSTATUS_SPP | crate::arch::riscv64::idt::SSTATUS_SPIE;
    f
}

#[cfg(target_arch = "x86_64")]
fn user_frame(rip: u64, rsp: u64) -> InterruptFrame {
    let mut f = EMPTY_FRAME;
    f.rip = rip;
    f.cs = USER_CS as u64;
    f.rflags = 0x202;
    f.rsp = rsp;
    f.ss = USER_DS as u64;
    f
}

#[cfg(target_arch = "riscv64")]
fn user_frame(rip: u64, rsp: u64) -> InterruptFrame {
    let mut f = EMPTY_FRAME;
    f.sepc = rip;
    f.set_sp(rsp);
    f.sstatus = crate::arch::riscv64::idt::SSTATUS_SPIE;
    f
}

#[cfg(target_arch = "x86_64")]
unsafe fn resume_to(frame: *const InterruptFrame) -> ! {
    core::arch::asm!(
        "mov rsp, {f}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "add rsp, 16",
        "iretq",
        f = in(reg) frame as u64,
        options(noreturn)
    );
}

#[cfg(target_arch = "riscv64")]
unsafe fn resume_to(frame: *const InterruptFrame) -> ! {
    extern "C" {
        fn trap_return();
    }
    core::arch::asm!(
        "mv sp, {f}",
        "j {ret}",
        f = in(reg) frame as u64,
        ret = sym trap_return,
        options(noreturn)
    );
}

pub fn init() {
    let t = tasks();
    t.queue.quantum = 2;
    for th in t.threads.iter_mut() {
        *th = empty_thread();
    }
    t.current = 0;
    t.started = false;
    t.ping_sent = false;
}

fn install(id: u32, frame: InterruptFrame) {
    let t = tasks();
    let i = slot_index(id);
    t.threads[i].saved = frame;
    t.threads[i].kstack_top = kstack_top(&t.threads[i]);
    t.threads[i].used = true;
    t.threads[i].user_buf = 0;
    if t.threads[i].cr3 == 0 {
        t.threads[i].cr3 = crate::mm::paging::kernel_cr3();
    }
    let _ = t.queue.spawn(id);
}

pub fn spawn_kthread() {
    let rip = kthread_b as usize as u64;
    let t = tasks();
    let i = slot_index(TID_KTHREAD);
    t.threads[i].kstack = KStack([0; KSTACK_SIZE]);
    let rsp = kstack_top(&t.threads[i]) - 16;
    install(TID_KTHREAD, kernel_frame(rip, rsp));
}

pub fn spawn_user(entry: u64, cr3: u64) {
    #[cfg(target_arch = "x86_64")]
    spawn_user_task(TID_USER, entry, USER_STACK_TOP, cr3);
    #[cfg(target_arch = "riscv64")]
    spawn_user_task(TID_USER, entry, USER_RV_STACK_TOP, cr3);
    let _ = USER_IMAGE_BASE;
}

pub fn spawn_user_task(id: u32, entry: u64, stack: u64, cr3: u64) {
    let t = tasks();
    let i = slot_index(id);
    t.threads[i].kstack = KStack([0; KSTACK_SIZE]);
    t.threads[i].cr3 = cr3;
    install(id, user_frame(entry, stack));
}

pub fn current_id() -> u32 {
    tasks().current
}

pub fn started() -> bool {
    tasks().started
}

/// PIT path: save the interrupted frame, maybe rotate, write the next frame back.
pub fn on_timer(frame: &mut InterruptFrame) {
    let t = tasks();
    if !t.started {
        return;
    }
    let cur = t.current;
    t.threads[slot_index(cur)].saved = *frame;
    if let Some(next) = t.queue.tick() {
        if next != cur {
            t.current = next;
            apply_hw(t, cur, next);
            *frame = t.threads[slot_index(next)].saved;
        }
    }
}

/// Syscall-side yield / block: mutate `frame` so the trampoline resumes `next`.
pub fn resched_from_trap(frame: &mut InterruptFrame, block: Option<WaitWhy>, user_buf: u64) {
    let t = tasks();
    if !t.started {
        return;
    }
    let cur = t.current;
    t.threads[slot_index(cur)].saved = *frame;
    t.threads[slot_index(cur)].user_buf = user_buf;
    let next = if let Some(why) = block {
        t.queue.block(why)
    } else {
        t.queue.yield_now()
    };
    if let Some(next) = next {
        if next != cur {
            t.current = next;
            apply_hw(t, cur, next);
            *frame = t.threads[slot_index(next)].saved;
            mark_switched();
        }
    }
}

pub fn wake_recv(ep: u32) {
    let t = tasks();
    let _ = t.queue.wake_recv(aether_core::EndpointId(ep));
}

pub fn wake_accel(queue: u32) {
    let t = tasks();
    let _ = t.queue.wake_accel(queue);
}

pub fn take_user_buf(id: u32) -> u64 {
    let t = tasks();
    let i = slot_index(id);
    let p = t.threads[i].user_buf;
    t.threads[i].user_buf = 0;
    p
}

pub fn set_saved_rax(id: u32, rax: u64) {
    tasks().threads[slot_index(id)].saved.set_ret(rax);
}

pub fn blocked_recv_thread(ep: u32) -> Option<u32> {
    let t = tasks();
    t.queue.slots.iter().find_map(|s| {
        if s.state == aether_core::ThreadState::Blocked && s.wait == WaitWhy::Recv(ep) {
            Some(s.id)
        } else {
            None
        }
    })
}

pub fn blocked_accel_thread(queue: u32) -> Option<u32> {
    let t = tasks();
    t.queue.slots.iter().find_map(|s| {
        if s.state == aether_core::ThreadState::Blocked && s.wait == WaitWhy::Accel(queue) {
            Some(s.id)
        } else {
            None
        }
    })
}

/// First drop: `/init` in ring-3. Does not return.
pub fn enter_user() -> ! {
    let t = tasks();
    t.queue.ensure_running();
    // Prefer the user thread as the first running context.
    if t.threads[slot_index(TID_USER)].used {
        t.queue.current = Some(TID_USER);
        if let Some(i) = t.queue.slots.iter_mut().find(|s| s.id == TID_USER) {
            i.state = aether_core::ThreadState::Running;
        }
        if let Some(i) = t.queue.slots.iter_mut().find(|s| s.id == TID_KTHREAD) {
            i.state = aether_core::ThreadState::Ready;
        }
        t.current = TID_USER;
    }
    // Snapshot before arming: once `started` is true a PIT tick saves the
    // interrupted frame over this slot. If that happens mid-println the
    // user RIP is lost and iretq re-enters this function forever — /init
    // never prints. A longer `run_boot_demo` (OperatorKernel) made the
    // race reliable (self-check already at ~90 ticks).
    let frame = t.threads[slot_index(t.current)].saved;
    let _irq = irq::save_disable();
    t.started = true;
    apply_hw(t, 0, t.current);
    #[cfg(target_arch = "x86_64")]
    println!("[boot] dropping to ring-3 /init (PIT preemption armed, per-task CR3)");
    #[cfg(target_arch = "riscv64")]
    println!("[boot] dropping to U-mode /init (sret, timer armed, per-task satp)");
    unsafe {
        resume_to(core::ptr::addr_of!(frame));
    }
}

fn kthread_b() -> ! {
    let mut last_print = 0u64;
    loop {
        let ticks = irq::ticks();
        if ticks >= 1 && ticks != last_print && ticks % 2 == 0 {
            write_str("[sched] kthread-B tick=");
            write_u64(ticks);
            console::nl();
            last_print = ticks;
        }
        let t = tasks();
        if !t.ping_sent && ticks >= 1 {
            if crate::world::kernel_send_ping() {
                t.ping_sent = true;
                println!("[sched] kthread-B fabric ping posted");
            }
        }
        crate::world::run_pending_accel();
        unsafe {
            #[cfg(target_arch = "x86_64")]
            core::arch::asm!("sti; hlt", options(nomem, nostack));
            #[cfg(target_arch = "riscv64")]
            core::arch::asm!("csrsi sstatus, 2; wfi", options(nomem, nostack));
        }
    }
}
