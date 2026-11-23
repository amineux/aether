//! Actual boot table selection and grant policy, shared with the kernel.
#[path = "../../kernel/src/cap_spaces.rs"]
mod cap_spaces;
use aether_core::caps::{CPtr, CapKind, CapRights, CapTable, Capability};
use aether_core::types::TenantId;
use cap_spaces::CapSpaces;

const INIT: u64 = 0x1000;
const PROBE: u64 = 0x2000;

fn spaces() -> CapSpaces {
    let mut caps = CapTable::new(TenantId(1));
    caps.mint(Capability::new(
        CapKind::Endpoint,
        CapRights::EP_FULL,
        10,
        TenantId(1),
    ))
    .unwrap();
    caps.mint(Capability::new(
        CapKind::AccelQueue,
        CapRights::ACCEL_FULL,
        1,
        TenantId(1),
    ))
    .unwrap();
    CapSpaces::new(caps)
}

#[test]
fn unbound_zero_and_unknown_roots_have_no_user_table() {
    let mut spaces = spaces();
    for root in [0, INIT, PROBE] {
        assert!(spaces.get(root).is_none());
        assert!(spaces.get_mut(root).is_none());
        assert!(!spaces.can_allocate(root));
    }
    assert_eq!(spaces.bind_init(0), Err(()));
    assert_eq!(spaces.bind_probe_endpoint(PROBE, 11), Err(()));
    assert_eq!(spaces.bootstrap().lookup(CPtr(0)).unwrap().object, 10);
    spaces.bind_init(INIT).unwrap();
    for root in [0, PROBE, 0x3000] {
        assert!(spaces.get(root).is_none());
        assert!(spaces.get_mut(root).is_none());
    }
}

#[test]
fn equal_slots_resolve_to_different_objects_in_same_tenant() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    let a = spaces.get(INIT).unwrap().lookup(CPtr(0)).unwrap();
    let b = spaces.get(PROBE).unwrap().lookup(CPtr(0)).unwrap();
    assert_eq!(a.tenant, b.tenant);
    assert_eq!((a.object, b.object), (10, 11));
    assert_ne!(a.cdt(), b.cdt());
    spaces.get_mut(PROBE).unwrap().revoke(CPtr(0)).unwrap();
    assert!(spaces.get(PROBE).unwrap().lookup(CPtr(0)).is_err());
    assert_eq!(
        spaces.get(INIT).unwrap().lookup(CPtr(0)).unwrap().object,
        10
    );
}

#[test]
fn probe_boot_policy_grants_only_private_ipc_without_grant_right() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    let probe = spaces.get(PROBE).unwrap();
    let cap = probe
        .require(
            CPtr(0),
            CapKind::Endpoint,
            CapRights::READ | CapRights::WRITE,
        )
        .unwrap();
    assert_eq!(cap.badge, 0xB4);
    assert!(!cap.rights.contains(CapRights::GRANT));
    assert!(probe
        .require(CPtr(0), CapKind::Endpoint, CapRights::GRANT)
        .is_err());
    assert!(probe
        .require(CPtr(0), CapKind::Memory, CapRights::MAP)
        .is_err());
    assert!(probe
        .require(CPtr(0), CapKind::AccelQueue, CapRights::SUBMIT)
        .is_err());
    for slot in 1..aether_core::caps::CAP_SLOTS {
        assert!(probe.lookup(CPtr(slot as u16)).is_err());
    }
    assert!(!spaces.can_allocate(PROBE));
    assert!(spaces.can_allocate(INIT));
    assert!(spaces
        .get(INIT)
        .unwrap()
        .require(CPtr(1), CapKind::AccelQueue, CapRights::SUBMIT)
        .is_ok());
}

#[test]
fn roots_cannot_be_rebound_or_replace_an_existing_space() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    for root in [0, INIT, PROBE] {
        assert_eq!(spaces.bind_init(root), Err(()));
    }
    for root in [0, INIT] {
        assert_eq!(spaces.bind_probe_endpoint(root, 99), Err(()));
    }
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    for root in [0, INIT, PROBE, 0x3000] {
        assert_eq!(spaces.bind_probe_endpoint(root, 99), Err(()));
    }
    assert_eq!(
        spaces.get(PROBE).unwrap().lookup(CPtr(0)).unwrap().object,
        11
    );
    assert_eq!(
        spaces.get(INIT).unwrap().lookup(CPtr(0)).unwrap().object,
        10
    );
    assert!(spaces.get(0x3000).is_none());
}

#[test]
fn cloned_threads_share_the_root_table_not_a_snapshot() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    let slot = spaces
        .get_mut(INIT)
        .unwrap()
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            42,
            TenantId(1),
        ))
        .unwrap();
    for _sibling in 0..4 {
        assert_eq!(
            spaces
                .get(INIT)
                .unwrap()
                .require(slot, CapKind::Memory, CapRights::MAP)
                .unwrap()
                .object,
            42
        );
        assert!(spaces.get(PROBE).unwrap().lookup(slot).is_err());
    }
}

#[test]
fn endpoint_copy_attenuates_and_tracks_parent_without_consuming_source() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    let parent = *spaces.get(INIT).unwrap().lookup(CPtr(0)).unwrap();
    let slot = spaces
        .grant_endpoint(INIT, PROBE, CPtr(0), CapRights::WRITE as u64)
        .unwrap();
    assert_eq!(slot, CPtr(1));
    let child = *spaces.get(PROBE).unwrap().lookup(slot).unwrap();
    assert_eq!(child.object, parent.object);
    assert_eq!(child.badge, parent.badge);
    assert_eq!(child.tenant, parent.tenant);
    assert_eq!(child.parent, Some(parent.cdt()));
    assert_ne!(child.cdt(), parent.cdt());
    assert_eq!(child.rights, CapRights(CapRights::WRITE));
    assert_eq!(spaces.get(INIT).unwrap().lookup(CPtr(0)), Ok(&parent));
    assert_eq!(
        spaces.get(PROBE).unwrap().lookup(CPtr(0)).unwrap().object,
        11
    );
    assert_eq!(
        spaces.grant_endpoint(PROBE, INIT, slot, CapRights::WRITE as u64),
        Err(aether_core::caps::CapError::InsufficientRights)
    );
    assert_eq!(spaces.get(INIT).unwrap().occupied(), 2);
}

#[test]
fn grant_rejects_zero_unknown_and_high_rights_without_truncation_or_mutation() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    let parent = *spaces.get(INIT).unwrap().lookup(CPtr(0)).unwrap();
    for rights in [
        0,
        CapRights::GRANT as u64,
        CapRights::MAP as u64,
        (CapRights::READ | CapRights::GRANT) as u64,
        u16::MAX as u64,
        (1 << 16) | CapRights::WRITE as u64,
        (1 << 32) | CapRights::WRITE as u64,
        u64::MAX,
    ] {
        assert_eq!(
            spaces.grant_endpoint(INIT, PROBE, CPtr(0), rights),
            Err(aether_core::caps::CapError::WouldEscalate)
        );
        assert_eq!(spaces.get(INIT).unwrap().lookup(CPtr(0)), Ok(&parent));
        assert_eq!(spaces.get(PROBE).unwrap().occupied(), 1);
    }
}

#[test]
fn grant_cannot_share_queue_memory_or_ungranted_endpoints() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    let memory = spaces
        .get_mut(INIT)
        .unwrap()
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            42,
            TenantId(1),
        ))
        .unwrap();
    for slot in [CPtr(1), memory] {
        assert_eq!(
            spaces.grant_endpoint(INIT, PROBE, slot, CapRights::READ as u64),
            Err(aether_core::caps::CapError::WrongKind)
        );
    }
    assert_eq!(
        spaces.grant_endpoint(PROBE, INIT, CPtr(0), CapRights::WRITE as u64),
        Err(aether_core::caps::CapError::InsufficientRights)
    );
    for slot in [CPtr(3), CPtr(u16::MAX)] {
        assert!(spaces
            .grant_endpoint(INIT, PROBE, slot, CapRights::WRITE as u64)
            .is_err());
    }
    assert_eq!(spaces.get(INIT).unwrap().occupied(), 3);
    assert_eq!(spaces.get(PROBE).unwrap().occupied(), 1);
}

#[test]
fn grant_never_escalates_a_grantable_read_only_source() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    let source = spaces
        .get_mut(INIT)
        .unwrap()
        .mint(Capability::new(
            CapKind::Endpoint,
            CapRights(CapRights::READ | CapRights::GRANT),
            12,
            TenantId(1),
        ))
        .unwrap();
    assert_eq!(
        spaces.grant_endpoint(INIT, PROBE, source, CapRights::WRITE as u64),
        Err(aether_core::caps::CapError::WouldEscalate)
    );
    assert_eq!(spaces.get(PROBE).unwrap().occupied(), 1);
    let target = spaces
        .grant_endpoint(INIT, PROBE, source, CapRights::READ as u64)
        .unwrap();
    assert!(spaces
        .get(PROBE)
        .unwrap()
        .require(target, CapKind::Endpoint, CapRights::READ)
        .is_ok());
    assert!(spaces
        .get(PROBE)
        .unwrap()
        .require(target, CapKind::Endpoint, CapRights::WRITE)
        .is_err());
}

#[test]
fn grant_refuses_unbound_unknown_and_same_space_roots() {
    let mut spaces = spaces();
    assert!(spaces
        .grant_endpoint(INIT, PROBE, CPtr(0), CapRights::WRITE as u64)
        .is_err());
    spaces.bind_init(INIT).unwrap();
    assert!(spaces
        .grant_endpoint(INIT, PROBE, CPtr(0), CapRights::WRITE as u64)
        .is_err());
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    for (source, target) in [
        (0, PROBE),
        (INIT, 0),
        (0x3000, PROBE),
        (INIT, 0x3000),
        (INIT, INIT),
        (PROBE, PROBE),
    ] {
        assert_eq!(
            spaces.grant_endpoint(source, target, CPtr(0), CapRights::WRITE as u64),
            Err(aether_core::caps::CapError::InvalidCptr)
        );
    }
    assert_eq!(spaces.get(INIT).unwrap().occupied(), 2);
    assert_eq!(spaces.get(PROBE).unwrap().occupied(), 1);
}

#[test]
fn full_target_grant_refusal_preserves_source_and_all_existing_children() {
    let mut spaces = spaces();
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, 11).unwrap();
    let parent = *spaces.get(INIT).unwrap().lookup(CPtr(0)).unwrap();
    for slot in 1..aether_core::caps::CAP_SLOTS {
        assert_eq!(
            spaces.grant_endpoint(INIT, PROBE, CPtr(0), CapRights::WRITE as u64),
            Ok(CPtr(slot as u16))
        );
    }
    let before: Vec<_> = (0..aether_core::caps::CAP_SLOTS)
        .map(|slot| {
            *spaces
                .get(PROBE)
                .unwrap()
                .lookup(CPtr(slot as u16))
                .unwrap()
        })
        .collect();
    for _ in 0..3 {
        assert_eq!(
            spaces.grant_endpoint(INIT, PROBE, CPtr(0), CapRights::WRITE as u64),
            Err(aether_core::caps::CapError::TableFull)
        );
        assert_eq!(spaces.get(INIT).unwrap().lookup(CPtr(0)), Ok(&parent));
        for (slot, cap) in before.iter().enumerate() {
            assert_eq!(
                spaces.get(PROBE).unwrap().lookup(CPtr(slot as u16)),
                Ok(cap)
            );
        }
    }
}
