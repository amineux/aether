//! Host1x-shaped SID-at-submit clip — not a Tegra driver.
//!
//! Soft SMMU already binds StreamIDs at map. This clip programs /
//! validates SET_SID at the job head **before** DMA: two tenants, two
//! SIDs, refuse until the submit SID is armed, wrong-SID abort, per-tenant
//! budget. Host tests and the kernel self-check run the same path.
//! Serial: `[sid] …`.
//!
//! This is **not** hardware-grade isolation. The budget and fault
//! injection are software. A real Host1x / SMMU still needs partner
//! silicon.

use crate::caps::{CapKind, CapRights, Capability};
use crate::iommu::{IommuMap, MapError, MapRequest, StreamId, SID_BUDGET_PER_TENANT};
use crate::types::{ChipletId, PhysAddr, TenantId, TileId};

/// Soft-CP-shaped SSIDs (ssid = 1), distinct STEs. Not blast's ssid 0.
pub const SID_SUBMIT_A: u32 = StreamId::accel(ChipletId(0), TileId(2), 1).0;
pub const SID_SUBMIT_B: u32 = StreamId::accel(ChipletId(1), TileId(3), 1).0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidSubmitReport {
    pub abort_until_set: bool,
    pub two_sids: bool,
    pub wrong_sid: bool,
    pub cross_tenant: bool,
    pub budget_ok: bool,
}

impl SidSubmitReport {
    pub fn all_ok(&self) -> bool {
        self.abort_until_set
            && self.two_sids
            && self.wrong_sid
            && self.cross_tenant
            && self.budget_ok
    }
}

fn mem_cap(obj: u32, tenant: TenantId) -> Capability {
    Capability::new(CapKind::Memory, CapRights::MEM_FULL, obj, tenant).with_generation(1)
}

/// Host-identical SET_SID / two-SID refuse story. Kernel prints `[sid] …`.
pub fn run_sid_submit_demo() -> SidSubmitReport {
    let mut iommu = IommuMap::new();
    let cap_a = mem_cap(1, TenantId(1));
    let cap_b = mem_cap(2, TenantId(2));
    let sid_a = StreamId::from_raw(SID_SUBMIT_A);
    let sid_b = StreamId::from_raw(SID_SUBMIT_B);

    let pin_a = iommu
        .map(
            &cap_a,
            MapRequest::pin_accel(PhysAddr(0x0100_0000), 0x1000, sid_a),
        )
        .unwrap();
    let abort_until_set = iommu.resolve_submit(sid_a.raw(), pin_a.iova, None)
        == Err(MapError::SubmitSid)
        && iommu.walk(sid_a, pin_a.iova).is_ok();

    iommu.set_sid(&cap_a, sid_a).unwrap();
    let ok_a = iommu.resolve_submit(sid_a.raw(), pin_a.iova, None) == Ok(PhysAddr(0x0100_0000));

    let pin_b = iommu
        .map(
            &cap_b,
            MapRequest::pin_accel(PhysAddr(0x0180_0000), 0x1000, sid_b),
        )
        .unwrap();
    let wrong_while_a =
        iommu.resolve_submit(sid_b.raw(), pin_b.iova, None) == Err(MapError::WrongStream);
    let cross_tenant = iommu.set_sid(&cap_a, sid_b) == Err(MapError::CrossTenant);

    iommu.set_sid(&cap_b, sid_b).unwrap();
    let ok_b = iommu.resolve_submit(sid_b.raw(), pin_b.iova, None) == Ok(PhysAddr(0x0180_0000));
    let wrong_while_b =
        iommu.resolve_submit(sid_a.raw(), pin_a.iova, None) == Err(MapError::WrongStream);

    let two_sids = ok_a && ok_b && sid_a != sid_b;
    let wrong_sid = wrong_while_a && wrong_while_b;

    // Tenant A already holds one SID; fill the rest of the budget.
    let mut budget_ok = iommu.tenant_sid_count(TenantId(1)) == 1;
    for i in 0..(SID_BUDGET_PER_TENANT - 1) {
        let extra = StreamId::accel(ChipletId(2), TileId(i as u16), 1);
        budget_ok &= iommu.bind_stream(&cap_a, extra).is_ok();
    }
    budget_ok &= iommu.sid_budget_left(TenantId(1)) == 0;
    budget_ok &= iommu.bind_stream(&cap_a, StreamId::accel(ChipletId(3), TileId(0), 1))
        == Err(MapError::SidBudget);
    // Tenant B still has budget (one SID used).
    budget_ok &= iommu.sid_budget_left(TenantId(2)) == SID_BUDGET_PER_TENANT - 1;

    SidSubmitReport {
        abort_until_set,
        two_sids,
        wrong_sid,
        cross_tenant,
        budget_ok,
    }
}


/// Host red-team report for Soft-SMMU SET_SID-at-submit refuse.
///
/// Sell line `[redteam] attack=submit-sid` — existing [`IommuMap::resolve_submit`]
/// path only. Bound stream without armed SET_SID latch → [`MapError::SubmitSid`].
/// Plain `walk` still admits (DMA map path). **Not** set-sid-unbound
/// (`StreamAbort` / Soft-CP Fault), **not** xqueue-sid-override / PASID / SidBudget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmitSidReport {
    /// `resolve_submit` before SET_SID → `MapError::SubmitSid`.
    pub abort_until_set: bool,
    /// Bound `walk` without SET_SID still admits (distinct from StreamAbort).
    pub walk_without_set_ok: bool,
    /// After `set_sid`, `resolve_submit` admits.
    pub after_set_ok: bool,
}

impl SubmitSidReport {
    pub fn all_ok(&self) -> bool {
        self.abort_until_set && self.walk_without_set_ok && self.after_set_ok
    }
}

/// Soft-SMMU `resolve_submit` without SET_SID → [`MapError::SubmitSid`].
/// Host1x-shaped SID-at-submit foundation — not Soft-CP unbound Fault,
/// not SidBudget / xqueue override / PASID.
pub fn run_submit_sid_demo() -> SubmitSidReport {
    let mut iommu = IommuMap::new();
    let cap = mem_cap(1, TenantId(1));
    let sid = StreamId::from_raw(SID_SUBMIT_A);

    let pin = iommu
        .map(
            &cap,
            MapRequest::pin_accel(PhysAddr(0x0100_0000), 0x1000, sid),
        )
        .unwrap();

    let abort_until_set =
        iommu.resolve_submit(sid.raw(), pin.iova, None) == Err(MapError::SubmitSid);
    let walk_without_set_ok = iommu.walk(sid, pin.iova).is_ok();

    iommu.set_sid(&cap, sid).unwrap();
    let after_set_ok =
        iommu.resolve_submit(sid.raw(), pin.iova, None) == Ok(PhysAddr(0x0100_0000));

    SubmitSidReport {
        abort_until_set,
        walk_without_set_ok,
        after_set_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sid_submit_demo_two_tenants() {
        let r = run_sid_submit_demo();
        assert!(r.abort_until_set, "refuse until SET_SID");
        assert!(r.two_sids, "two SIDs");
        assert!(r.wrong_sid, "wrong SID");
        assert!(r.cross_tenant, "cross-tenant SET_SID");
        assert!(r.budget_ok, "SID budget");
        assert!(r.all_ok());
        assert_ne!(SID_SUBMIT_A, SID_SUBMIT_B);
    }
    #[test]
    fn submit_sid_demo_refuses_until_set() {
        let r = run_submit_sid_demo();
        assert!(r.abort_until_set, "resolve_submit before SET_SID → SubmitSid");
        assert!(r.walk_without_set_ok, "walk without SET_SID still admits");
        assert!(r.after_set_ok, "after set_sid resolve_submit admits");
        assert!(r.all_ok());
    }

}
