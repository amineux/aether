//! The exact retirement policy used by task::exit_user_thread.
#[path = "../../kernel/src/thread_exit.rs"]
mod thread_exit;
use aether_core::fabric::EndpointId;
use aether_core::preempt::{CpuQueue, ThreadState, WaitWhy};

fn snapshot(q: &CpuQueue) -> impl PartialEq + std::fmt::Debug {
    (q.slots, q.current, q.quantum, q.switches, q.ticks)
}
fn queue() -> CpuQueue {
    let mut q = CpuQueue::new();
    for id in 1..=3 {
        assert!(q.spawn(id));
    }
    assert_eq!(q.ensure_running(), Some(1));
    q
}

#[test]
fn shared_root_retirement_removes_only_caller_and_rotates_to_ready_successor() {
    let mut q = queue();
    let before_switches = q.switches;
    assert_eq!(
        thread_exit::retire_shared(&mut q, 1, 0x1000, |id| Some(if id == 3 {
            0x2000
        } else {
            0x1000
        })),
        Some(2)
    );
    assert_eq!(q.current, Some(2));
    assert_eq!(q.switches, before_switches + 1);
    assert!(!q
        .slots
        .iter()
        .any(|s| s.id == 1 && s.state != ThreadState::Empty));
    assert_eq!(
        q.slots.iter().find(|s| s.id == 3).unwrap().state,
        ThreadState::Ready
    );
    assert!(!q.wake_recv_one(EndpointId(1), 1));
    assert!(!q.wake_accel_one(1, 1));
    assert_eq!(
        q.cancel_recv_if(|_, _| panic!("retired slot cannot be a receiver")),
        0
    );
}

#[test]
fn kernel_or_other_root_threads_do_not_count_as_live_siblings() {
    let mut q = queue();
    let before = snapshot(&q);
    assert_eq!(
        thread_exit::retire_shared(&mut q, 1, 0x1000, |id| match id {
            1 => Some(0x1000),
            2 => None,
            _ => Some(0x2000),
        }),
        None
    );
    assert_eq!(snapshot(&q), before);
}

#[test]
fn shared_but_blocked_sibling_cannot_leave_exit_without_a_ready_successor() {
    let mut q = CpuQueue::new();
    q.spawn(1);
    q.spawn(2);
    q.ensure_running();
    q.block(WaitWhy::Recv(7)); // Thread 2 is Running; thread 1 is blocked.
    let before = snapshot(&q);
    assert_eq!(
        thread_exit::retire_shared(&mut q, 2, 0x1000, |_| Some(0x1000)),
        None
    );
    assert_eq!(snapshot(&q), before);
}

#[test]
fn blocked_sibling_keeps_root_live_when_kernel_successor_can_run() {
    let mut q = queue();
    q.block(WaitWhy::Accel(1)); // 1 blocks, 2 runs, 3 stays Ready (kernel).
    assert_eq!(
        thread_exit::retire_shared(&mut q, 2, 0x1000, |id| if id == 3 {
            None
        } else {
            Some(0x1000)
        }),
        Some(3)
    );
    assert_eq!(
        q.slots.iter().find(|s| s.id == 1).unwrap().wait,
        WaitWhy::Accel(1)
    );
    assert!(q.wake_accel_one(1, 1));
}

#[test]
fn invalid_caller_root_and_nonrunning_current_refuse_without_queue_mutation() {
    for (current, root) in [(0, 0x1000), (2, 0x1000), (1, 0), (1, 0x2000)] {
        let mut q = queue();
        let before = snapshot(&q);
        assert_eq!(
            thread_exit::retire_shared(&mut q, current, root, |_| Some(0x1000)),
            None
        );
        assert_eq!(snapshot(&q), before);
    }
    let mut q = queue();
    q.slots[0].state = ThreadState::Blocked;
    let before = snapshot(&q);
    assert_eq!(q.finish_current(), None);
    assert_eq!(snapshot(&q), before);
}

#[test]
fn an_empty_queue_or_retired_slot_cannot_authorize_a_later_root_exit() {
    let mut empty = CpuQueue::new();
    let before = snapshot(&empty);
    assert_eq!(empty.finish_current(), None);
    assert_eq!(snapshot(&empty), before);
    let mut q = CpuQueue::new();
    q.spawn(1);
    q.spawn(2);
    q.spawn(3);
    q.ensure_running();
    assert_eq!(
        thread_exit::retire_shared(&mut q, 1, 0x1000, |id| if id == 3 {
            None
        } else {
            Some(0x1000)
        }),
        Some(2)
    );
    let before = snapshot(&q);
    // A ready kernel successor exists, but stale root-provider data for an
    // empty slot must not count as a live sibling.
    assert_eq!(
        thread_exit::retire_shared(&mut q, 2, 0x1000, |id| if id == 3 {
            None
        } else {
            Some(0x1000)
        }),
        None
    );
    assert_eq!(snapshot(&q), before);
}
