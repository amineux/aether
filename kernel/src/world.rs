//! Kernel objects the syscall gate touches: caps, fabric, arenas, SoftNPU.

use aether_core::accel::{AccelJobDesc, AccelOp, SoftNpu};
use aether_core::arena::{Arena, ArenaAllocator, ArenaRequest};
use aether_core::caps::{CapKind, CapRights, CapTable, Capability, CPtr};
use aether_core::fabric::{ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags};
use aether_core::preempt::WaitWhy;
use aether_core::sysnr::{UserAccelJob, UserCompletion, UserIpcMsg};
use aether_core::types::{BankId, PhysAddr, TenantId};
use aether_core::{INIT_EP_CPTR, INIT_QUEUE_CPTR};
use aether_drivers::softnpu::IdentityDma;
use aether_drivers::SoftNpuDevice;
use aether_hal::AccelDevice;

use crate::console::{self, write_hex, write_i32, write_str, write_u64};
use crate::mm::paging;
use crate::println;
use crate::sync::SpinLock;
use crate::syscall::SysError;
use crate::task;

struct Inner {
    caps: CapTable,
    fabric: Fabric,
    arenas: ArenaAllocator,
    npu: SoftNpu,
    ep: EndpointId,
    pending: Option<UserAccelJob>,
    completion: Option<UserCompletion>,
    last_arena: Option<Arena>,
    mapped_va: u64,
}

static WORLD: SpinLock<Option<Inner>> = SpinLock::new(None);

pub fn init() {
    let tenant = TenantId(1);
    let mut caps = CapTable::new(tenant);
    let mut fabric = Fabric::new();
    let ep = fabric.create_endpoint(tenant).expect("ep");
    let ep_cptr = caps
        .mint(Capability {
            kind: CapKind::Endpoint,
            rights: CapRights::EP_FULL,
            object: ep.0,
            badge: 0xA3,
            generation: 0,
            tenant,
        })
        .expect("ep cap");
    assert_eq!(ep_cptr.0, INIT_EP_CPTR);
    let q = caps
        .mint(Capability {
            kind: CapKind::AccelQueue,
            rights: CapRights::ACCEL_FULL,
            object: 1,
            badge: 0,
            generation: 0,
            tenant,
        })
        .expect("q cap");
    assert_eq!(q.0, INIT_QUEUE_CPTR);

    let arenas = ArenaAllocator::new(&[
        (BankId(0), PhysAddr(0x0100_0000), 8 * 1024 * 1024),
        (BankId(1), PhysAddr(0x0180_0000), 8 * 1024 * 1024),
    ])
    .expect("arenas");

    *WORLD.lock() = Some(Inner {
        caps,
        fabric,
        arenas,
        npu: SoftNpu::new(),
        ep,
        pending: None,
        completion: None,
        last_arena: None,
        mapped_va: 0,
    });
    write_str("[boot] init caps: ep cptr=");
    write_u64(INIT_EP_CPTR as u64);
    write_str(" queue cptr=");
    write_u64(INIT_QUEUE_CPTR as u64);
    write_str(" object ep=");
    write_u64(ep.0 as u64);
    console::nl();
}

fn with<T>(f: impl FnOnce(&mut Inner) -> T) -> T {
    let mut g = WORLD.lock();
    f(g.as_mut().expect("world"))
}

pub fn kernel_send_ping() -> bool {
    let ep = with(|w| w.ep);
    let msg = match Message::new(
        ep,
        0xA3,
        MsgFlags(MsgFlags::ASYNC),
        ChipletRoute::LOCAL,
        TenantId(1),
        b"ping-fabric",
    ) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let ok = with(|w| w.fabric.send(msg).is_ok());
    if ok {
        if let Some(tid) = task::blocked_recv_thread(ep.0) {
            let buf = task::take_user_buf(tid);
            if buf != 0 {
                let _ = copy_ipc_out(buf, 0xA3, 2, b"ping-fabric");
                task::set_saved_rax(tid, 0);
            }
            task::wake_recv(ep.0);
        }
    }
    ok
}

pub fn run_pending_accel() {
    let job = with(|w| w.pending.take());
    let Some(job) = job else {
        return;
    };
    let Some(op) = AccelOp::from_u32(job.op) else {
        return;
    };
    let desc = AccelJobDesc::matmul_i32(
        job.m,
        job.n,
        job.k,
        PhysAddr(job.a),
        PhysAddr(job.b),
        PhysAddr(job.c),
        1,
    );
    let mut desc = desc;
    desc.op = op;
    let mut idma = IdentityDma;
    let mut npu_ok = None;
    with(|w| {
        if let Ok(cpl) = w.npu.execute(&desc, &mut idma) {
            npu_ok = Some(UserCompletion {
                job_seq: cpl.job_seq,
                status: cpl.status,
                cycles: cpl.cycles,
            });
        }
    });
    let mut dev = SoftNpuDevice::new(IdentityDma);
    let _ = dev.probe();
    let _ = dev.map(PhysAddr(job.a), 256);
    let _ = dev.submit(&desc);
    let _ = dev.poll();

    if let Some(cpl) = npu_ok {
        write_str("[accel] complete job#");
        write_u64(cpl.job_seq as u64);
        write_str(" status=");
        write_i32(cpl.status);
        write_str(" cycles=");
        write_u64(cpl.cycles as u64);
        console::nl();
        with(|w| w.completion = Some(cpl));
        if let Some(tid) = task::blocked_accel_thread(1) {
            let buf = task::take_user_buf(tid);
            if buf != 0 {
                let _ = write_user_completion(buf, cpl);
                task::set_saved_rax(tid, 0);
            }
            task::wake_accel(1);
        }
    }
}

fn copy_ipc_out(dst: u64, badge: u64, flags: u16, payload: &[u8]) -> Result<(), SysError> {
    crate::syscall::copy_to_user(dst, core::mem::size_of::<UserIpcMsg>() as u64)?;
    let mut m = UserIpcMsg::empty();
    m.badge = badge;
    m.flags = flags;
    let _ = m.set_payload(payload);
    unsafe {
        core::ptr::write_volatile(dst as *mut UserIpcMsg, m);
    }
    Ok(())
}

fn write_user_completion(dst: u64, cpl: UserCompletion) -> Result<(), SysError> {
    crate::syscall::copy_to_user(dst, core::mem::size_of::<UserCompletion>() as u64)?;
    unsafe {
        core::ptr::write_volatile(dst as *mut UserCompletion, cpl);
    }
    Ok(())
}

pub fn sys_send(cptr: u64, msg_ptr: u64) -> Result<u64, SysError> {
    let cap = with(|w| {
        w.caps
            .require(CPtr(cptr as u16), CapKind::Endpoint, CapRights::WRITE)
            .map(|c| (c.object, c.badge))
            .map_err(|_| SysError::NoCap)
    })?;
    let m = crate::syscall::copy_user_ipc(msg_ptr)?;
    let dest = EndpointId(cap.0);
    let msg = Message::new(
        dest,
        if m.badge != 0 { m.badge } else { cap.1 },
        MsgFlags(m.flags),
        ChipletRoute::LOCAL,
        TenantId(1),
        m.payload(),
    )
    .map_err(|_| SysError::Inval)?;
    with(|w| w.fabric.send(msg)).map_err(|e| match e {
        FabricError::QueueFull => SysError::Again,
        _ => SysError::Inval,
    })?;
    if let Some(tid) = task::blocked_recv_thread(dest.0) {
        let buf = task::take_user_buf(tid);
        if buf != 0 {
            let _ = copy_ipc_out(buf, m.badge, m.flags, m.payload());
            task::set_saved_rax(tid, 0);
        }
        task::wake_recv(dest.0);
    }
    Ok(0)
}

pub fn sys_recv(cptr: u64, out_ptr: u64, frame: &mut crate::arch::idt::InterruptFrame) -> Result<u64, SysError> {
    let object = with(|w| {
        w.caps
            .require(CPtr(cptr as u16), CapKind::Endpoint, CapRights::READ)
            .map(|c| c.object)
            .map_err(|_| SysError::NoCap)
    })?;
    crate::syscall::copy_to_user(out_ptr, core::mem::size_of::<UserIpcMsg>() as u64)?;
    match with(|w| w.fabric.recv(EndpointId(object))) {
        Ok(got) => {
            copy_ipc_out(out_ptr, got.header.badge, got.header.flags.0, got.payload())?;
            Ok(0)
        }
        Err(FabricError::WouldBlock) => {
            frame.rax = 0;
            task::resched_from_trap(frame, Some(WaitWhy::Recv(object)), out_ptr);
            Err(SysError::Again)
        }
        Err(_) => Err(SysError::Inval),
    }
}

pub fn sys_map(cptr: u64, vaddr: u64, _flags: u64) -> Result<u64, SysError> {
    let object = with(|w| {
        w.caps
            .require(CPtr(cptr as u16), CapKind::Memory, CapRights::MAP)
            .map(|c| c.object)
            .map_err(|_| SysError::NoCap)
    })?;
    let arena = with(|w| {
        w.last_arena
            .filter(|a| a.id.0 == object)
            .ok_or(SysError::Inval)
    })?;
    let va = if vaddr == 0 { arena.base.0 } else { vaddr };
    paging::allow_user_2m(va);
    with(|w| w.mapped_va = va);
    write_str("[mm] map mem cptr va=");
    write_hex(va);
    write_str(" user-2M");
    console::nl();
    Ok(va)
}

pub fn sys_unmap(_vaddr: u64, _len: u64) -> Result<u64, SysError> {
    Ok(0)
}

pub fn sys_arena_alloc(size: u64, _flags: u64, bank: u64) -> Result<u64, SysError> {
    if size == 0 || size > 64 * 1024 {
        return Err(SysError::Inval);
    }
    let pref = if bank == 0 {
        Some(BankId(0))
    } else {
        Some(BankId(bank as u8))
    };
    let arena = with(|w| {
        w.arenas
            .alloc(ArenaRequest::tensor(size, pref))
            .map_err(|_| SysError::Inval)
    })?;
    let cptr = with(|w| {
        w.last_arena = Some(arena);
        w.caps
            .mint(Capability {
                kind: CapKind::Memory,
                rights: CapRights::MEM_FULL,
                object: arena.id.0,
                badge: 0,
                generation: 0,
                tenant: TenantId(1),
            })
            .map_err(|_| SysError::Inval)
    })?;
    write_str("[mm] arena_alloc cptr=");
    write_u64(cptr.0 as u64);
    write_str(" bank=");
    write_u64(arena.bank.0 as u64);
    write_str(" @ ");
    write_hex(arena.base.0);
    console::nl();
    Ok(cptr.0 as u64)
}

pub fn sys_accel_submit(cptr: u64, job_ptr: u64) -> Result<u64, SysError> {
    with(|w| {
        w.caps
            .require(CPtr(cptr as u16), CapKind::AccelQueue, CapRights::SUBMIT)
            .map(|_| ())
            .map_err(|_| SysError::NoCap)
    })?;
    let job = crate::syscall::copy_user_job(job_ptr)?;
    if job.m != 4 || job.n != 4 || job.k != 4 {
        return Err(SysError::Inval);
    }
    crate::syscall::copy_from_user(job.a, 256)?;
    with(|w| {
        w.pending = Some(job);
        w.completion = None;
    });
    println!("[accel] submit queued (cap SUBMIT ok, SoftNPU deferred)");
    Ok(0)
}

pub fn sys_accel_wait(
    cptr: u64,
    out_ptr: u64,
    frame: &mut crate::arch::idt::InterruptFrame,
) -> Result<u64, SysError> {
    with(|w| {
        w.caps
            .require(CPtr(cptr as u16), CapKind::AccelQueue, CapRights::WAIT)
            .map(|_| ())
            .map_err(|_| SysError::NoCap)
    })?;
    crate::syscall::copy_to_user(out_ptr, core::mem::size_of::<UserCompletion>() as u64)?;
    if let Some(cpl) = with(|w| w.completion.take()) {
        write_user_completion(out_ptr, cpl)?;
        return Ok(0);
    }
    frame.rax = 0;
    task::resched_from_trap(frame, Some(WaitWhy::Accel(1)), out_ptr);
    Err(SysError::Again)
}
