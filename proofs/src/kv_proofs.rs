//! KV grants: `attend` fails closed across tenants, weights and rights.
//!
//! Bounds: tenant ids, object ids and sequence numbers `< 3`; a single KV
//! object in the ledger; a 2x2 layer/token window; symbolic cap rights and
//! symbolic attend request.

use aether_core::caps::{CapKind, CapRights, CapTable, Capability};
use aether_core::kvfabric::{attend, AttendReq, KvKind, KvLedger, KvObject, KvWindow};
use aether_core::types::{PhysAddr, TenantId};

/// Property (noninterference / fail-closed): `attend` succeeds only when the
/// KV object actually belongs to the table owner, is a KV page (not weights),
/// and the requested sequence matches. In particular a tenant can never attend
/// a KV object owned by another tenant.
#[kani::proof]
#[kani::unwind(10)]
fn kv_attend_never_crosses_tenant() {
    let owner: u32 = kani::any();
    kani::assume(owner < 3);
    let mut t = CapTable::new(TenantId(owner));

    let obj_tenant: u32 = kani::any();
    kani::assume(obj_tenant < 3);
    let obj_id: u32 = kani::any();
    kani::assume(obj_id < 3);
    let seq: u32 = kani::any();
    kani::assume(seq < 3);
    let kind = if kani::any() { KvKind::Kv } else { KvKind::Weights };
    let window = KvWindow { layer_lo: 0, layer_hi: 2, token_lo: 0, token_hi: 2 };

    let mut ledger = KvLedger::new();
    ledger
        .insert(KvObject {
            id: obj_id,
            seq,
            base: PhysAddr(0x1000),
            bytes: 0x1000,
            kind,
            window,
            tenant: TenantId(obj_tenant),
        })
        .unwrap();

    let rights: u16 = kani::any();
    let cptr = t
        .mint(Capability::new(CapKind::Memory, CapRights(rights), obj_id, TenantId(owner)))
        .unwrap();

    let req = AttendReq {
        seq: kani::any(),
        layer: kani::any(),
        token: kani::any(),
        write: kani::any(),
    };

    if attend(&t, cptr, &ledger, req).is_ok() {
        assert!(obj_tenant == owner);
        assert!(kind == KvKind::Kv);
        assert!(req.seq == seq);
        if req.write {
            assert!(CapRights(rights).contains(CapRights::WRITE));
        } else {
            assert!(CapRights(rights).contains(CapRights::READ));
        }
    }
}

/// Bound: source and destination tables for tenants `< 3`; one Memory cap
/// with symbolic rights in the source.
///
/// Property: a decode-side `try_regrant` succeeds only when the source cap
/// holds `GRANT`; otherwise it is refused with `WouldRegrant` and the
/// destination table stays empty (slot 0 still empty).
#[kani::proof]
#[kani::unwind(34)]
fn kv_regrant_requires_grant() {
    use aether_core::caps::CPtr;
    use aether_core::kvfabric::{try_regrant, KvError};
    let a: u32 = kani::any();
    let b: u32 = kani::any();
    kani::assume(a < 3 && b < 3);
    let mut src = CapTable::new(TenantId(a));
    let mut dst = CapTable::new(TenantId(b));
    let rights: u16 = kani::any();
    let s = src
        .mint(Capability::new(CapKind::Memory, CapRights(rights), 1, TenantId(a)))
        .unwrap();
    let res = try_regrant(&mut src, s, &mut dst);
    if !CapRights(rights).contains(CapRights::GRANT) {
        assert!(res == Err(KvError::WouldRegrant));
        assert!(dst.lookup(CPtr(0)).is_err());
    }
    if let Ok(d) = res {
        assert!(dst.lookup(d).unwrap().tenant == TenantId(b));
    }
}

/// Bound: one KV object owned by tenant 1; a Memory cap of tenant 2 with
/// symbolic rights naming that object; empty Soft SMMU. (Tenant ids are
/// fixed so the checker never unrolls the unreachable map path; the check
/// only compares ids, so any other distinct pair behaves the same.)
///
/// Property (cross-tenant DMA pin refused): `pin_kv` on another tenant's KV
/// object never succeeds and installs nothing in the SMMU.
#[kani::proof]
#[kani::unwind(20)]
fn kv_pin_cross_tenant_installs_nothing() {
    use aether_core::iommu::{IommuMap, StreamId};
    use aether_core::kvfabric::pin_kv;
    let obj_tenant: u32 = 1;
    let cap_tenant: u32 = 2;
    let mut ledger = KvLedger::new();
    ledger
        .insert(KvObject {
            id: 1,
            seq: 0,
            base: PhysAddr(0x4000),
            bytes: 0x1000,
            kind: KvKind::Kv,
            window: KvWindow { layer_lo: 0, layer_hi: 1, token_lo: 0, token_hi: 1 },
            tenant: TenantId(obj_tenant),
        })
        .unwrap();
    let rights: u16 = kani::any();
    let cap = Capability::new(CapKind::Memory, CapRights(rights), 1, TenantId(cap_tenant));
    let mut m = IommuMap::new();
    assert!(pin_kv(&mut m, &cap, &ledger, StreamId(0)).is_err());
    assert!(m.is_empty());
    assert!(m.ste_count() == 0);
}
