//! Two-tenant blast-radius diligence clip — not an OS track.
//!
//! Tenant A and B each mint Memory, Activity, and SpectralCut. The clip
//! proves: A cannot name B's objects; CrossCut placement is refused;
//! Soft SMMU wrong SID aborts. Host tests and the kernel self-check
//! run the same path. Serial: `[blast] …`. No new EventKind, no hops
//! theater, no FLOP claim.

use crate::activity::{Activity, ActivityId, ActivityKind};
use crate::arena::{ArenaAllocator, ArenaRequest};
use crate::caps::{CapError, CapKind, CapRights, CapTable, Capability};
use crate::cut::{bind_place, CutError, SpectralCut};
use crate::fabric::Fabric;
use crate::iommu::{IommuMap, MapError, MapRequest, StreamId};
use crate::observe::{EventKind, EventRing};
use crate::space::MemorySpace;
use crate::types::{BankId, ChipletId, PhysAddr, TenantId, TileId};

/// A's NPU stream vs B's GPU stream (distinct STEs).
pub const SID_A: u32 = StreamId::accel(ChipletId(0), TileId(2), 0).0;
pub const SID_B: u32 = StreamId::accel(ChipletId(1), TileId(3), 0).0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlastReport {
    pub mem_ok: bool,
    pub activity_ok: bool,
    pub cut_ok: bool,
    pub smmu_ok: bool,
    pub events: u32,
}

impl BlastReport {
    pub fn all_ok(&self) -> bool {
        self.mem_ok && self.activity_ok && self.cut_ok && self.smmu_ok
    }
}

/// Host-identical two-tenant refuse story. Kernel prints `[blast] …`.
pub fn run_blast_demo() -> BlastReport {
    let mut events = EventRing::new();

    let tenant_a = TenantId(1);
    let tenant_b = TenantId(2);
    let mut caps_a = CapTable::new(tenant_a);
    let mut caps_b = CapTable::new(tenant_b);

    let mut fabric = Fabric::new();
    let ep_a = fabric.create_endpoint(tenant_a).unwrap();
    let ep_b = fabric.create_endpoint(tenant_b).unwrap();

    let mut arenas = ArenaAllocator::new(&[
        (BankId(0), PhysAddr(0x0100_0000), 8 * 1024 * 1024),
        (BankId(1), PhysAddr(0x0180_0000), 8 * 1024 * 1024),
    ])
    .unwrap();
    let arena_a = arenas
        .alloc(
            ArenaRequest::tensor(64 * 1024, Some(BankId(0)))
                .in_space(MemorySpace::TileSram)
                .for_tenant(tenant_a),
        )
        .unwrap();
    let arena_b = arenas
        .alloc(
            ArenaRequest::tensor(64 * 1024, Some(BankId(1)))
                .in_space(MemorySpace::TileSram)
                .for_tenant(tenant_b),
        )
        .unwrap();

    let mem_a = caps_a
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            arena_a.id.0,
            tenant_a,
        ))
        .unwrap();
    let mem_b = caps_b
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            arena_b.id.0,
            tenant_b,
        ))
        .unwrap();
    let mint_cross = caps_b.mint(Capability::new(
        CapKind::Memory,
        CapRights::MEM_FULL,
        arena_a.id.0,
        tenant_a,
    ));
    let mem_ok = mint_cross == Err(CapError::CrossTenant)
        && !caps_b.holds(CapKind::Memory, arena_a.id.0)
        && !caps_a.holds(CapKind::Memory, arena_b.id.0)
        && caps_a
            .require(mem_a, CapKind::Memory, CapRights::MAP)
            .is_ok()
        && caps_b
            .require(mem_b, CapKind::Memory, CapRights::MAP)
            .is_ok();
    if mem_ok {
        events.emit(
            EventKind::IsolationDeny,
            tenant_b.0 as u64,
            arena_a.id.0 as u64,
        );
    }

    let act_a = Activity::new(ActivityId(1), ActivityKind::VirtAccel, ep_a);
    let act_b = Activity::new(ActivityId(2), ActivityKind::VirtAccel, ep_b);
    let act_cap_a = act_a.publish(&mut caps_a).unwrap();
    let act_cap_b = act_b.publish(&mut caps_b).unwrap();
    let activity_ok = caps_a
        .require(act_cap_a, CapKind::Activity, CapRights::SUBMIT)
        .is_ok()
        && caps_b
            .require(act_cap_b, CapKind::Activity, CapRights::SUBMIT)
            .is_ok()
        && !caps_b.holds(CapKind::Activity, act_a.id.0)
        && !caps_a.holds(CapKind::Activity, act_b.id.0);
    if activity_ok {
        events.emit(
            EventKind::IsolationDeny,
            act_a.id.0 as u64,
            act_b.id.0 as u64,
        );
    }

    let (graph, cut) = SpectralCut::qemu_chiplet_cut(400).unwrap();
    let cut_cap_a = caps_a
        .mint(Capability::new(
            CapKind::SpectralCut,
            CapRights::CUT_FULL,
            cut.id.0,
            tenant_a,
        ))
        .unwrap();
    let place_ok = bind_place(&caps_a, cut_cap_a, &cut, &graph, TileId(2), Some(BankId(0))).is_ok();
    let cross = bind_place(&caps_a, cut_cap_a, &cut, &graph, TileId(1), Some(BankId(0)));
    let cut_ok = place_ok
        && cross == Err(CutError::CrossCut)
        && !caps_b.holds(CapKind::SpectralCut, cut.id.0)
        && bind_place(&caps_b, cut_cap_a, &cut, &graph, TileId(2), Some(BankId(0)))
            == Err(CutError::NotBound);
    if cut_ok {
        events.emit(EventKind::CutRefuse, TileId(1).0 as u64, BankId(0).0 as u64);
    }

    let mut iommu = IommuMap::new();
    let sid_a = StreamId::from_raw(SID_A);
    let sid_b = StreamId::from_raw(SID_B);
    let pin_a = iommu
        .map(
            caps_a.lookup(mem_a).unwrap(),
            MapRequest::pin_accel(arena_a.base, arena_a.size, sid_a),
        )
        .unwrap();
    let abort = iommu.walk(sid_b, pin_a.iova) == Err(MapError::StreamAbort);
    let pin_b = iommu
        .map(
            caps_b.lookup(mem_b).unwrap(),
            MapRequest::pin_accel(arena_b.base, arena_b.size, sid_b),
        )
        .unwrap();
    let wrong = iommu.resolve_result(SID_B, pin_a.iova, None) == Err(MapError::WrongStream);
    let smmu_ok = abort
        && wrong
        && iommu.walk(sid_a, pin_a.iova).map(|w| w.pa) == Ok(arena_a.base)
        && iommu.walk(sid_b, pin_b.iova).map(|w| w.pa) == Ok(arena_b.base);
    if smmu_ok {
        events.emit(EventKind::IsolationDeny, SID_B as u64, pin_a.iova.0);
    }

    BlastReport {
        mem_ok,
        activity_ok,
        cut_ok,
        smmu_ok,
        events: events.len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blast_demo_two_tenants_refuse() {
        let r = run_blast_demo();
        assert!(r.mem_ok, "memory");
        assert!(r.activity_ok, "activity");
        assert!(r.cut_ok, "crosscut");
        assert!(r.smmu_ok, "wrong SID");
        assert!(r.all_ok());
        assert!(r.events >= 3);
        assert_ne!(SID_A, SID_B);
    }
}
