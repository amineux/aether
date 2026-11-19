//! Exercise the kernel's delivery primitive against the real checked Fabric.
#[path = "../../kernel/src/ipc_delivery.rs"]
mod ipc_delivery;
use aether_core::caps::CapTable;
use aether_core::fabric::{
    ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags, MAX_QUEUE,
};
use aether_core::types::TenantId;
use ipc_delivery::deliver_next;

fn send(fabric: &mut Fabric, owner: TenantId, ep: EndpointId, badge: u64, payload: &[u8]) {
    let message = Message::new(
        ep,
        badge,
        MsgFlags(MsgFlags::ASYNC),
        ChipletRoute::LOCAL,
        owner,
        payload,
    )
    .unwrap();
    fabric.send_as(&CapTable::new(owner), message).unwrap();
}

#[test]
fn delivery_consumes_fifo_once_and_uses_the_queued_header() {
    let owner = TenantId(1);
    let mut fabric = Fabric::new();
    let ep = fabric.create_endpoint(owner).unwrap();
    send(&mut fabric, owner, ep, 0xA3, b"first");
    send(&mut fabric, owner, ep, 0xB4, b"second");
    for (badge, payload) in [(0xA3, &b"first"[..]), (0xB4, &b"second"[..])] {
        assert_eq!(
            deliver_next(&mut fabric, owner, ep, |got| {
                assert_eq!(got.header.badge, badge);
                assert_eq!(got.header.flags, MsgFlags(MsgFlags::ASYNC));
                assert_eq!(got.payload(), payload);
                Ok::<_, ()>(())
            }),
            Ok(Ok(()))
        );
    }
    assert_eq!(fabric.pending_as(owner, ep), Ok(0));
    let mut called = false;
    assert_eq!(
        deliver_next(&mut fabric, owner, ep, |_| {
            called = true;
            Ok::<_, ()>(())
        }),
        Err(FabricError::WouldBlock)
    );
    assert!(!called);
}

#[test]
fn delivery_releases_queue_capacity_for_repeated_blocked_receives() {
    let owner = TenantId(1);
    let mut fabric = Fabric::new();
    let ep = fabric.create_endpoint(owner).unwrap();
    for sequence in 0..(MAX_QUEUE + 2) {
        send(&mut fabric, owner, ep, sequence as u64, b"one");
        assert_eq!(
            deliver_next(&mut fabric, owner, ep, |got| {
                assert_eq!(got.header.badge, sequence as u64);
                Ok::<_, ()>(())
            }),
            Ok(Ok(()))
        );
        assert_eq!(fabric.pending_as(owner, ep), Ok(0));
    }
}

#[test]
fn foreign_receiver_cannot_dequeue_or_touch_output() {
    let owner = TenantId(1);
    let mut fabric = Fabric::new();
    let ep = fabric.create_endpoint(owner).unwrap();
    send(&mut fabric, owner, ep, 0xA3, b"private");
    let mut called = false;
    assert_eq!(
        deliver_next(&mut fabric, TenantId(2), ep, |_| {
            called = true;
            Ok::<_, ()>(())
        }),
        Err(FabricError::NoSuchEndpoint)
    );
    assert!(!called);
    assert_eq!(fabric.pending_as(owner, ep), Ok(1));
}

#[test]
fn failed_output_is_returned_without_fabricating_success_or_replaying_message() {
    let owner = TenantId(1);
    let mut fabric = Fabric::new();
    let ep = fabric.create_endpoint(owner).unwrap();
    send(&mut fabric, owner, ep, 0xA3, b"one");
    assert_eq!(
        deliver_next(&mut fabric, owner, ep, |_| Err(-3)),
        Ok(Err(-3))
    );
    assert_eq!(fabric.pending_as(owner, ep), Ok(0));
    assert_eq!(
        deliver_next(&mut fabric, owner, ep, |_| Ok::<_, i32>(())),
        Err(FabricError::WouldBlock)
    );
}
