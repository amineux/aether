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

/// Property: after a single successful map, `translate_stream` on the same
/// stream returns `Some` only for guest PAs inside `[base, base + LEN)`, and
/// the returned IOVA lies inside the region's own IOVA window
/// `[iova, iova + LEN)`. The mapping never translates an address outside the
/// pinned region (the "translate never leaves the arena" property, bounded to
/// one region).
#[kani::proof]
#[kani::unwind(20)]
fn translate_stays_within_mapped_region() {
    let mut m = IommuMap::new();
    let tenant: u32 = kani::any();
    kani::assume(tenant < 2);
    let sid: u32 = kani::any();
    kani::assume(sid < 2);
    let page: u64 = kani::any();
    kani::assume(page < 0x100);
    let base = 0x1000 + page * 0x1000;

    let region = match m.map(&mem_cap(tenant), MapRequest::pin_stream(PhysAddr(base), LEN, sid)) {
        Ok(r) => r,
        Err(_) => return,
    };
    assert!(region.tenant == TenantId(tenant));
    assert!(region.guest_pa.0 == base && region.len == LEN);

    let gpa: u64 = kani::any();
    if let Some(iova) = m.translate_stream(sid, PhysAddr(gpa)) {
        assert!(gpa >= region.guest_pa.0 && gpa < region.guest_pa.0 + region.len);
        assert!(iova.0 >= region.iova.0 && iova.0 < region.iova.0 + region.len);
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
