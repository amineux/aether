//! Shared boot-demo: fabric IPC + tensor arena + SoftNPU matmul.
//!
//! The kernel prints this report; host tests assert the same path.

use crate::accel::{demo_f16_f32_ok, AccelJobDesc, AccelOp, SliceMem, SoftNpu};
use crate::activity::{Activity, ActivityId, ActivityKind};
use crate::arena::{ArenaAllocator, ArenaRequest};
use crate::caps::{CapKind, CapRights, CapTable, Capability};
use crate::color::{admit_arena_wave, ColorError};
use crate::cut::{bind_place, CutError, SpectralCut};
use crate::fabric::{ChipletRoute, Fabric, FabricError, Message, MsgFlags};
use crate::fence::Timeline;
use crate::hodge::{authorize, FlowClass, HodgeError, CLASS_ALL, CLASS_CURL, CLASS_GRADIENT};
use crate::iommu::{IommuMap, MapError, MapRequest};
use crate::observe::{EventKind, EventRing};
use crate::opkernel::{CollectiveKind, OpKernelError, OpKernelId, OperatorKernelHandle};
use crate::partition::{
    BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
};
use crate::phase::Phase;
use crate::sched::{Job, JobKind, TileKind, TileScheduler};
use crate::space::{map_place, FabricAddr, MemorySpace, Place, SpaceError};
use crate::sparsify::{decide_header, SparsifiedCollective, SparsifyAction};
use crate::types::{BankId, ChipletId, PhysAddr, TenantId, TileId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemoReport {
    pub ipc_ok: bool,
    pub arena_ok: bool,
    pub accel_ok: bool,
    pub isolation_ok: bool,
    pub sched_ok: bool,
    pub cut_ok: bool,
    pub hodge_ok: bool,
    pub space_ok: bool,
    pub activity_ok: bool,
    pub fence_ok: bool,
    pub map_ok: bool,
    pub color_ok: bool,
    pub revoke_ok: bool,
    pub opkernel_ok: bool,
    pub sparsify_ok: bool,
    pub dtype_ok: bool,
    pub job_seq: u32,
    pub c00: i32,
    pub c11: i32,
    pub arena_base: u64,
    pub arena_bank: u8,
    pub events: u32,
    pub cut_phi_milli: u32,
    pub fence_id: u64,
}

impl DemoReport {
    pub fn all_ok(&self) -> bool {
        self.ipc_ok
            && self.arena_ok
            && self.accel_ok
            && self.isolation_ok
            && self.sched_ok
            && self.cut_ok
            && self.hodge_ok
            && self.space_ok
            && self.activity_ok
            && self.fence_ok
            && self.map_ok
            && self.color_ok
            && self.revoke_ok
            && self.opkernel_ok
            && self.sparsify_ok
            && self.dtype_ok
    }
}

/// 4×4 identity @ known matrix. C must equal B.
pub const DEMO_B: [i32; 16] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53];

pub fn run_boot_demo() -> DemoReport {
    let mut events = EventRing::new();
    events.emit(EventKind::Boot, 1, 0);

    let tenant_a = TenantId(1);
    let tenant_b = TenantId(2);
    let mut caps_a = CapTable::new(tenant_a);
    let mut caps_b = CapTable::new(tenant_b);

    let mut fabric = Fabric::new();
    let ep_a = fabric.create_endpoint(tenant_a).unwrap();
    let ep_b = fabric.create_endpoint(tenant_b).unwrap();

    let ep_cap_a = caps_a
        .mint(
            Capability::new(CapKind::Endpoint, CapRights::EP_FULL, ep_a.0, tenant_a)
                .with_badge(0xA3),
        )
        .unwrap();
    let ep_cap_b = caps_b
        .mint(
            Capability::new(CapKind::Endpoint, CapRights::EP_FULL, ep_b.0, tenant_b)
                .with_badge(0xB7),
        )
        .unwrap();
    events.emit(EventKind::CapMint, ep_a.0 as u64, ep_b.0 as u64);

    fabric
        .send(
            Message::new(
                ep_b,
                0xA3,
                MsgFlags(MsgFlags::ASYNC),
                ChipletRoute::for_tile(TileId(0)),
                tenant_a,
                b"ping-fabric",
            )
            .unwrap(),
        )
        .unwrap();
    events.emit(EventKind::IpcSend, ep_b.0 as u64, 0xA3);
    let got = fabric.recv(ep_b).unwrap();
    events.emit(
        EventKind::IpcRecv,
        got.header.badge,
        got.header.payload_len as u64,
    );
    let ipc_ok = got.payload() == b"ping-fabric"
        && got.header.badge == 0xA3
        && got.header.route.tile == 0
        && caps_a
            .require(ep_cap_a, CapKind::Endpoint, CapRights::WRITE)
            .is_ok();

    // Two banks: 16MiB @ 16MiB and 24MiB (demo window, not real DRAM map).
    let mut arenas = ArenaAllocator::new(&[
        (BankId(0), PhysAddr(0x0100_0000), 8 * 1024 * 1024),
        (BankId(1), PhysAddr(0x0180_0000), 8 * 1024 * 1024),
    ])
    .unwrap();
    let here = Place::new(ChipletId(0), MemorySpace::TileSram).with_tile(2);
    let arena = arenas
        .alloc(
            ArenaRequest::tensor(64 * 1024, Some(BankId(0)))
                .in_space(MemorySpace::TileSram)
                .for_tenant(tenant_a),
        )
        .unwrap();
    events.emit(EventKind::ArenaAlloc, arena.base.0, arena.size);
    arenas
        .transfer_owner(arena.id, None, 2, tenant_a.0)
        .unwrap();
    events.emit(EventKind::ArenaXfer, arena.id.0 as u64, 2);

    let mem_cap = caps_a
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            arena.id.0,
            tenant_a,
        ))
        .unwrap();
    let mut iommu = IommuMap::new();
    let pin = iommu
        .map(
            caps_a.lookup(mem_cap).unwrap(),
            MapRequest::pin(arena.base, arena.size),
        )
        .unwrap();
    let no_map_cap = Capability::new(
        CapKind::Memory,
        CapRights(CapRights::READ | CapRights::WRITE),
        arena.id.0,
        tenant_a,
    )
    .with_generation(1);
    let map_refused = iommu.map(&no_map_cap, MapRequest::pin(PhysAddr(0x2000_0000), 0x1000))
        == Err(MapError::NoMemoryCap);
    let map_ok = pin.iova != arena.base
        && iommu.translate(arena.base) == Some(pin.iova)
        && iommu.covers(arena.base, 64)
        && map_refused
        && IommuMap::check_cap(caps_a.lookup(mem_cap).unwrap()).is_ok();

    // Isolation: tenant B must not hold a cap to A's arena.
    let isolation_ok = !caps_b.holds(CapKind::Memory, arena.id.0)
        && caps_b
            .require(mem_cap, CapKind::Memory, CapRights::READ)
            .is_err();
    if isolation_ok {
        events.emit(
            EventKind::IsolationDeny,
            tenant_b.0 as u64,
            arena.id.0 as u64,
        );
    }

    // Grant a read+map view to the NPU queue owner (still tenant A).
    let granted = caps_a
        .derive(
            mem_cap,
            CapRights(CapRights::READ | CapRights::WRITE | CapRights::MAP),
        )
        .is_ok();
    events.emit(EventKind::CapGrant, arena.id.0 as u64, granted as u64);

    // CDT: mint a dedicated Memory parent, derive in-table, GRANT a child to B,
    // revoke parent across both tables. Unrelated caps (B's endpoint, A's
    // arena Memory) must survive.
    let cdt_parent = caps_a
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            0xCD7,
            tenant_a,
        ))
        .unwrap();
    let cdt_child = caps_a
        .derive(cdt_parent, CapRights(CapRights::READ | CapRights::MAP))
        .unwrap();
    let cdt_b = caps_a
        .transfer(cdt_parent, &mut caps_b, CapRights(CapRights::READ), false)
        .unwrap();
    caps_a.revoke_in(cdt_parent, &mut [&mut caps_b]).unwrap();
    let revoke_ok = caps_a.lookup(cdt_parent).is_err()
        && caps_a
            .require(cdt_child, CapKind::Memory, CapRights::READ)
            .is_err()
        && caps_b
            .require(cdt_b, CapKind::Memory, CapRights::READ)
            .is_err()
        && caps_a
            .require(mem_cap, CapKind::Memory, CapRights::READ)
            .is_ok()
        && caps_b
            .require(ep_cap_b, CapKind::Endpoint, CapRights::READ)
            .is_ok();
    if revoke_ok {
        events.emit(EventKind::CapRevoke, 0xCD7, cdt_child.0 as u64);
    }
    let remote = FabricAddr::new(Place::new(ChipletId(1), MemorySpace::CxlRegion), 0x2000);
    let silent = map_place(here, remote);
    if silent == Err(SpaceError::SilentRemoteLoad) {
        events.emit(EventKind::SpaceRefuse, 1, 0);
    }
    let local_ok = map_place(here, FabricAddr::new(here, arena.base.0)).is_ok();
    let unified_default = CapRights::MEM_FULL.contains(CapRights::UNIFIED);
    let arena_ok = arena.pinned && arena.dma && arena.bank == BankId(0) && granted;
    let space_ok = arena.space == MemorySpace::TileSram
        && silent == Err(SpaceError::SilentRemoteLoad)
        && local_ok
        && !unified_default;

    let (graph, cut) = SpectralCut::qemu_chiplet_cut(400).unwrap();
    let cut_cap = caps_a
        .mint(Capability::new(
            CapKind::SpectralCut,
            CapRights::CUT_FULL,
            cut.id.0,
            tenant_a,
        ))
        .unwrap();
    events.emit(EventKind::CutBind, cut.id.0 as u64, cut.phi_milli as u64);
    let place_ok = bind_place(&caps_a, cut_cap, &cut, &graph, TileId(2), Some(BankId(0))).is_ok();
    let cross = bind_place(&caps_a, cut_cap, &cut, &graph, TileId(1), Some(BankId(0)));
    let cut_refuse = cross == Err(CutError::CrossCut);
    if cut_refuse {
        events.emit(EventKind::CutRefuse, 1, 0);
    }
    // Tenant B cannot bind A's cut (no cap).
    let b_no_cut = !caps_b.holds(CapKind::SpectralCut, cut.id.0);
    let cut_ok = place_ok && cut_refuse && b_no_cut && cut.phi_milli <= cut.bound_milli;

    let part = PartitionProfile::new(
        PartitionId(1),
        SpatialSlice::single_chiplet(ChipletId(0), (1 << 0) | (1 << 2), 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 2,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );
    let part_cap = part.mint(&mut caps_a).unwrap();
    let act = Activity::new(ActivityId(1), ActivityKind::VirtAccel, ep_a).bind_partition(part.id);
    let act_cap = act.publish(&mut caps_a).unwrap();
    events.emit(
        EventKind::ActivityBind,
        act.id.0 as u64,
        act.endpoint.0 as u64,
    );
    let activity_ok = caps_a
        .require(act_cap, CapKind::Activity, CapRights::SUBMIT)
        .is_ok()
        && caps_a
            .require(part_cap, CapKind::Partition, CapRights::BIND)
            .is_ok()
        && !caps_b.holds(CapKind::Activity, act.id.0)
        && act.kind == ActivityKind::VirtAccel;

    let mut timeline = Timeline::new(part.id);
    let fence = timeline.submit(&part, None).unwrap();
    events.emit(EventKind::FenceSubmit, fence.id.0, part.id.0 as u64);
    let wait_before = timeline.wait(fence.id) == Err(PartitionError::FenceNotReady);

    let mut sched = TileScheduler::new();
    sched.set_graph(graph);
    sched.install_cut(cut);
    sched.bind_partition(part);
    sched.add_tile(TileId(0), TileKind::Cpu, BankId(0));
    sched.add_tile(TileId(1), TileKind::Cpu, BankId(1));
    sched.add_tile(TileId(2), TileKind::Npu, BankId(0));
    sched.enqueue(Job {
        id: 0,
        kind: JobKind::Thread,
        tile_hint: Some(TileId(0)),
        bank_affinity: Some(BankId(0)),
        priority: 3,
        deadline_ticks: Some(1_000),
        tenant: tenant_a.0,
        cut_id: Some(cut.id.0),
        phase: Phase::Compute,
        partition_id: Some(part.id.0),
        fence_id: Some(fence.id.0),
        arena_color: Some(arena.color),
    });
    sched.enqueue(Job {
        id: 0,
        kind: JobKind::AccelWave,
        tile_hint: Some(TileId(2)),
        bank_affinity: Some(BankId(0)),
        priority: 0,
        deadline_ticks: Some(50),
        tenant: tenant_a.0,
        cut_id: Some(cut.id.0),
        phase: Phase::Compute,
        partition_id: Some(part.id.0),
        fence_id: Some(fence.id.0),
        arena_color: Some(arena.color),
    });
    let wave = sched.pick(TileId(2));
    events.emit(
        EventKind::SchedPick,
        wave.map(|j| j.id).unwrap_or(0) as u64,
        2,
    );
    let cpu = sched.pick(TileId(0));
    let sched_ok = wave.is_some() && cpu.is_some();

    // Backing store for the software NPU (host and kernel both use this path
    // when they do not have a real identity-mapped PA). 4×4 i32 × 3 matrices.
    let mut backing = [0u8; 256];
    let ident = [1i32, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
    for (i, v) in ident.iter().enumerate() {
        backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in DEMO_B.iter().enumerate() {
        backing[64 + i * 4..64 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut mem = SliceMem {
        base: PhysAddr(0),
        bytes: &mut backing,
    };
    let job = AccelJobDesc {
        op: AccelOp::MatMul,
        flags: 0,
        m: 4,
        n: 4,
        k: 4,
        a: PhysAddr(0),
        b: PhysAddr(64),
        c: PhysAddr(128),
        bias: PhysAddr(0),
        a_stride: 4,
        b_stride: 4,
        c_stride: 4,
        dtype: crate::accel::DType::I32,
        tenant: tenant_a.0,
        completion_ep: ep_a.0,
        space: MemorySpace::TileSram,
        place: here,
        phase: Phase::Compute,
        partition: part.id,
        fence_id: fence.id.0,
    };
    let qcap = caps_a
        .mint(Capability::new(
            CapKind::AccelQueue,
            CapRights::ACCEL_FULL,
            1,
            tenant_a,
        ))
        .unwrap();
    let _ = caps_a.require(qcap, CapKind::AccelQueue, CapRights::SUBMIT);

    events.emit(EventKind::AccelSubmit, 4, 4);
    let mut npu = SoftNpu::new();
    let cpl = npu.execute(&job, &mut mem).unwrap();
    events.emit(
        EventKind::AccelComplete,
        cpl.job_seq as u64,
        cpl.cycles as u64,
    );
    let fence_done = timeline.complete(fence.id).unwrap();
    events.emit(EventKind::FenceComplete, fence_done.id.0, 1);
    let wait_after = timeline.wait(fence.id).unwrap();
    let color_ok = admit_arena_wave(
        tenant_a.0,
        Phase::Compute,
        arenas.get(arena.id).unwrap(),
        BankId(0),
    )
    .is_ok()
        && admit_arena_wave(
            tenant_b.0,
            Phase::Compute,
            arenas.get(arena.id).unwrap(),
            BankId(0),
        ) == Err(ColorError::ForeignTenant)
        && admit_arena_wave(
            tenant_a.0,
            Phase::Compute,
            arenas.get(arena.id).unwrap(),
            BankId(1),
        ) == Err(ColorError::ForeignBank)
        && admit_arena_wave(
            tenant_a.0,
            Phase::Exchange,
            arenas.get(arena.id).unwrap(),
            BankId(1),
        )
        .is_ok();

    let fence_ok = fence.submitted
        && wait_before
        && fence_done.completed
        && wait_after.completed
        && !fence_done.timed_out
        && job.phase == Phase::Compute
        && job.space == MemorySpace::TileSram
        && timeline.retired() == fence.seq()
        && timeline.in_flight() == 0;

    let hodge_cap = caps_a
        .mint(
            Capability::new(CapKind::FlowQuota, CapRights::HODGE_FULL, 1, tenant_a)
                .with_badge(CLASS_ALL),
        )
        .unwrap();
    let hodge_grad = authorize(caps_a.lookup(hodge_cap).unwrap(), FlowClass::Gradient).is_ok();
    // Tenant B has no FlowQuota cap: cannot authorize harmonic.
    let b_no_hodge = !caps_b.holds(CapKind::FlowQuota, 1);

    let grad_msg = Message::new(
        ep_a,
        0x11,
        MsgFlags(MsgFlags::ASYNC | MsgFlags::TREE_OFFLOAD),
        ChipletRoute::for_tile(TileId(0)),
        tenant_a,
        b"allreduce",
    )
    .unwrap()
    .with_flow(FlowClass::Gradient)
    .with_phase(Phase::Exchange);
    let grad_ok = fabric.send(grad_msg).is_ok();
    events.emit(EventKind::HodgeAdmit, FlowClass::Gradient as u64, 1);
    let _ = fabric.recv(ep_a);

    let curl_msg = Message::new(
        ep_a,
        0x22,
        MsgFlags(MsgFlags::ASYNC | MsgFlags::RING_RESERVE),
        ChipletRoute::for_tile(TileId(2)),
        tenant_a,
        b"ring",
    )
    .unwrap()
    .with_flow(FlowClass::Curl);
    let curl_ok = fabric.send(curl_msg).is_ok()
        && authorize(caps_a.lookup(hodge_cap).unwrap(), FlowClass::Curl).is_ok();
    let _ = fabric.recv(ep_a);

    let harm_tree = Message::new(
        ep_a,
        0x33,
        MsgFlags(MsgFlags::ASYNC | MsgFlags::TREE_OFFLOAD),
        ChipletRoute::LOCAL,
        tenant_a,
        b"cycle",
    )
    .unwrap()
    .with_flow(FlowClass::Harmonic);
    let harm_refused =
        fabric.send(harm_tree) == Err(FabricError::Hodge(HodgeError::HarmonicTreeReduce));
    events.emit(EventKind::HodgeRefuse, FlowClass::Harmonic as u64, 1);
    let hodge_ok = hodge_grad
        && grad_ok
        && curl_ok
        && harm_refused
        && b_no_hodge
        && authorize(
            &Capability::new(CapKind::FlowQuota, CapRights::HODGE_FULL, 1, tenant_b)
                .with_badge(CLASS_GRADIENT | CLASS_CURL)
                .with_generation(1),
            FlowClass::Harmonic,
        )
        .is_err();

    // OperatorKernelHandle: tree+gradient injects; tree+harmonic bind refuses;
    // torus+harmonic injects without TREE_OFFLOAD. Tenant B holds no cap.
    let tree = OperatorKernelHandle::bind(OpKernelId(1), CollectiveKind::Tree, FlowClass::Gradient)
        .unwrap();
    let tree_cap = tree.mint(&mut caps_a).unwrap();
    let tree_inject = tree
        .inject(&caps_a, tree_cap, &mut fabric, ep_a, tenant_a, b"ok-tree")
        .is_ok();
    let tree_got = fabric.recv(ep_a).unwrap();
    events.emit(EventKind::HodgeAdmit, FlowClass::Gradient as u64, 2);
    let harm_bind_refused =
        OperatorKernelHandle::bind(OpKernelId(2), CollectiveKind::Tree, FlowClass::Harmonic)
            .is_err();
    let wrong_class = tree.inject_as(
        &caps_a,
        tree_cap,
        &mut fabric,
        ep_a,
        tenant_a,
        b"nope",
        FlowClass::Harmonic,
    ) == Err(OpKernelError::ClassMismatch);
    let torus =
        OperatorKernelHandle::bind(OpKernelId(3), CollectiveKind::Torus, FlowClass::Harmonic)
            .unwrap();
    let torus_cap = torus.mint(&mut caps_a).unwrap();
    let torus_inject = torus
        .inject(&caps_a, torus_cap, &mut fabric, ep_a, tenant_a, b"ok-torus")
        .is_ok();
    let torus_got = fabric.recv(ep_a).unwrap();
    let opkernel_ok = tree_inject
        && tree_got.header.flags.tree_offload()
        && tree_got.header.flow == FlowClass::Gradient
        && harm_bind_refused
        && wrong_class
        && torus_inject
        && !torus_got.header.flags.tree_offload()
        && torus_got.header.flow == FlowClass::Harmonic
        && !caps_b.holds(CapKind::OperatorKernel, 1);

    // SparsifiedCollective: below-threshold harmonic is dropped; at-threshold
    // is kept; Gradient energy is ignored; Harmonic+TREE still refuses.
    let dropped =
        torus
            .sparsify(100, 500)
            .inject(&caps_a, torus_cap, &mut fabric, ep_a, tenant_a, b"tiny")
            == Ok(SparsifyAction::Drop)
            && fabric.pending(ep_a).unwrap() == 0;
    let kept =
        torus
            .sparsify(500, 500)
            .inject(&caps_a, torus_cap, &mut fabric, ep_a, tenant_a, b"kept")
            == Ok(SparsifyAction::Keep);
    let keep_got = fabric.recv(ep_a).unwrap();
    let grad_pass =
        tree.sparsify(0, 9999)
            .inject(&caps_a, tree_cap, &mut fabric, ep_a, tenant_a, b"grad")
            == Ok(SparsifyAction::Keep);
    let grad_got = fabric.recv(ep_a).unwrap();
    let harm_tree_still = decide_header(CollectiveKind::Tree, FlowClass::Harmonic, 1, 500)
        == Err(HodgeError::HarmonicTreeReduce);
    let header_drop = SparsifiedCollective::from_header(
        OpKernelId(11),
        CollectiveKind::Torus,
        FlowClass::Harmonic,
        100,
        500,
    )
    .unwrap()
    .decide()
        == Ok(SparsifyAction::Drop);
    let sparsify_ok = dropped
        && kept
        && keep_got.header.flow == FlowClass::Harmonic
        && !keep_got.header.flags.tree_offload()
        && grad_pass
        && grad_got.header.flow == FlowClass::Gradient
        && grad_got.header.flags.tree_offload()
        && harm_tree_still
        && header_drop;

    fabric
        .send(
            Message::new(
                ep_a,
                cpl.job_seq as u64,
                MsgFlags(MsgFlags::ASYNC | MsgFlags::REPLY),
                ChipletRoute::for_tile(TileId(2)),
                tenant_a,
                b"accel-done",
            )
            .unwrap(),
        )
        .ok();

    let done = fabric.recv(ep_a).unwrap();
    let c00 = i32::from_le_bytes(backing[128..132].try_into().unwrap());
    let c11 = i32::from_le_bytes(backing[128 + 20..128 + 24].try_into().unwrap());
    let accel_ok = cpl.status == 0
        && done.payload() == b"accel-done"
        && c00 == DEMO_B[0]
        && c11 == DEMO_B[5]
        && npu.jobs_retired == 1
        && job.fence_id == fence.id.0;
    let dtype_ok = demo_f16_f32_ok();

    DemoReport {
        ipc_ok,
        arena_ok,
        accel_ok,
        isolation_ok,
        sched_ok,
        cut_ok,
        hodge_ok,
        space_ok,
        activity_ok,
        fence_ok,
        map_ok,
        color_ok,
        revoke_ok,
        opkernel_ok,
        sparsify_ok,
        dtype_ok,
        job_seq: cpl.job_seq,
        c00,
        c11,
        arena_base: arena.base.0,
        arena_bank: arena.bank.0,
        events: events.len() as u32,
        cut_phi_milli: cut.phi_milli,
        fence_id: fence.id.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_demo_succeeds() {
        let r = run_boot_demo();
        assert!(r.ipc_ok, "ipc");
        assert!(r.arena_ok, "arena");
        assert!(r.accel_ok, "accel");
        assert!(r.isolation_ok, "isolation");
        assert!(r.sched_ok, "sched");
        assert!(r.cut_ok, "cut");
        assert!(r.hodge_ok, "hodge");
        assert!(r.space_ok, "space");
        assert!(r.activity_ok, "activity");
        assert!(r.fence_ok, "fence");
        assert!(r.map_ok, "map");
        assert!(r.color_ok, "color");
        assert!(r.revoke_ok, "cdt revoke");
        assert!(r.opkernel_ok, "opkernel");
        assert!(r.sparsify_ok, "sparsify");
        assert!(r.dtype_ok, "f16/f32 soft-float");
        assert!(r.all_ok());
        assert!(r.fence_id > 0);
        assert!(r.cut_phi_milli > 0 && r.cut_phi_milli <= 400);
        assert_eq!(r.c00, 2);
        assert_eq!(r.c11, 13);
        assert_eq!(r.arena_bank, 0);
    }
}
