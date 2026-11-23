//! Kernel objects the syscall gate touches: caps, fabric, arenas, SoftNPU.

use aether_core::accel::{AccelJobDesc, AccelOp};
use aether_core::arena::{Arena, ArenaAllocator, ArenaId, ArenaRequest};
use aether_core::caps::{CPtr, CapKind, CapRights, CapTable, Capability};
use aether_core::color::admit_arena_wave;
use aether_core::fabric::{ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags};
use aether_core::fence::Timeline;
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
use aether_drivers::softnpu::KernelDma;
use aether_drivers::SoftNpuDevice;
use aether_hal::AccelDevice;

use crate::console::{self, write_hex, write_i32, write_str, write_u64};
use crate::mm::paging;
use crate::println;
use crate::sync::SpinLock;
use crate::syscall::SysError;
use crate::task;

struct Inner {
    cap_spaces: crate::cap_spaces::CapSpaces,
    fabric: Fabric,
    arenas: ArenaAllocator,
    npu: SoftNpuDevice<KernelDma>,
    ep: EndpointId,
    pending: bool,
    completion: Option<UserCompletion>,
    last_arena: Option<Arena>,
    /// Authority for every live SYS_MAP pin; boot pins are not syscall-owned.
    dma_pins: crate::dma_pins::DmaPins,
    part: PartitionProfile,
    timeline: Timeline,
}

static WORLD: SpinLock<Option<Inner>> = SpinLock::new(None);

/// Boot code supplies the loaded /init root, never a syscall argument.
pub fn bind_init_caps(root: u64) {
    with(|w| w.cap_spaces.bind_init(root)).expect("bootstrap cap root already bound or invalid");
}

/// A separate endpoint/table for the second boot image, never bootstrap aliases.
#[cfg(target_arch = "x86_64")]
pub fn bind_probe_caps(root: u64) {
    with(|w| {
        let ep = w
            .fabric
            .create_endpoint(w.cap_spaces.bootstrap().owner())
            .expect("probe ep");
        assert_ne!(ep, w.ep);
        w.cap_spaces
            .bind_probe_endpoint(root, ep.0)
            .expect("probe cap root invalid");
    });
}

fn caller_root() -> Result<u64, SysError> {
    let root = task::current_user_root().ok_or(SysError::NoCap)?;
    with(|w| w.cap_spaces.get(root).map(|_| root).ok_or(SysError::NoCap))
}

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

    let mut npu = SoftNpuDevice::new(KernelDma);
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
    crate::accel_boot::prepare(
        &mut npu,
        &user_cap,
        PhysAddr(USER_IMAGE_BASE),
        USER_IMAGE_END - USER_IMAGE_BASE,
    )
    .unwrap_or_else(|_| {
        println!("[boot] SoftNPU preparation failed (probe or user-image DMA pin)");
        panic!("required accelerator initialization failed");
    });

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
        cap_spaces: crate::cap_spaces::CapSpaces::new(caps),
        fabric,
        arenas,
        npu,
        ep,
        pending: false,
        completion: None,
        last_arena: None,
        dma_pins: crate::dma_pins::DmaPins::new(),
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
    println!("[boot] init DMA ready (probe + full user-image pin)");
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
    // Kernel ping on tenant 1's own endpoint, through the checked path.
    let ok = with(|w| w.fabric.send_as(w.cap_spaces.bootstrap(), msg).is_ok());
    if ok {
        deliver_blocked_recv(ep);
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
        // x86: KPTI entry already switched to kernel CR3; KernelDma
        // loads guest PAs via HH (`phys_to_kva`). RISC-V IRQ may fire
        // on a user satp — KernelDma is PA=VA there; U-bit / SMAP
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
        deliver_blocked_accel(w);
        if w.completion.is_some() {
            println!("[accel] completion cached until wait");
        }
    });
}

/// Called under the World lock so result consumption, saved errno and the
/// selected waiter's wake cannot race the current BSP userspace scheduler.
fn deliver_blocked_accel(w: &mut Inner) {
    let Some(tid) = task::blocked_accel_thread(1) else {
        return;
    };
    let root = task::thread_cr3(tid);
    let copied = crate::completion_delivery::deliver_next(&mut w.completion, |completion| {
        let buf = task::take_user_buf(tid);
        write_user_completion_in(root, buf, completion)
    });
    if let Some(result) = copied {
        task::set_saved_rax(
            tid,
            match result {
                Ok(()) => 0,
                Err(error) => crate::syscall::err_rax(error),
            },
        );
        task::wake_accel(1, tid);
        println!("[accel] consumed completion for one blocked waiter");
    }
}

fn copy_ipc_out(dst: u64, badge: u64, flags: u16, payload: &[u8]) -> Result<(), SysError> {
    copy_ipc_out_in(0, dst, badge, flags, payload)
}

fn copy_ipc_out_in(
    root: u64,
    dst: u64,
    badge: u64,
    flags: u16,
    payload: &[u8],
) -> Result<(), SysError> {
    let mut m = UserIpcMsg::empty();
    m.badge = badge;
    m.flags = flags;
    let _ = m.set_payload(payload);
    // Field-wise bytes: padding goes out as zeros, never kernel stack.
    let mut b = [0u8; core::mem::size_of::<UserIpcMsg>()];
    b[0..8].copy_from_slice(&m.badge.to_ne_bytes());
    b[8..10].copy_from_slice(&m.flags.to_ne_bytes());
    b[10..12].copy_from_slice(&m.len.to_ne_bytes());
    b[12..76].copy_from_slice(&m.payload);
    crate::syscall::write_user_in(root, dst, &b)
}

fn write_user_completion(dst: u64, cpl: UserCompletion) -> Result<(), SysError> {
    write_user_completion_in(0, dst, cpl)
}

fn write_user_completion_in(root: u64, dst: u64, cpl: UserCompletion) -> Result<(), SysError> {
    let mut b = [0u8; core::mem::size_of::<UserCompletion>()];
    b[0..4].copy_from_slice(&cpl.job_seq.to_ne_bytes());
    b[4..8].copy_from_slice(&cpl.status.to_ne_bytes());
    b[8..12].copy_from_slice(&cpl.cycles.to_ne_bytes());
    crate::syscall::write_user_in(root, dst, &b)
}

/// Consume the FIFO entry and complete exactly one blocked receive. The World
/// lock disables interrupts, so the selected waiter/buffer cannot change between
/// dequeue, copy and targeted wake on the current BSP-only userspace scheduler.
fn deliver_blocked_recv(endpoint: EndpointId) {
    with(|w| {
        let Some(tid) = task::blocked_recv_thread(endpoint.0) else {
            return;
        };
        let root = task::thread_cr3(tid);
        let Some(caps) = w.cap_spaces.get(root) else {
            return;
        };
        let result =
            crate::ipc_delivery::deliver_next(&mut w.fabric, caps.owner(), endpoint, |message| {
                let buf = task::take_user_buf(tid);
                copy_ipc_out_in(
                    root,
                    buf,
                    message.header.badge,
                    message.header.flags.0,
                    message.payload(),
                )
            });
        if let Ok(copied) = result {
            let succeeded = copied.is_ok();
            task::set_saved_rax(
                tid,
                match copied {
                    Ok(()) => 0,
                    Err(error) => crate::syscall::err_rax(error),
                },
            );
            task::wake_recv(endpoint.0, tid);
            if succeeded {
                println!("[ipc] queued message completed one blocked receive");
            }
        }
        // Empty/invalid queue: callback never ran; keep the waiter and its
        // saved buffer blocked rather than returning a fabricated success.
    });
}

/// Copy endpoint READ/WRITE rights into a live thread's registered cap space.
/// The returned CPtr belongs to the destination; threads sharing a root/table
/// are refused. No caller-supplied root or tenant reaches the table registry.
pub fn sys_cap_grant(cptr: u64, target_tid: u64, rights: u64) -> Result<u64, SysError> {
    let root = caller_root()?;
    let cptr = CPtr::try_from(cptr).map_err(|_| SysError::NoCap)?;
    let target = task::user_root(target_tid).ok_or(SysError::NoCap)?;
    with(|w| w.cap_spaces.grant_endpoint(root, target, cptr, rights))
        .map(|slot| u64::from(slot.0))
        .map_err(|error| match error {
            aether_core::caps::CapError::TableFull => SysError::Again,
            _ => SysError::NoCap,
        })
}

pub fn sys_send(cptr: u64, msg_ptr: u64) -> Result<u64, SysError> {
    let root = caller_root()?;
    let cptr = CPtr::try_from(cptr).map_err(|_| SysError::NoCap)?;
    let cap = with(|w| {
        let caps = w.cap_spaces.get(root).ok_or(SysError::NoCap)?;
        caps.require(cptr, CapKind::Endpoint, CapRights::WRITE)
            .map(|c| (c.object, c.badge, caps.owner()))
            .map_err(|_| SysError::NoCap)
    })?;
    let m = crate::syscall::copy_user_ipc(msg_ptr)?;
    let dest = EndpointId(cap.0);
    // Sender tag is the calling table's owner, never caller-chosen.
    let caller = cap.2;
    let msg = Message::new(
        dest,
        if m.badge != 0 { m.badge } else { cap.1 },
        MsgFlags(m.flags),
        ChipletRoute::LOCAL,
        caller,
        m.payload(),
    )
    .map_err(|_| SysError::Inval)?;
    // Checked send: caller owns `dest` or holds Endpoint+WRITE on it;
    // Hodge quota is charged to the caller's own per-tenant budget.
    with(|w| {
        let caps = w.cap_spaces.get(root).ok_or(SysError::NoCap)?;
        w.fabric.send_as(caps, msg).map_err(|e| match e {
            FabricError::QueueFull => SysError::Again,
            _ => SysError::Inval,
        })
    })?;
    deliver_blocked_recv(dest);
    Ok(0)
}

pub fn sys_recv(
    cptr: u64,
    out_ptr: u64,
    frame: &mut crate::arch::idt::InterruptFrame,
) -> Result<u64, SysError> {
    let root = caller_root()?;
    let cptr = CPtr::try_from(cptr).map_err(|_| SysError::NoCap)?;
    let (object, owner) = with(|w| {
        let caps = w.cap_spaces.get(root).ok_or(SysError::NoCap)?;
        caps.require(cptr, CapKind::Endpoint, CapRights::READ)
            .map(|c| (c.object, caps.owner()))
            .map_err(|_| SysError::NoCap)
    })?;
    crate::syscall::copy_to_user(out_ptr, core::mem::size_of::<UserIpcMsg>() as u64)?;
    // Checked recv: only the endpoint's owner dequeues.
    match with(|w| w.fabric.recv_as(owner, EndpointId(object))) {
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
    let root = caller_root()?;
    let cptr = CPtr::try_from(cptr).map_err(|_| SysError::NoCap)?;
    let cap = with(|w| {
        w.cap_spaces
            .get(root)
            .ok_or(SysError::NoCap)?
            .require(cptr, CapKind::Memory, CapRights::MAP)
            .copied()
            .map_err(|_| SysError::NoCap)
    })?;
    let arena = with(|w| {
        w.arenas
            .get(ArenaId(cap.object))
            .copied()
            .map_err(|_| SysError::Inval)
    })?;
    if arena.owner_tenant != Some(cap.tenant.0) {
        return Err(SysError::NoCap);
    }
    // The pinned physical address is derived only from the caller's own
    // arena capability. A caller-supplied address other than the arena's
    // own base is refused (would otherwise pin — and on x86 user-map —
    // arbitrary physical memory, e.g. kernel text). See sysnr::map_pin_addr.
    let va = aether_core::sysnr::map_pin_addr(arena.base.0, vaddr).ok_or(SysError::Inval)?;
    let owner_root = root;
    let iova = with(|w| {
        w.dma_pins
            .map(&mut w.npu, owner_root, &cap, PhysAddr(va), arena.size)
            .map_err(|_| SysError::Fault)
    })?;
    paging::allow_user_2m(va);
    write_str("[mm] iommu map mem cptr pa=");
    write_hex(va);
    write_str(" iova=");
    write_hex(iova.0);
    write_str(" (soft-smmu sid=0) user-2M");
    console::nl();
    Ok(iova.0)
}

pub fn sys_unmap(vaddr: u64, _len: u64) -> Result<u64, SysError> {
    let root = caller_root()?;
    let owner_root = root;
    // Syscall 5 keeps its number and `(vaddr, len)` args. `len` is ignored:
    // this is not munmap of `SYS_MMAP` pages. The Result is the existing
    // negative `SysError` return — a refused or missing pin is not `Ok(0)`.
    with(|w| {
        if w.dma_pins.is_empty() {
            return Err(SysError::NoCap);
        }
        // Stored per-pin authority is removed only after tenant-checked unmap
        // succeeds. Refusing an unknown address cannot lose another live pin.
        w.dma_pins
            .unmap(&mut w.npu, owner_root, PhysAddr(vaddr))
            .map_err(|_| SysError::Fault)?;
        Ok(0)
    })
}

pub fn sys_arena_alloc(size: u64, _flags: u64, bank: u64) -> Result<u64, SysError> {
    let root = caller_root()?;
    if !with(|w| w.cap_spaces.can_allocate(root)) {
        return Err(SysError::NoCap);
    }
    if size == 0 || size > 64 * 1024 {
        return Err(SysError::Inval);
    }
    // Reject out-of-width values rather than silently selecting bank 0 for 256.
    let bank = u8::try_from(bank).map_err(|_| SysError::Inval)?;
    let pref = Some(BankId(bank));
    let (arena, cptr) = with(|w| {
        let (arena, cptr) = w
            .arenas
            .alloc_with_cap(
                w.cap_spaces.get_mut(root).ok_or(SysError::NoCap)?,
                ArenaRequest::tensor(size, pref),
            )
            .map_err(|_| SysError::Inval)?;
        w.last_arena = Some(arena);
        Ok::<_, SysError>((arena, cptr))
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
    let root = caller_root()?;
    let cptr = CPtr::try_from(cptr).map_err(|_| SysError::NoCap)?;
    with(|w| {
        w.cap_spaces
            .get(root)
            .ok_or(SysError::NoCap)?
            .require(cptr, CapKind::AccelQueue, CapRights::SUBMIT)
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
        let Inner {
            pending,
            completion,
            timeline,
            npu,
            part,
            ..
        } = w;
        crate::completion_delivery::submit_if_idle(pending, completion, || {
            let fence = timeline.submit(part, None).map_err(|_| SysError::Again)?;
            desc.partition = part.id;
            desc.fence_id = fence.id.0;
            if npu.submit(&desc).is_err() {
                let _ = timeline.timeout(fence.id);
                return Err(SysError::Again);
            }
            Ok(())
        })
        .unwrap_or(Err(SysError::Again))
    })?;
    println!("[accel] virtqueue doorbell kick (SoftNPU deferred to IRQ)");
    #[cfg(target_arch = "riscv64")]
    crate::arch::riscv64::plic::raise_softnpu_doorbell();
    #[cfg(target_arch = "x86_64")]
    crate::arch::x86_64::softnpu_irq::raise_softnpu_doorbell();
    #[cfg(target_arch = "aarch64")]
    crate::arch::aarch64::softnpu_irq::raise_softnpu_doorbell();
    Ok(0)
}

pub fn sys_accel_wait(
    cptr: u64,
    out_ptr: u64,
    frame: &mut crate::arch::idt::InterruptFrame,
) -> Result<u64, SysError> {
    let root = caller_root()?;
    let cptr = CPtr::try_from(cptr).map_err(|_| SysError::NoCap)?;
    with(|w| {
        w.cap_spaces
            .get(root)
            .ok_or(SysError::NoCap)?
            .require(cptr, CapKind::AccelQueue, CapRights::WAIT)
            .map(|_| ())
            .map_err(|_| SysError::NoCap)
    })?;
    crate::syscall::copy_to_user(out_ptr, core::mem::size_of::<UserCompletion>() as u64)?;
    let copied = with(|w| {
        crate::completion_delivery::deliver_next(&mut w.completion, |cpl| {
            write_user_completion(out_ptr, cpl)
        })
    });
    if let Some(result) = copied {
        result?;
        return Ok(0);
    }
    frame.set_ret(0);
    task::resched_from_trap(frame, Some(WaitWhy::Accel(1)), out_ptr);
    Err(SysError::Again)
}
