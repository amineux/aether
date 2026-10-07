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
