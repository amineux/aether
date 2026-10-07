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
#[kani::unwind(28)]
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

/// Bound: empty allocator; one symbolic allocation of size in `[1, 4*PAGE_4K]`.
///
/// Property (no leak): if an allocation succeeds, freeing it restores the
/// bank's free byte count exactly — alloc-then-free never loses a byte.
#[kani::proof]
#[kani::unwind(28)]
fn arena_alloc_then_free_conserves_bytes() {
    let mut a = fresh();
    let total = a.free_bytes(BankId(0));
    let size: u64 = kani::any();
    kani::assume(size >= 1 && size <= 4 * PAGE_4K);
    if let Ok(ar) = a.alloc(ArenaRequest::tensor(size, Some(BankId(0))).for_tenant(TenantId(1))) {
        assert!(a.free_bytes(BankId(0)) <= total);
        assert!(a.free(ar.id).is_ok());
        assert!(a.free_bytes(BankId(0)) == total);
    }
}

/// Bound: two live arenas (tenants 1 and 2); one symbolic `transfer_owner`
/// aimed at the first, with `from`/`to` tiles and tenant each `< 4`.
///
/// Property (noninterference): an ownership operation on one tenant's arena
/// never changes another tenant's arena — every byte of the second arena is
/// identical before and after, whatever the operation returns.
#[kani::proof]
#[kani::unwind(28)]
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
#[kani::unwind(28)]
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
