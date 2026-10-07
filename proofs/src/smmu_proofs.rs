//! Soft SMMU: a mapping translates only inside its own region, and a stream
//! bound to one tenant cannot be walked on behalf of another.
//!
//! Bounds: one mapping of `LEN = 0x1000` bytes at a page-aligned base, on a
//! stream id `< 2`, for a tenant `< 2`; the translate `guest_pa` ranges over
//! all of `u64`. Only a single region is installed (the allocator's STE / CD /
//! S1 / S2 tables are otherwise empty).

use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::iommu::{IommuMap, MapError, MapRequest};
use aether_core::types::{PhysAddr, TenantId};

const LEN: u64 = 0x1000;

fn mem_cap(tenant: u32) -> Capability {
    Capability::new(
        CapKind::Memory,
        CapRights(CapRights::MAP | CapRights::READ | CapRights::WRITE),
        7,
        TenantId(tenant),
    )
}

/// Bound: tenant 1, stream 0, one mapping of `LEN` bytes at a symbolic
/// page-aligned guest PA `0x1000 + page * 0x1000` with `page < 16`; the
/// translate `guest_pa` ranges over all of `u64`.
///
/// Property: after a single successful map, `translate_stream` on the same
/// stream returns `Some` only for guest PAs inside `[base, base + LEN)`, and
/// the returned IOVA lies inside the region's own IOVA window
/// `[iova, iova + LEN)` at the same offset. The mapping never translates an
/// address outside the pinned region (the "translate never leaves the
/// caller's arena" property, bounded to one region).
#[kani::proof]
#[kani::unwind(20)]
#[kani::solver(cadical)]
fn translate_stays_within_mapped_region() {
    let mut m = IommuMap::new();
    let page: u64 = kani::any();
    kani::assume(page < 16);
    let base = 0x1000 + page * 0x1000;

    let region = match m.map(&mem_cap(1), MapRequest::pin_stream(PhysAddr(base), LEN, 0)) {
        Ok(r) => r,
        Err(_) => return,
    };
    assert!(region.tenant == TenantId(1));
    assert!(region.guest_pa.0 == base && region.len == LEN);

    let gpa: u64 = kani::any();
    if let Some(iova) = m.translate_stream(0, PhysAddr(gpa)) {
        assert!(gpa >= base && gpa < base + LEN);
        assert!(iova.0 == region.iova.0 + (gpa - base));
    }
}

/// Property: a walk tagged with a different tenant than the mapping is refused
/// with `CrossTenant` for an in-range guest PA, while the owning tenant walks
/// successfully. A stream bound to one tenant cannot be resolved for another.
#[kani::proof]
#[kani::unwind(20)]
fn translate_refuses_other_tenant() {
    let mut m = IommuMap::new();
    let sid: u32 = 0;
    let base: u64 = 0x2000;

    let _region = match m.map(&mem_cap(1), MapRequest::pin_stream(PhysAddr(base), LEN, sid)) {
        Ok(r) => r,
        Err(_) => return,
    };
    let off: u64 = kani::any();
    kani::assume(off < LEN);
    let other: u32 = kani::any();
    kani::assume(other < 4 && other != 1);

    let res = m.translate_result(sid, PhysAddr(base + off), Some(TenantId(other)));
    assert!(res == Err(MapError::CrossTenant));
    let ok = m.translate_result(sid, PhysAddr(base + off), Some(TenantId(1)));
    assert!(ok.is_ok());
}

/// Bound: one region of `LEN` bytes at guest PA `0x2000` on stream 0, owned
/// by tenant 1. Attacker: a Memory cap of any tenant `< 4` other than 1 with
/// symbolic rights and object; the unmap IOVA ranges over all of `u64`.
///
/// Property (cross-tenant non-modification, `unmap_for`): another tenant can
/// never unmap the region. Every attempt is refused, the region table still
/// holds exactly the original region, and the owner's translation of every
/// in-range guest PA is unchanged.
#[kani::proof]
#[kani::unwind(20)]
fn unmap_for_cross_tenant_leaves_region() {
    let mut m = IommuMap::new();
    let base: u64 = 0x2000;
    let region = match m.map(&mem_cap(1), MapRequest::pin_stream(PhysAddr(base), LEN, 0)) {
        Ok(r) => r,
        Err(_) => return,
    };
    let other: u32 = kani::any();
    kani::assume(other < 4 && other != 1);
    let rights: u16 = kani::any();
    let object: u32 = kani::any();
    let attacker = Capability::new(CapKind::Memory, CapRights(rights), object, TenantId(other));
    let iova: u64 = kani::any();

    assert!(m.unmap_for(&attacker, PhysAddr(iova)).is_err());
    assert!(m.len() == 1);
    assert!(m.iter().next() == Some(region));
    let off: u64 = kani::any();
    kani::assume(off < LEN);
    assert!(
        m.translate_result(0, PhysAddr(base + off), Some(TenantId(1)))
            == Ok(PhysAddr(region.iova.0 + off))
    );
}

/// Bound: stream 0 bound by tenant 1 through one `LEN`-byte region; the
/// attacker is any tenant `< 4` other than 1 holding a full Memory+MAP cap
/// and asks for any guest PA / length with `len <= 4 * LEN`.
///
/// Property: a tenant cannot pin anything on a stream another tenant owns.
/// `map` is refused with `CrossTenant` and the region table is unchanged.
#[kani::proof]
#[kani::unwind(20)]
fn map_on_foreign_stream_refused() {
    let mut m = IommuMap::new();
    let region = match m.map(&mem_cap(1), MapRequest::pin_stream(PhysAddr(0x2000), LEN, 0)) {
        Ok(r) => r,
        Err(_) => return,
    };
    let other: u32 = kani::any();
    kani::assume(other < 4 && other != 1);
    let gpa: u64 = kani::any();
    let len: u64 = kani::any();
    kani::assume(len >= 1 && len <= 4 * LEN);
    kani::assume(gpa.checked_add(len).is_some());
    let res = m.map(&mem_cap(other), MapRequest::pin_stream(PhysAddr(gpa), len, 0));
    assert!(res == Err(MapError::CrossTenant));
    assert!(m.len() == 1);
    assert!(m.iter().next() == Some(region));
}
