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
