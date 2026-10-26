//! Capability table: mint and require enforce tenant, kind and rights.
//!
//! Bounds: tenant ids `< 4`; capability kinds range over the 10 defined
//! discriminants (0..=9); rights and object ids range over all bits / values.

use aether_core::caps::{CapKind, CapRights, CapTable, Capability};
use aether_core::types::TenantId;

fn any_kind_including_empty() -> CapKind {
    let v: u8 = kani::any();
    kani::assume(v <= 9);
    CapKind::from_u8(v).unwrap()
}

fn any_kind_nonempty() -> CapKind {
    let v: u8 = kani::any();
    kani::assume(v >= 1 && v <= 9);
    CapKind::from_u8(v).unwrap()
}

/// Property: `mint` only ever stores a capability that belongs to the table's
/// owner. If `mint` succeeds the cap's tenant equals the owner and its kind is
/// not `Empty`, and the stored cap reads back as the owner's.
#[kani::proof]
fn cap_mint_rejects_cross_tenant() {
    let owner: u32 = kani::any();
    kani::assume(owner < 4);
    let mut t = CapTable::new(TenantId(owner));

    let ten: u32 = kani::any();
    kani::assume(ten < 4);
    let kind = any_kind_including_empty();
    let rights: u16 = kani::any();
    let object: u32 = kani::any();
    let cap = Capability::new(kind, CapRights(rights), object, TenantId(ten));

    if let Ok(cptr) = t.mint(cap) {
        assert!(ten == owner);
        assert!(kind != CapKind::Empty);
        let got = t.lookup(cptr).unwrap();
        assert!(got.tenant == TenantId(owner));
        assert!(got.object == object);
    }
}

/// Property: `require` returns a capability only when its kind matches the
/// requested kind, it holds every requested right, and it belongs to the
/// table owner. (The cap was minted for the owner, so a success can never hand
/// back another tenant's authority.)
#[kani::proof]
fn cap_require_matches_kind_rights_tenant() {
    let owner: u32 = kani::any();
    kani::assume(owner < 4);
    let mut t = CapTable::new(TenantId(owner));

    let kind = any_kind_nonempty();
    let rights: u16 = kani::any();
    let object: u32 = kani::any();
    let cptr = t
        .mint(Capability::new(kind, CapRights(rights), object, TenantId(owner)))
        .unwrap();

    let req_kind = any_kind_nonempty();
    let need: u16 = kani::any();
    if let Ok(cap) = t.require(cptr, req_kind, need) {
        assert!(cap.kind == req_kind);
        assert!(cap.rights.contains(need));
        assert!(cap.tenant == TenantId(owner));
    }
}

/// Bound: two tables owned by tenants `< 4`; one source cap with symbolic
/// kind (1..=9), rights and object; symbolic requested rights and move flag.
///
/// Property (no escalation across tenants): a successful `transfer` mints a
/// cap in the destination that belongs to the destination's owner and whose
/// rights are a subset of the source's rights; it requires `GRANT` on the
/// source. A refused transfer leaves every destination slot empty.
#[kani::proof]
#[kani::unwind(34)]
fn cap_transfer_never_escalates() {
    use aether_core::caps::{CPtr, CAP_SLOTS};
    let a: u32 = kani::any();
    let b: u32 = kani::any();
    kani::assume(a < 4 && b < 4);
    let mut src = CapTable::new(TenantId(a));
    let mut dst = CapTable::new(TenantId(b));
    let kind = any_kind_nonempty();
    let rights: u16 = kani::any();
    let object: u32 = kani::any();
    let s = src
        .mint(Capability::new(kind, CapRights(rights), object, TenantId(a)))
        .unwrap();
    let want: u16 = kani::any();
    let mv: bool = kani::any();
    match src.transfer(s, &mut dst, CapRights(want), mv) {
        Ok(d) => {
            assert!(CapRights(rights).contains(CapRights::GRANT));
            assert!(CapRights(rights).can_derive(CapRights(want)));
            let got = dst.lookup(d).unwrap();
            assert!(got.tenant == TenantId(b));
            assert!(got.rights.0 == want);
            assert!(got.object == object);
        }
        Err(_) => {
            let mut i = 0;
            while i < CAP_SLOTS {
                assert!(dst.lookup(CPtr(i as u16)).is_err());
                i += 1;
            }
        }
    }
}
