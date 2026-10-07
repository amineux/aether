//! Tensor-arena allocator: limit, no-leak, and cross-tenant noninterference.
//!
//! Shared bounds: a single bank `[BASE, BASE + BANK_SIZE)` and the allocator's
//! own fixed tables (`MAX_ARENAS = 16` arena slots, `MAX_FREE = 24` free
//! spans). Allocation sizes are bounded per harness as documented.

use aether_core::arena::{ArenaAllocator, ArenaError, ArenaRequest, MAX_ARENAS};
use aether_core::types::{BankId, PhysAddr, TenantId, PAGE_4K};

const BASE: u64 = 0x10_0000;
const BANK_SIZE: u64 = (MAX_ARENAS as u64 + 8) * PAGE_4K;

fn fresh() -> ArenaAllocator {
    ArenaAllocator::new(&[(BankId(0), PhysAddr(BASE), BANK_SIZE)]).unwrap()
}

fn one_page(tenant: u32) -> ArenaRequest {
    ArenaRequest::tensor(PAGE_4K, Some(BankId(0))).for_tenant(TenantId(tenant))
}

/// Bound: table filled with 16 one-page arenas; probe size in `[1, PAGE_4K]`.
///
/// Property (ArenaLimit + no leak): once every arena slot is in use, the next
/// allocation is refused with `ArenaError::ArenaLimit` and the bank's free
/// byte count is unchanged — the refuse path splits and loses nothing.
#[kani::proof]
#[kani::unwind(26)]
fn arena_limit_refuses_and_conserves() {
    let mut a = fresh();
    for _ in 0..MAX_ARENAS {
        assert!(a.alloc(one_page(1)).is_ok());
    }
    let before = a.free_bytes(BankId(0));
    let spans_before = a.free_span_count();
    let size: u64 = kani::any();
    kani::assume(size >= 1 && size <= PAGE_4K);
    let res = a.alloc(ArenaRequest::tensor(size, Some(BankId(0))).for_tenant(TenantId(2)));
    assert!(res == Err(ArenaError::ArenaLimit));
    assert!(a.free_bytes(BankId(0)) == before);
    assert!(a.free_span_count() == spans_before);
    assert!(a.live_count() == MAX_ARENAS);
}

/// Bound: three live arenas (1, 2 and 1 pages, tenants 1, 2, 3) carved from
/// one bank; they are then freed in a symbolic order (all 6 permutations).
///
/// Property (no leak, any free order): every `free` returns exactly that
/// arena's bytes to the bank, and after all three the bank's free byte count
/// equals the starting total and the free list has re-coalesced to a single
/// span. Coalescing never drops or double-counts a byte whatever the order.
#[kani::proof]
#[kani::unwind(26)]
fn arena_free_any_order_conserves_bytes() {
    let mut a = fresh();
    let total = a.free_bytes(BankId(0));
    let x = a.alloc(one_page(1)).unwrap();
    let y = a
        .alloc(ArenaRequest::tensor(2 * PAGE_4K, Some(BankId(0))).for_tenant(TenantId(2)))
        .unwrap();
    let z = a.alloc(one_page(3)).unwrap();
    assert!(a.free_bytes(BankId(0)) == total - 4 * PAGE_4K);
    let order: u8 = kani::any();
    kani::assume(order < 6);
    let seq = match order {
        0 => [x, y, z],
        1 => [x, z, y],
        2 => [y, x, z],
        3 => [y, z, x],
        4 => [z, x, y],
        _ => [z, y, x],
    };
    for ar in seq {
        let before = a.free_bytes(BankId(0));
        assert!(a.free(ar.id).is_ok());
        assert!(a.free_bytes(BankId(0)) == before + ar.size);
    }
    assert!(a.free_bytes(BankId(0)) == total);
    assert!(a.free_span_count() == 1);
    assert!(a.live_count() == 0);
}

/// Bound: two live arenas (tenants 1 and 2); one symbolic `transfer_owner`
/// aimed at the first, with `from`/`to` tiles and tenant each `< 4`.
///
/// Property (noninterference): an ownership operation on one tenant's arena
/// never changes another tenant's arena — every byte of the second arena is
/// identical before and after, whatever the operation returns.
#[kani::proof]
#[kani::unwind(26)]
fn arena_op_does_not_touch_other_tenant() {
    let mut a = fresh();
    let x = a.alloc(one_page(1)).unwrap();
    let y = a.alloc(one_page(2)).unwrap();
    let y_before = *a.get(y.id).unwrap();

    let from = if kani::any() {
        let t: u16 = kani::any();
        kani::assume(t < 4);
        Some(t)
    } else {
        None
    };
    let to: u16 = kani::any();
    kani::assume(to < 4);
    let tenant: u32 = kani::any();
    kani::assume(tenant < 4);
    let _ = a.transfer_owner(x.id, from, to, tenant);

    let y_after = *a.get(y.id).unwrap();
    assert!(y_before == y_after);
}

/// Bound: one freshly-allocated arena (owner tile `None`); symbolic `from`,
/// `to`, `tenant` each `< 8`.
///
/// Property: a handoff claimed by a non-owner (`from = Some(_)` on an arena
/// the kernel still owns) is refused with `ArenaError::NotOwner` and leaves
/// the arena byte-identical.
#[kani::proof]
#[kani::unwind(26)]
fn arena_nonowner_transfer_refused_leaves_state() {
    let mut a = fresh();
    let x = a.alloc(one_page(1)).unwrap();
    let x_before = *a.get(x.id).unwrap();
    let from: u16 = kani::any();
    kani::assume(from < 8);
    let to: u16 = kani::any();
    kani::assume(to < 8);
    let tenant: u32 = kani::any();
    kani::assume(tenant < 8);
    let res = a.transfer_owner(x.id, Some(from), to, tenant);
    assert!(res == Err(ArenaError::NotOwner));
    assert!(*a.get(x.id).unwrap() == x_before);
}

/// Bound: tenant 2 holds one live one-page arena; tenant 1 then asks for one
/// symbolic allocation of size `[1, 2*PAGE_4K]`.
///
/// Property (allocation isolation): a new allocation never overlaps another
/// tenant's live arena, and the other arena is byte-identical afterwards.
#[kani::proof]
#[kani::unwind(26)]
fn arena_alloc_never_overlaps_live_arena() {
    let mut a = fresh();
    let y = a.alloc(one_page(2)).unwrap();
    let y_before = *a.get(y.id).unwrap();
    let size: u64 = kani::any();
    kani::assume(size >= 1 && size <= 2 * PAGE_4K);
    if let Ok(x) = a.alloc(ArenaRequest::tensor(size, Some(BankId(0))).for_tenant(TenantId(1))) {
        let xe = x.base.0 + x.size;
        let ye = y.base.0 + y.size;
        assert!(xe <= y.base.0 || ye <= x.base.0);
        assert!(x.base.0 >= BASE && xe <= BASE + BANK_SIZE);
        assert!(x.id != y.id);
    }
    assert!(*a.get(y.id).unwrap() == y_before);
}

/// Bound: two live one-page arenas (tenants 1 and 2); `free` of any arena id
/// `< 8` other than the second arena's.
///
/// Property (cross-tenant non-modification, `free`): freeing anything other
/// than tenant 2's arena leaves tenant 2's arena live and byte-identical, and
/// the freed range never covers it.
#[kani::proof]
#[kani::unwind(26)]
fn arena_free_does_not_touch_other_tenant() {
    use aether_core::arena::ArenaId;
    let mut a = fresh();
    let _x = a.alloc(one_page(1)).unwrap();
    let y = a.alloc(one_page(2)).unwrap();
    let y_before = *a.get(y.id).unwrap();
    let id: u32 = kani::any();
    kani::assume(id < 8 && id != y.id.0);
    let _ = a.free(ArenaId(id));
    assert!(*a.get(y.id).unwrap() == y_before);
    assert!(a.live_count() >= 1);
}

/// Bound: one arena handed off kernel -> tile 2 (tenant 1); a reclaim or
/// handoff attempt with symbolic `from` (any `Option<u16>` other than
/// `Some(2)`), `to < 8` and tenant `< 8`.
///
/// Property (reclaim refused): only the current owner tile can hand the arena
/// on; the kernel-style reclaim (`from = None`) and every other tile are
/// refused with `NotOwner`, and the arena is byte-identical.
#[kani::proof]
#[kani::unwind(26)]
fn arena_reclaim_by_non_owner_refused() {
    let mut a = fresh();
    let x = a.alloc(one_page(1)).unwrap();
    assert!(a.transfer_owner(x.id, None, 2, 1).is_ok());
    let x_before = *a.get(x.id).unwrap();
    let from: Option<u16> = if kani::any() { Some(kani::any()) } else { None };
    kani::assume(from != Some(2));
    let to: u16 = kani::any();
    kani::assume(to < 8);
    let tenant: u32 = kani::any();
    kani::assume(tenant < 8);
    assert!(a.transfer_owner(x.id, from, to, tenant) == Err(ArenaError::NotOwner));
    assert!(*a.get(x.id).unwrap() == x_before);
}
