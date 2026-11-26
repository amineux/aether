//! Exercise the kernel's actual teardown transaction and waiter policy.
#[path = "../../kernel/src/cap_spaces.rs"]
mod cap_spaces;
#[path = "../../kernel/src/endpoint_close.rs"]
mod endpoint_close;
use aether_core::caps::{CPtr, CapKind, CapRights, CapTable, Capability};
use aether_core::fabric::{ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags};
use aether_core::preempt::{CpuQueue, ThreadState, WaitWhy};
use aether_core::types::TenantId;
use cap_spaces::CapSpaces;

const INIT: u64 = 0x1000;
const PROBE: u64 = 0x2000;
fn fixture() -> (CapSpaces, Fabric, EndpointId, EndpointId) {
    let mut fabric = Fabric::new();
    let ep = fabric.create_endpoint(TenantId(1)).unwrap();
    let private = fabric.create_endpoint(TenantId(1)).unwrap();
    let mut caps = CapTable::new(TenantId(1));
    caps.mint(Capability::new(
        CapKind::Endpoint,
        CapRights::EP_FULL,
        ep.0,
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
    let mut spaces = CapSpaces::new(caps);
    spaces.bind_init(INIT).unwrap();
    spaces.bind_probe_endpoint(PROBE, private.0).unwrap();
    (spaces, fabric, ep, private)
}
fn message(ep: EndpointId) -> Message {
    Message::new(
        ep,
        0xA3,
        MsgFlags(2),
        ChipletRoute::LOCAL,
        TenantId(1),
        b"queued",
    )
    .unwrap()
}

#[test]
fn close_deletes_the_lineage_keeps_private_caps_and_reclaims_fabric_capacity() {
    let (mut spaces, mut fabric, ep, private) = fixture();
    let child = spaces
        .grant_endpoint(INIT, PROBE, CPtr(0), CapRights::READ as u64)
        .unwrap();
    let queue = *spaces.get(INIT).unwrap().lookup(CPtr(1)).unwrap();
    let private_cap = *spaces.get(PROBE).unwrap().lookup(CPtr(0)).unwrap();
    assert_eq!(
        endpoint_close::close(&mut spaces, &mut fabric, INIT, CPtr(0)),
        Ok(ep)
    );
    assert!(spaces.get(INIT).unwrap().lookup(CPtr(0)).is_err());
    assert!(spaces.get(PROBE).unwrap().lookup(child).is_err());
    assert_eq!(spaces.get(INIT).unwrap().lookup(CPtr(1)), Ok(&queue));
    assert_eq!(spaces.get(PROBE).unwrap().lookup(CPtr(0)), Ok(&private_cap));
    assert_eq!(fabric.live_count(TenantId(1)), 1);
    fabric
        .send_as(spaces.get(PROBE).unwrap(), message(private))
        .unwrap();
    assert_eq!(
        fabric.recv_as(TenantId(1), private).unwrap().payload(),
        b"queued"
    );
    assert!(endpoint_close::close(&mut spaces, &mut fabric, INIT, CPtr(0)).is_err());
}

#[test]
fn checked_receive_on_closed_empty_or_queued_endpoint_never_blocks_or_dequeues() {
    for queued in [false, true] {
        let (mut spaces, mut fabric, ep, _) = fixture();
        if queued {
            fabric
                .send_as(spaces.get(INIT).unwrap(), message(ep))
                .unwrap();
        }
        endpoint_close::close(&mut spaces, &mut fabric, INIT, CPtr(0)).unwrap();
        assert!(matches!(
            fabric.recv_as(TenantId(2), ep),
            Err(FabricError::NoSuchEndpoint)
        ));
        assert!(matches!(
            fabric.recv_as(TenantId(1), ep),
            Err(FabricError::Closed)
        ));
        assert_eq!(fabric.pending_as(TenantId(1), ep), Ok(usize::from(queued)));
        let old_slot = fabric.slot_of(ep);
        // Fill empty slots first, then force reuse of the closed slot.
        let mut fresh = fabric.create_endpoint(TenantId(2)).unwrap();
        for _ in 0..aether_core::fabric::MAX_ENDPOINTS {
            if fabric.slot_of(fresh) == old_slot {
                break;
            }
            fresh = fabric.create_endpoint(TenantId(2 + fresh.0)).unwrap();
        }
        assert_eq!(fabric.slot_of(fresh), old_slot);
        assert_ne!(fresh, ep);
        assert!(matches!(
            fabric.recv_as(TenantId(1), ep),
            Err(FabricError::NoSuchEndpoint)
        ));
        assert!(matches!(
            fabric.send_unchecked(message(ep)),
            Err(FabricError::NoSuchEndpoint)
        ));
    }
}

#[test]
fn refused_capability_close_preserves_both_tables_and_queued_messages() {
    let (mut spaces, mut fabric, ep, _) = fixture();
    let child = spaces
        .grant_endpoint(INIT, PROBE, CPtr(0), CapRights::WRITE as u64)
        .unwrap();
    fabric
        .send_as(spaces.get(INIT).unwrap(), message(ep))
        .unwrap();
    for (root, slot) in [
        (0, CPtr(0)),
        (0x3000, CPtr(0)),
        (INIT, CPtr(1)),
        (INIT, CPtr(31)),
        (PROBE, CPtr(0)),
        (PROBE, child),
    ] {
        assert!(endpoint_close::close(&mut spaces, &mut fabric, root, slot).is_err());
        assert_eq!(spaces.get(INIT).unwrap().occupied(), 2);
        assert_eq!(spaces.get(PROBE).unwrap().occupied(), 2);
        assert_eq!(fabric.live_count(TenantId(1)), 2);
        assert_eq!(fabric.pending_as(TenantId(1), ep), Ok(1));
    }
}

#[test]
fn foreign_owner_preflight_refuses_before_deleting_a_valid_grant_lineage() {
    let (mut spaces, mut fabric, _, _) = fixture();
    let foreign = fabric.create_endpoint(TenantId(2)).unwrap();
    let source = spaces
        .get_mut(INIT)
        .unwrap()
        .mint(Capability::new(
            CapKind::Endpoint,
            CapRights::EP_FULL,
            foreign.0,
            TenantId(1),
        ))
        .unwrap();
    let child = spaces
        .grant_endpoint(INIT, PROBE, source, CapRights::READ as u64)
        .unwrap();
    let parent = *spaces.get(INIT).unwrap().lookup(source).unwrap();
    let derived = *spaces.get(PROBE).unwrap().lookup(child).unwrap();
    assert!(endpoint_close::close(&mut spaces, &mut fabric, INIT, source).is_err());
    assert_eq!(spaces.get(INIT).unwrap().lookup(source), Ok(&parent));
    assert_eq!(spaces.get(PROBE).unwrap().lookup(child), Ok(&derived));
    assert_eq!(fabric.live_count(TenantId(2)), 1);
}

#[test]
fn close_cancels_independent_read_aliases_but_not_private_or_accel_waiters() {
    let (mut spaces, mut fabric, ep, private) = fixture();
    spaces
        .grant_endpoint(INIT, PROBE, CPtr(0), CapRights::READ as u64)
        .unwrap();
    let alias = spaces
        .get_mut(PROBE)
        .unwrap()
        .mint(Capability::new(
            CapKind::Endpoint,
            CapRights(CapRights::READ),
            ep.0,
            TenantId(1),
        ))
        .unwrap();
    let mut queue = CpuQueue::new();
    for id in 1..=5 {
        queue.spawn(id);
    }
    queue.ensure_running();
    for why in [
        WaitWhy::Recv(ep.0),
        WaitWhy::Recv(ep.0),
        WaitWhy::Recv(private.0),
        WaitWhy::Accel(1),
    ] {
        queue.block(why);
    }
    let closed = endpoint_close::close(&mut spaces, &mut fabric, INIT, CPtr(0)).unwrap();
    assert!(spaces.get(PROBE).unwrap().lookup(alias).is_ok());
    assert!(spaces.can_receive(PROBE, ep.0)); // Independent lineage survives, object does not.
    assert_eq!(
        queue.cancel_recv_if(|id, endpoint| !endpoint_close::receiver_allowed(
            &spaces,
            closed,
            if id == 1 { INIT } else { PROBE },
            endpoint.0
        )),
        2
    );
    for id in [1, 2] {
        assert_eq!(
            queue.slots.iter().find(|s| s.id == id).unwrap().state,
            ThreadState::Ready
        );
    }
    for id in [3, 4] {
        assert_eq!(
            queue.slots.iter().find(|s| s.id == id).unwrap().state,
            ThreadState::Blocked
        );
    }
    assert_eq!(queue.current, Some(5));
}

#[test]
fn single_boot_space_close_and_missing_object_are_handled_without_partial_delete() {
    let mut fabric = Fabric::new();
    let ep = fabric.create_endpoint(TenantId(1)).unwrap();
    let mut caps = CapTable::new(TenantId(1));
    let source = caps
        .mint(Capability::new(
            CapKind::Endpoint,
            CapRights::EP_FULL,
            ep.0,
            TenantId(1),
        ))
        .unwrap();
    let missing = caps
        .mint(Capability::new(
            CapKind::Endpoint,
            CapRights::EP_FULL,
            999,
            TenantId(1),
        ))
        .unwrap();
    let mut spaces = CapSpaces::new(caps);
    spaces.bind_init(INIT).unwrap();
    assert!(endpoint_close::close(&mut spaces, &mut fabric, INIT, missing).is_err());
    assert!(spaces.get(INIT).unwrap().lookup(missing).is_ok());
    assert_eq!(
        endpoint_close::close(&mut spaces, &mut fabric, INIT, source),
        Ok(ep)
    );
}
