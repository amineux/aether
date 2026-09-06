//! x86_64 SMP smoke: INIT-SIPI AP 1, per-CPU `gs`, IPI, work-steal.
//!
//! APs stay in kernel mode. Ring-3 `/init` and kthread-B remain BSP-only.
//! Per-task PML4 is a follow-up on the BSP user path; APs stay on the
//! kernel CR3. SMEP/SMAP are armed here so CR4 matches the BSP.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use aether_core::sched::{Job, JobKind, TileKind, TileScheduler};
use aether_core::types::{BankId, TileId};
use aether_core::Phase;

use crate::arch::irq;
use crate::println;
use crate::sync::SpinLock;

use super::{apic, cpu, gdt, idt};

const AP_TRAMP_PHYS: u64 = 0x8000;
const AP_MAIL_STACK: u64 = 0x8F00;
const AP_MAIL_ENTRY: u64 = 0x8F08;
const AP_MAIL_SIG: u64 = 0x8F10;
const AP_SIG_LIVE: u32 = 0xA5A5_0001;
const APIC_ID_AP: u32 = 1;
const AP_STACK_SIZE: usize = 16 * 1024;
const SMOKE_JOBS: u32 = 8;

core::arch::global_asm!(include_str!("ap_tramp.S"), options(att_syntax));

extern "C" {
    fn ap_trampoline_start();
    fn ap_trampoline_end();
}

#[repr(align(16))]
struct ApStack([u8; AP_STACK_SIZE]);

static mut AP_STACK: ApStack = ApStack([0; AP_STACK_SIZE]);

static AP_ONLINE: AtomicBool = AtomicBool::new(false);
static WORK_GO: AtomicBool = AtomicBool::new(false);
static HARTS_READY: AtomicU32 = AtomicU32::new(0);
static WORK_DONE: AtomicU32 = AtomicU32::new(0);
static CPU_JOBS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
static AP_STEAL_JOBS: AtomicU32 = AtomicU32::new(0);

static SCHED: SpinLock<TileScheduler> = SpinLock::new(TileScheduler::new());

fn ap_stack_top() -> u64 {
    let p = unsafe { core::ptr::addr_of!(AP_STACK.0) as u64 } + AP_STACK_SIZE as u64;
    p & !0xF
}

fn write_u64(addr: u64, v: u64) {
    unsafe {
        core::ptr::write_volatile(addr as *mut u64, v);
    }
}

fn write_u32(addr: u64, v: u32) {
    unsafe {
        core::ptr::write_volatile(addr as *mut u32, v);
    }
}

fn read_u32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Wait for `pred`, bounded by PIT ticks so UP `qemu-ci` does not sit for seconds.
fn wait_ticks(mut pred: impl FnMut() -> bool, max_ticks: u64) -> bool {
    let start = irq::ticks();
    let mut spins = 0u64;
    loop {
        if pred() {
            return true;
        }
        if irq::ticks().saturating_sub(start) >= max_ticks || spins > 8_000_000 {
            return pred();
        }
        core::hint::spin_loop();
        spins += 1;
    }
}

fn install_trampoline() -> bool {
    let src = ap_trampoline_start as usize;
    let end = ap_trampoline_end as usize;
    if end <= src {
        return false;
    }
    let n = end - src;
    if n == 0 || n > 0x0F00 {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(src as *const u8, AP_TRAMP_PHYS as *mut u8, n);
    }
    write_u64(AP_MAIL_STACK, ap_stack_top());
    write_u64(AP_MAIL_ENTRY, ap_entry as usize as u64);
    write_u32(AP_MAIL_SIG, 0);
    core::sync::atomic::fence(Ordering::SeqCst);
    true
}

fn smoke_job(id: u32, hint: Option<TileId>) -> Job {
    Job {
        id,
        kind: JobKind::Thread,
        tile_hint: hint,
        bank_affinity: None,
        priority: 4,
        deadline_ticks: None,
        tenant: 1,
        cut_id: None,
        phase: Phase::Compute,
        partition_id: None,
        fence_id: None,
        arena_color: None,
    }
}

fn prepare_scheduler() {
    let mut s = SCHED.lock();
    *s = TileScheduler::new();
    s.add_tile(TileId(0), TileKind::Cpu, BankId(0));
    s.add_tile(TileId(1), TileKind::Cpu, BankId(1));
    // Job 1 stays on the BSP (hard affinity). The rest are stealable.
    s.enqueue(smoke_job(1, Some(TileId(0))));
    for id in 2..=SMOKE_JOBS {
        s.enqueue(smoke_job(id, None));
    }
}

fn hart_step(cpu: u32) -> bool {
    let tile = TileId(cpu as u16);
    let mut s = SCHED.lock();
    let (job, stole) = if cpu == 0 {
        match s.pick(tile) {
            Some(j) => (Some(j), false),
            None => (s.steal(tile), true),
        }
    } else {
        match s.steal(tile) {
            Some(j) => (Some(j), true),
            None => (s.pick(tile), false),
        }
    };
    drop(s);
    let Some(_job) = job else {
        return false;
    };
    if (cpu as usize) < CPU_JOBS.len() {
        CPU_JOBS[cpu as usize].fetch_add(1, Ordering::Relaxed);
    }
    if stole && cpu != 0 {
        AP_STEAL_JOBS.fetch_add(1, Ordering::Relaxed);
    }
    true
}

fn drain_work(cpu: u32) {
    HARTS_READY.fetch_add(1, Ordering::Release);
    let _ = wait_ticks(|| HARTS_READY.load(Ordering::Acquire) >= 2, 10);
    let mut idle = 0u32;
    while idle < 256 {
        if hart_step(cpu) {
            idle = 0;
        } else {
            idle += 1;
            core::hint::spin_loop();
        }
    }
    WORK_DONE.fetch_add(1, Ordering::Release);
}

fn run_work_steal_smoke() {
    CPU_JOBS[0].store(0, Ordering::Relaxed);
    CPU_JOBS[1].store(0, Ordering::Relaxed);
    AP_STEAL_JOBS.store(0, Ordering::Relaxed);
    WORK_DONE.store(0, Ordering::Relaxed);
    HARTS_READY.store(0, Ordering::Relaxed);
    prepare_scheduler();
    WORK_GO.store(true, Ordering::Release);

    drain_work(0);

    let _ = wait_ticks(|| WORK_DONE.load(Ordering::Acquire) >= 2, 20);

    let c0 = CPU_JOBS[0].load(Ordering::Relaxed);
    let c1 = CPU_JOBS[1].load(Ordering::Relaxed);
    let stolen = AP_STEAL_JOBS.load(Ordering::Relaxed);
    crate::console::write_str("[smp] work-steal: cpu0=");
    crate::console::write_u64(c0 as u64);
    crate::console::write_str(" cpu1=");
    crate::console::write_u64(c1 as u64);
    crate::console::write_str(" jobs=");
    crate::console::write_u64((c0 + c1) as u64);
    crate::console::write_str(" steal=");
    crate::console::write_u64(stolen as u64);
    crate::console::nl();

    if c0 > 0 && c1 > 0 && stolen > 0 {
        println!("[smp] SMP smoke ok (2 harts)");
    } else {
        println!("[smp] SMP smoke FAIL (need jobs on both CPUs + a steal)");
    }
}

/// AP long-mode entry. Does not return.
#[no_mangle]
pub extern "C" fn ap_entry() -> ! {
    idt::load();
    gdt::load_ap();
    super::kpti::load_ap();
    cpu::set_gs(1);
    crate::mm::paging::enable_smep_smap();
    apic::enable();
    AP_ONLINE.store(true, Ordering::Release);
    irq::enable();

    let _ = wait_ticks(|| WORK_GO.load(Ordering::Acquire), 50);
    if WORK_GO.load(Ordering::Acquire) {
        drain_work(1);
    }

    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

pub fn start_aps() {
    cpu::init_bsp();
    apic::enable();

    crate::console::write_str("[smp] BSP apic_id=");
    crate::console::write_u64(apic::local_id() as u64);
    crate::console::write_str(" gs cpu_id=");
    crate::console::write_u64(cpu::id() as u64);
    crate::console::nl();

    if !install_trampoline() {
        println!("[smp] AP trampoline install failed; staying UP");
        return;
    }

    apic::init_sipi(APIC_ID_AP, AP_TRAMP_PHYS);
    println!("[smp] INIT-SIPI sent to APIC ID 1 (vector 8 @ 0x8000)");

    let online = wait_ticks(
        || AP_ONLINE.load(Ordering::Acquire) || read_u32(AP_MAIL_SIG) == AP_SIG_LIVE,
        10,
    );

    if !AP_ONLINE.load(Ordering::Acquire) {
        if online && read_u32(AP_MAIL_SIG) == AP_SIG_LIVE {
            println!("[smp] AP reached long mode but not ap_entry; staying UP");
        } else {
            println!("[smp] UP only (AP 1 did not come online; try make qemu-smp)");
        }
        return;
    }

    cpu::set_ncpus(2);
    println!("[smp] AP 1 online (gs cpu_id=1)");

    let before = cpu::local_ticks_of(1);
    apic::ipi(APIC_ID_AP, apic::IPI_VECTOR);
    let ipi_ok = wait_ticks(|| cpu::local_ticks_of(1) > before, 10);
    crate::console::write_str("[smp] IPI ");
    crate::console::write_str(if ipi_ok { "ok" } else { "FAIL" });
    crate::console::write_str(" (ap ticks=");
    crate::console::write_u64(cpu::local_ticks_of(1));
    crate::console::write_str(")");
    crate::console::nl();

    run_work_steal_smoke();
}
