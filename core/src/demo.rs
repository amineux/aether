//! Shared boot-demo: fabric IPC + tensor arena + SoftNPU matmul.
//!
//! The kernel prints this report; host tests assert the same path.

use crate::accel::{AccelJobDesc, AccelOp, SliceMem, SoftNpu};
use crate::arena::{ArenaAllocator, ArenaRequest};
use crate::caps::{CapKind, CapRights, CapTable, Capability};
use crate::fabric::{ChipletRoute, Fabric, Message, MsgFlags};
use crate::observe::{EventKind, EventRing};
use crate::sched::{Job, JobKind, TileKind, TileScheduler};
use crate::types::{BankId, PhysAddr, TenantId, TileId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemoReport {
    pub ipc_ok: bool,
    pub arena_ok: bool,
    pub accel_ok: bool,
    pub isolation_ok: bool,
    pub sched_ok: bool,
    pub job_seq: u32,
    pub c00: i32,
    pub c11: i32,
    pub arena_base: u64,
    pub arena_bank: u8,
    pub events: u32,
}

impl DemoReport {
    pub fn all_ok(&self) -> bool {
        self.ipc_ok && self.arena_ok && self.accel_ok && self.isolation_ok && self.sched_ok
    }
}

/// 4×4 identity @ known matrix. C must equal B.
pub const DEMO_B: [i32; 16] = [
    2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53,
];

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
        .mint(Capability {
            kind: CapKind::Endpoint,
            rights: CapRights::EP_FULL,
            object: ep_a.0,
            badge: 0xA3,
            generation: 0,
            tenant: tenant_a,
        })
        .unwrap();
    let _ep_cap_b = caps_b
        .mint(Capability {
            kind: CapKind::Endpoint,
            rights: CapRights::EP_FULL,
            object: ep_b.0,
            badge: 0xB7,
            generation: 0,
            tenant: tenant_b,
        })
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
    events.emit(EventKind::IpcRecv, got.header.badge, got.header.payload_len as u64);
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
    let arena = arenas
        .alloc(ArenaRequest::tensor(64 * 1024, Some(BankId(0))))
        .unwrap();
    events.emit(EventKind::ArenaAlloc, arena.base.0, arena.size);
    arenas
        .transfer_owner(arena.id, None, 2, tenant_a.0)
        .unwrap();
    events.emit(EventKind::ArenaXfer, arena.id.0 as u64, 2);

    let mem_cap = caps_a
        .mint(Capability {
            kind: CapKind::Memory,
            rights: CapRights::MEM_FULL,
            object: arena.id.0,
            badge: 0,
            generation: 0,
            tenant: tenant_a,
        })
        .unwrap();

    // Isolation: tenant B must not hold a cap to A's arena.
    let isolation_ok = !caps_b.holds(CapKind::Memory, arena.id.0)
        && caps_b
            .require(mem_cap, CapKind::Memory, CapRights::READ)
            .is_err();
    if isolation_ok {
        events.emit(EventKind::IsolationDeny, tenant_b.0 as u64, arena.id.0 as u64);
    }

    // Grant a read+map view to the NPU queue owner (still tenant A) — then
    // move write into an accel-queue cap's world by transferring a derived cap.
    let granted = caps_a
        .derive(
            mem_cap,
            CapRights(CapRights::READ | CapRights::WRITE | CapRights::MAP),
        )
        .is_ok();
    events.emit(EventKind::CapGrant, arena.id.0 as u64, granted as u64);
    let arena_ok = arena.pinned && arena.dma && arena.bank == BankId(0) && granted;

    let mut sched = TileScheduler::new();
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
    });
    sched.enqueue(Job {
        id: 0,
        kind: JobKind::AccelWave,
        tile_hint: Some(TileId(2)),
        bank_affinity: Some(BankId(0)),
        priority: 0,
        deadline_ticks: Some(50),
        tenant: tenant_a.0,
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
    let ident = [
        1i32, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1,
    ];
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
    };
    let qcap = caps_a
        .mint(Capability {
            kind: CapKind::AccelQueue,
            rights: CapRights::ACCEL_FULL,
            object: 1,
            badge: 0,
            generation: 0,
            tenant: tenant_a,
        })
        .unwrap();
    let _ = caps_a.require(qcap, CapKind::AccelQueue, CapRights::SUBMIT);

    events.emit(EventKind::AccelSubmit, 4, 4);
    let mut npu = SoftNpu::new();
    let cpl = npu.execute(&job, &mut mem).unwrap();
    events.emit(EventKind::AccelComplete, cpl.job_seq as u64, cpl.cycles as u64);

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
        && npu.jobs_retired == 1;

    DemoReport {
        ipc_ok,
        arena_ok,
        accel_ok,
        isolation_ok,
        sched_ok,
        job_seq: cpl.job_seq,
        c00,
        c11,
        arena_base: arena.base.0,
        arena_bank: arena.bank.0,
        events: events.len() as u32,
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
        assert!(r.all_ok());
        assert_eq!(r.c00, 2);
        assert_eq!(r.c11, 13);
        assert_eq!(r.arena_bank, 0);
    }
}
