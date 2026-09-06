//! Kernel objects the syscall gate touches: caps, fabric, arenas, SoftNPU.

use aether_core::accel::{AccelJobDesc, AccelOp};
use aether_core::arena::{Arena, ArenaAllocator, ArenaRequest};
use aether_core::caps::{CPtr, CapKind, CapRights, CapTable, Capability};
use aether_core::color::admit_arena_wave;
use aether_core::fabric::{ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags};
use aether_core::fence::Timeline;
use aether_core::iommu::MapRequest;
use aether_core::partition::{BlastRadius, PartitionId, PartitionProfile, QosBudget, SpatialSlice};
use aether_core::phase::Phase;
use aether_core::preempt::WaitWhy;
use aether_core::sysnr::{UserCompletion, UserIpcMsg};
use aether_core::types::{BankId, ChipletId, PhysAddr, TenantId};
#[cfg(target_arch = "x86_64")]
use aether_core::{INIT_EP_CPTR, INIT_QUEUE_CPTR, USER_IMAGE_BASE, USER_IMAGE_END};
#[cfg(target_arch = "riscv64")]
use aether_core::{
    INIT_EP_CPTR, INIT_QUEUE_CPTR, USER_RV_IMAGE_BASE as USER_IMAGE_BASE,
    USER_RV_IMAGE_END as USER_IMAGE_END,
};
#[cfg(target_arch = "aarch64")]
use aether_core::{
    INIT_EP_CPTR, INIT_QUEUE_CPTR, USER_AA_IMAGE_BASE as USER_IMAGE_BASE,
    USER_AA_IMAGE_END as USER_IMAGE_END,
};
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
    npu: SoftNpuDevice<IdentityDma>,
    ep: EndpointId,
    pending: bool,
    completion: Option<UserCompletion>,
    last_arena: Option<Arena>,
    mapped_va: u64,
    part: PartitionProfile,
    timeline: Timeline,
}

static WORLD: SpinLock<Option<Inner>> = SpinLock::new(None);

pub fn init() {
    let tenant = TenantId(1);
    let mut caps = CapTable::new(tenant);
    let mut fabric = Fabric::new();
    let ep = fabric.create_endpoint(tenant).expect("ep");
    let ep_cptr = caps
        .mint(Capability::new(CapKind::Endpoint, CapRights::EP_FULL, ep.0, tenant).with_badge(0xA3))
        .expect("ep cap");
    assert_eq!(ep_cptr.0, INIT_EP_CPTR);
    let q = caps
        .mint(Capability::new(
            CapKind::AccelQueue,
            CapRights::ACCEL_FULL,
            1,
            tenant,
        ))
        .expect("q cap");
    assert_eq!(q.0, INIT_QUEUE_CPTR);

    #[cfg(target_arch = "x86_64")]
    let arena_bank0 = 0x0100_0000u64;
    #[cfg(target_arch = "riscv64")]
    let arena_bank0 = 0x8300_0000u64;
    #[cfg(target_arch = "aarch64")]
    let arena_bank0 = 0x4300_0000u64;
    crate::mm::frame::reserve_range(arena_bank0, arena_bank0 + 16 * 1024 * 1024);
    let arenas = ArenaAllocator::new(&[
        (BankId(0), PhysAddr(arena_bank0), 8 * 1024 * 1024),
        (BankId(1), PhysAddr(arena_bank0 + 8 * 1024 * 1024), 8 * 1024 * 1024),
    ])
    .expect("arenas");

    let mut npu = SoftNpuDevice::new(IdentityDma);
    let _ = npu.probe();
    // Soft-SMMU pin the /init image so stack tensors remain legal DMA targets.
    let user_mem = caps
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            0xFFFF,
            tenant,
        ))
        .expect("user-image mem cap");
    let user_cap = *caps.lookup(user_mem).expect("user mem");
    let _ = npu.map_with_cap(
        &user_cap,
        MapRequest::pin(PhysAddr(USER_IMAGE_BASE), USER_IMAGE_END - USER_IMAGE_BASE),
    );

    // Software timeline the used-ring IRQ retires. Not a silicon fence.
    let part = PartitionProfile::new(
        PartitionId(1),
        SpatialSlice::single_chiplet(ChipletId(0), 0b111, 0b11),
        QosBudget {
            bw_mbps: 100,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 2,
        },
    );
    let timeline = Timeline::new(part.id);

    *WORLD.lock() = Some(Inner {
        caps,
        fabric,
        arenas,
        npu,
        ep,
        pending: false,
        completion: None,
        last_arena: None,
        mapped_va: 0,
        part,
        timeline,
    });
    write_str("[boot] init caps: ep cptr=");
    write_u64(INIT_EP_CPTR as u64);
    write_str(" queue cptr=");
    write_u64(INIT_QUEUE_CPTR as u64);
    write_str(" object ep=");
    write_u64(ep.0 as u64);
    console::nl();
    println!("[boot] virtqueue MMIO negotiated (SoftNPU backend, Soft SMMU)");
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

/// Device-side IRQ/poll: service the virtqueue, then harvest the used ring.
/// Safe before `init` (timer can fire while World is still None).
pub fn run_pending_accel() {
    let serviced = {
        let mut g = WORLD.lock();
        let Some(w) = g.as_mut() else {
            return;
        };
        if !w.pending && !w.npu.doorbell_pending() && !w.npu.irq_pending() {
            return;
        }
        // x86: KPTI entry already switched to kernel CR3; IdentityDma
        // is PA=VA on that map. RISC-V IRQ may fire on a user satp —
        // IdentityDma is PA=VA through the trampoline map; U-bit / SMAP
        // leaves need SUM / STAC.
        crate::mm::paging::with_user_access(|| {
            let _ = w.npu.service();
            w.npu.poll()
        })
    };
    let Some(cpl) = serviced else {
        return;
    };
    let retired = with(|w| w.npu.retire_into(&mut w.timeline));
    write_str("[accel] used-ring IRQ job#");
    write_u64(cpl.job_seq as u64);
    write_str(" status=");
    write_i32(cpl.status);
    write_str(" cycles=");
    write_u64(cpl.cycles as u64);
    match retired {
        Ok(Some(f)) => {
            write_str(" fence#");
            write_u64(f.seq());
            write_str(" retire");
        }
        Ok(None) => {}
        Err(_) => {
            write_str(" fence-retire skip");
        }
    }
    console::nl();
    with(|w| {
        w.pending = false;
        w.completion = Some(UserCompletion {
            job_seq: cpl.job_seq,
            status: cpl.status,
            cycles: cpl.cycles,
        });
    });
    if let Some(tid) = task::blocked_accel_thread(1) {
        let buf = task::take_user_buf(tid);
        if buf != 0 {
            let _ = write_user_completion(
                buf,
                UserCompletion {
                    job_seq: cpl.job_seq,
                    status: cpl.status,
                    cycles: cpl.cycles,
                },
            );
            task::set_saved_rax(tid, 0);
        }
        task::wake_accel(1);
    }
}

fn copy_ipc_out(dst: u64, badge: u64, flags: u16, payload: &[u8]) -> Result<(), SysError> {
    crate::syscall::copy_to_user(dst, core::mem::size_of::<UserIpcMsg>() as u64)?;
    let mut m = UserIpcMsg::empty();
    m.badge = badge;
    m.flags = flags;
    let _ = m.set_payload(payload);
    crate::mm::paging::with_user_access(|| unsafe {
        core::ptr::write_volatile(dst as *mut UserIpcMsg, m);
    });
    Ok(())
}

fn write_user_completion(dst: u64, cpl: UserCompletion) -> Result<(), SysError> {
    crate::syscall::copy_to_user(dst, core::mem::size_of::<UserCompletion>() as u64)?;
    crate::mm::paging::with_user_access(|| unsafe {
        core::ptr::write_volatile(dst as *mut UserCompletion, cpl);
    });
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

pub fn sys_recv(
    cptr: u64,
    out_ptr: u64,
    frame: &mut crate::arch::idt::InterruptFrame,
) -> Result<u64, SysError> {
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
            frame.set_ret(0);
            task::resched_from_trap(frame, Some(WaitWhy::Recv(object)), out_ptr);
            Err(SysError::Again)
        }
        Err(_) => Err(SysError::Inval),
    }
}

pub fn sys_map(cptr: u64, vaddr: u64, _flags: u64) -> Result<u64, SysError> {
    let cap = with(|w| {
        w.caps
            .require(CPtr(cptr as u16), CapKind::Memory, CapRights::MAP)
            .copied()
            .map_err(|_| SysError::NoCap)
    })?;
    let arena = with(|w| {
        w.last_arena
            .filter(|a| a.id.0 == cap.object)
            .ok_or(SysError::Inval)
    })?;
    let va = if vaddr == 0 { arena.base.0 } else { vaddr };
    let iova = with(|w| {
        w.npu
            .map_with_cap(&cap, MapRequest::pin(PhysAddr(va), arena.size))
            .map_err(|_| SysError::Fault)
    })?;
    paging::allow_user_2m(va);
    with(|w| w.mapped_va = iova.0);
    write_str("[mm] iommu map mem cptr pa=");
    write_hex(va);
    write_str(" iova=");
    write_hex(iova.0);
    write_str(" (soft-smmu sid=0) user-2M");
    console::nl();
    Ok(iova.0)
}

pub fn sys_unmap(vaddr: u64, _len: u64) -> Result<u64, SysError> {
    with(|w| {
        let _ = w.npu.unmap(PhysAddr(vaddr));
    });
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
            .alloc(ArenaRequest::tensor(size, pref).for_tenant(TenantId(1)))
            .map_err(|_| SysError::Inval)
    })?;
    let cptr = with(|w| {
        w.last_arena = Some(arena);
        w.caps
            .mint(Capability::new(
                CapKind::Memory,
                CapRights::MEM_FULL,
                arena.id.0,
                TenantId(1),
            ))
            .map_err(|_| SysError::Inval)
    })?;
    write_str("[mm] arena_alloc cptr=");
    write_u64(cptr.0 as u64);
    write_str(" bank=");
    write_u64(arena.bank.0 as u64);
    write_str(" color.tenant=1 @ ");
    write_hex(arena.base.0);
    console::nl();
    Ok(cptr.0 as u64)
}

fn dma_ok(w: &Inner, ptr: u64, len: u64) -> bool {
    aether_core::sysnr::user_range_known(ptr, len) || w.npu.iommu.covers(PhysAddr(ptr), len)
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
    with(|w| {
        if !dma_ok(w, job.a, 256) {
            return Err(SysError::Fault);
        }
        if let Some(ar) = w.last_arena {
            if admit_arena_wave(1, Phase::Compute, &ar, BankId(0)).is_err() {
                write_str("[accel] refuse foreign bank color (need Exchange / transfer)\r\n");
                return Err(SysError::Inval);
            }
        }
        Ok(())
    })?;
    let Some(op) = AccelOp::from_u32(job.op) else {
        return Err(SysError::Inval);
    };
    let mut desc = AccelJobDesc::matmul_i32(
        job.m,
        job.n,
        job.k,
        PhysAddr(job.a),
        PhysAddr(job.b),
        PhysAddr(job.c),
        1,
    );
    desc.op = op;
    with(|w| {
        let fence = w
            .timeline
            .submit(&w.part, None)
            .map_err(|_| SysError::Again)?;
        desc.partition = w.part.id;
        desc.fence_id = fence.id.0;
        if w.npu.submit(&desc).is_err() {
            let _ = w.timeline.timeout(fence.id);
            return Err(SysError::Again);
        }
        w.pending = true;
        w.completion = None;
        Ok::<(), SysError>(())
    })?;
    println!("[accel] virtqueue doorbell kick (SoftNPU deferred to IRQ)");
    #[cfg(target_arch = "riscv64")]
    crate::arch::riscv64::plic::raise_softnpu_doorbell();
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
    frame.set_ret(0);
    task::resched_from_trap(frame, Some(WaitWhy::Accel(1)), out_ptr);
    Err(SysError::Again)
}
