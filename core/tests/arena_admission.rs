use aether_core::arena::{
    ArenaAdmissionFailure, ArenaAllocator, ArenaError, ArenaRequest, MAX_ARENAS,
};
use aether_core::caps::{CapError, CapKind, CapRights, CapTable, Capability, CAP_SLOTS};
use aether_core::{BankId, PhysAddr, TenantId};

fn allocator() -> ArenaAllocator {
    ArenaAllocator::new(&[(BankId(0), PhysAddr(0x10000), 0x100000)]).unwrap()
}

fn state(arenas: &ArenaAllocator, caps: &CapTable) -> String {
    format!("{arenas:?}\n{caps:?}")
}

#[test]
fn wrapping_windows_are_refused_instead_of_wrapping_allocator_arithmetic() {
    assert!(matches!(
        ArenaAllocator::new(&[(BankId(0), PhysAddr(u64::MAX - 8), 16)]),
        Err(ArenaError::BadSize)
    ));
    assert!(matches!(
        ArenaAllocator::new(&[(BankId(0), PhysAddr(0), 0)]),
        Err(ArenaError::BadSize)
    ));
}

#[test]
fn overlapping_windows_are_refused_in_either_order() {
    for second in [(0x18000, 0x10000), (0x11000, 0x1000), (0x8000, 0x20000)] {
        let windows = [
            (BankId(0), PhysAddr(0x10000), 0x10000),
            (BankId(1), PhysAddr(second.0), second.1),
        ];
        assert!(matches!(
            ArenaAllocator::new(&windows),
            Err(ArenaError::BadSize)
        ));
        assert!(matches!(
            ArenaAllocator::new(&[windows[1], windows[0]]),
            Err(ArenaError::BadSize)
        ));
    }
}

#[test]
fn duplicate_bank_ids_are_refused_even_for_disjoint_windows() {
    assert!(matches!(
        ArenaAllocator::new(&[
            (BankId(7), PhysAddr(0x10000), 0x10000),
            (BankId(7), PhysAddr(0x30000), 0x10000)
        ]),
        Err(ArenaError::UnknownBank)
    ));
}

#[test]
fn adjacent_unsorted_windows_and_top_of_address_space_are_valid() {
    let mut arenas = ArenaAllocator::new(&[
        (BankId(9), PhysAddr(0x20000), 0x10000),
        (BankId(3), PhysAddr(0x10000), 0x10000),
    ])
    .unwrap();
    let first = arenas
        .alloc(ArenaRequest::tensor(0x10000, Some(BankId(3))))
        .unwrap();
    let second = arenas
        .alloc(ArenaRequest::tensor(0x10000, Some(BankId(9))))
        .unwrap();
    assert_eq!(first.base.0 + first.size, second.base.0);
    let mut high =
        ArenaAllocator::new(&[(BankId(0), PhysAddr(u64::MAX - 0xffff), 0xffff)]).unwrap();
    let last = high
        .alloc(ArenaRequest::tensor(4096, Some(BankId(0))))
        .unwrap();
    assert!(last.base.0.checked_add(last.size).is_some());
}

#[test]
fn allocation_and_capability_have_the_same_object_owner_and_bank() {
    let mut arenas = allocator();
    let mut caps = CapTable::new(TenantId(7));
    let (arena, cptr) = arenas
        .alloc_with_cap(&mut caps, ArenaRequest::tensor(256, None))
        .unwrap();
    let cap = caps.require(cptr, CapKind::Memory, CapRights::MAP).unwrap();
    assert_eq!(cap.object, arena.id.0);
    assert_eq!(cap.tenant, TenantId(7));
    assert_eq!(arena.owner_tenant, Some(7));
    assert_eq!(arena.color.tenant, TenantId(7));
    assert_eq!(arena.color.bank, arena.bank);
    assert_eq!(caps.occupied(), 1);
    assert_eq!(arenas.live_count(), 1);
}

#[test]
fn full_cap_table_preserves_allocator_and_ids_then_cleanly_retries() {
    let mut arenas = allocator();
    let mut caps = CapTable::new(TenantId(1));
    for i in 0..CAP_SLOTS {
        caps.mint(Capability::new(
            CapKind::Endpoint,
            CapRights::EP_FULL,
            i as u32,
            TenantId(1),
        ))
        .unwrap();
    }
    let before = state(&arenas, &caps);
    for _ in 0..128 {
        assert_eq!(
            arenas.alloc_with_cap(&mut caps, ArenaRequest::tensor(256, None)),
            Err(ArenaAdmissionFailure::Capability(CapError::TableFull))
        );
        assert_eq!(state(&arenas, &caps), before);
    }
    caps.revoke(aether_core::CPtr(0)).unwrap();
    let (arena, _) = arenas
        .alloc_with_cap(&mut caps, ArenaRequest::tensor(256, None))
        .unwrap();
    assert_eq!(arena.id.0, 1, "failed requests must not consume IDs");
    assert_eq!(arena.base, PhysAddr(0x10000));
    assert_eq!(arenas.live_count(), 1);
}

#[test]
fn allocator_refusals_do_not_mint_capabilities() {
    let mut arenas = allocator();
    let mut caps = CapTable::new(TenantId(1));
    for (req, cause) in [
        (ArenaRequest::tensor(0, None), ArenaError::BadSize),
        (ArenaRequest::tensor(0x200000, None), ArenaError::NoSpace),
        (
            ArenaRequest {
                align: 5000,
                ..ArenaRequest::tensor(256, None)
            },
            ArenaError::BadAlign,
        ),
    ] {
        let before = state(&arenas, &caps);
        assert_eq!(
            arenas.alloc_with_cap(&mut caps, req),
            Err(ArenaAdmissionFailure::Arena(cause))
        );
        assert_eq!(state(&arenas, &caps), before);
    }
}

#[test]
fn foreign_color_refusal_preserves_both_structures() {
    let mut arenas = allocator();
    let mut caps = CapTable::new(TenantId(1));
    let before = state(&arenas, &caps);
    assert_eq!(
        arenas.alloc_with_cap(
            &mut caps,
            ArenaRequest::tensor(256, Some(BankId(0))).for_tenant(TenantId(2))
        ),
        Err(ArenaAdmissionFailure::Capability(CapError::CrossTenant))
    );
    assert_eq!(state(&arenas, &caps), before);
}

#[test]
fn arena_limit_does_not_mint_a_cap_and_reuses_reclaimed_capacity() {
    let mut arenas =
        ArenaAllocator::new(&[(BankId(0), PhysAddr(0x10000), MAX_ARENAS as u64 * 4096)]).unwrap();
    let mut caps = CapTable::new(TenantId(1));
    let mut first = None;
    for _ in 0..MAX_ARENAS {
        let admitted = arenas
            .alloc_with_cap(&mut caps, ArenaRequest::tensor(256, None))
            .unwrap();
        first.get_or_insert(admitted);
    }
    let before = state(&arenas, &caps);
    assert_eq!(
        arenas.alloc_with_cap(&mut caps, ArenaRequest::tensor(256, None)),
        Err(ArenaAdmissionFailure::Arena(ArenaError::ArenaLimit))
    );
    assert_eq!(state(&arenas, &caps), before);
    let (old, cptr) = first.unwrap();
    caps.revoke(cptr).unwrap();
    arenas.free(old.id).unwrap();
    let (new, _) = arenas
        .alloc_with_cap(&mut caps, ArenaRequest::tensor(256, None))
        .unwrap();
    assert_eq!(new.base, old.base);
    assert_ne!(new.id, old.id);
    assert_eq!(caps.occupied(), MAX_ARENAS);
}

#[test]
fn older_capability_still_resolves_its_own_arena_after_later_allocation() {
    let mut arenas = allocator();
    let mut caps = CapTable::new(TenantId(1));
    let (older, old_cptr) = arenas
        .alloc_with_cap(&mut caps, ArenaRequest::tensor(256, None))
        .unwrap();
    let (newer, _) = arenas
        .alloc_with_cap(&mut caps, ArenaRequest::tensor(256, None))
        .unwrap();
    let cap = caps
        .require(old_cptr, CapKind::Memory, CapRights::MAP)
        .unwrap();
    assert_eq!(
        arenas.get(aether_core::arena::ArenaId(cap.object)),
        Ok(&older)
    );
    assert_ne!(older.id, newer.id);
}
