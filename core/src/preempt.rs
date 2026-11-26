//! Host-testable preemptive / blocking CPU-thread queue.
//!
//! The fabric [`crate::sched::TileScheduler`] places Thread vs AccelWave
//! jobs on tiles. This module is the *other* scheduler: a tiny ready
//! queue with quantum rotation and waiter wake-up. The kernel's
//! `task.rs` mirrors these transitions on real stacks.

use crate::fabric::EndpointId;

pub const MAX_THREADS: usize = 8;
pub const DEFAULT_QUANTUM: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Empty,
    Ready,
    Running,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitWhy {
    None,
    Recv(u32),
    Accel(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadSlot {
    pub id: u32,
    pub state: ThreadState,
    pub wait: WaitWhy,
    pub slice_left: u32,
}

#[derive(Clone, Debug)]
pub struct CpuQueue {
    pub slots: [ThreadSlot; MAX_THREADS],
    pub current: Option<u32>,
    pub quantum: u32,
    pub switches: u32,
    pub ticks: u64,
}

impl CpuQueue {
    pub const fn new() -> Self {
        const EMPTY: ThreadSlot = ThreadSlot {
            id: 0,
            state: ThreadState::Empty,
            wait: WaitWhy::None,
            slice_left: 0,
        };
        Self {
            slots: [EMPTY; MAX_THREADS],
            current: None,
            quantum: DEFAULT_QUANTUM,
            switches: 0,
            ticks: 0,
        }
    }

    fn idx(&self, id: u32) -> Option<usize> {
        self.slots.iter().position(|s| s.state != ThreadState::Empty && s.id == id)
    }

    pub fn spawn(&mut self, id: u32) -> bool {
        if id == 0 || self.idx(id).is_some() {
            return false;
        }
        if let Some(slot) = self.slots.iter_mut().find(|s| s.state == ThreadState::Empty) {
            *slot = ThreadSlot {
                id,
                state: ThreadState::Ready,
                wait: WaitWhy::None,
                slice_left: self.quantum,
            };
            true
        } else {
            false
        }
    }

    pub fn ready_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.state == ThreadState::Ready || s.state == ThreadState::Running)
            .count()
    }

    pub fn blocked_count(&self) -> usize {
        self.slots.iter().filter(|s| s.state == ThreadState::Blocked).count()
    }

    fn pick_ready(&self, skip: Option<u32>) -> Option<u32> {
        let start = skip.and_then(|id| self.idx(id)).map(|i| i + 1).unwrap_or(0);
        for k in 0..MAX_THREADS {
            let i = (start + k) % MAX_THREADS;
            let s = &self.slots[i];
            if s.state == ThreadState::Ready && Some(s.id) != skip {
                return Some(s.id);
            }
        }
        None
    }

    fn set_state(&mut self, id: u32, st: ThreadState) {
        if let Some(i) = self.idx(id) {
            self.slots[i].state = st;
            if st != ThreadState::Blocked {
                self.slots[i].wait = WaitWhy::None;
            }
            if st == ThreadState::Ready || st == ThreadState::Running {
                self.slots[i].slice_left = self.quantum;
            }
        }
    }

    /// Elect a running thread if none. Returns the current id.
    pub fn ensure_running(&mut self) -> Option<u32> {
        if let Some(id) = self.current {
            if let Some(i) = self.idx(id) {
                if self.slots[i].state == ThreadState::Running {
                    return Some(id);
                }
            }
        }
        let next = self.pick_ready(None)?;
        self.set_state(next, ThreadState::Running);
        self.current = Some(next);
        Some(next)
    }

    /// Cooperative yield: current goes Ready, next Ready becomes Running.
    pub fn yield_now(&mut self) -> Option<u32> {
        let cur = self.current?;
        if let Some(next) = self.pick_ready(Some(cur)) {
            self.set_state(cur, ThreadState::Ready);
            self.set_state(next, ThreadState::Running);
            self.current = Some(next);
            self.switches += 1;
            Some(next)
        } else {
            Some(cur)
        }
    }

    /// Block current on `why`. Returns the thread that should run, if any.
    pub fn block(&mut self, why: WaitWhy) -> Option<u32> {
        let cur = self.current?;
        if let Some(i) = self.idx(cur) {
            self.slots[i].state = ThreadState::Blocked;
            self.slots[i].wait = why;
        }
        self.current = None;
        if let Some(next) = self.pick_ready(Some(cur)) {
            self.set_state(next, ThreadState::Running);
            self.current = Some(next);
            self.switches += 1;
            Some(next)
        } else {
            None
        }
    }

    pub fn wake_recv(&mut self, ep: EndpointId) -> u32 {
        self.wake(WaitWhy::Recv(ep.0))
    }

    /// Wake only the receiver whose output buffer has been completed.
    pub fn wake_recv_one(&mut self, ep: EndpointId, id: u32) -> bool {
        self.wake_one(WaitWhy::Recv(ep.0), id)
    }

    pub fn wake_accel(&mut self, queue: u32) -> u32 {
        self.wake(WaitWhy::Accel(queue))
    }

    /// A completion belongs to one waiter, not every thread on this queue.
    pub fn wake_accel_one(&mut self, queue: u32, id: u32) -> bool {
        self.wake_one(WaitWhy::Accel(queue), id)
    }

    /// Cancel selected blocked receives. The callback completes each selected
    /// syscall before its thread becomes Ready; other wait kinds are untouched.
    pub fn cancel_recv_if(&mut self, mut cancel: impl FnMut(u32, EndpointId) -> bool) -> u32 {
        let mut count = 0;
        for i in 0..MAX_THREADS {
            let slot = self.slots[i];
            if slot.state == ThreadState::Blocked {
                if let WaitWhy::Recv(ep) = slot.wait {
                    if cancel(slot.id, EndpointId(ep))
                        && self.wake_recv_one(EndpointId(ep), slot.id)
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    fn wake_one(&mut self, why: WaitWhy, id: u32) -> bool {
        let Some(i) = self.idx(id) else { return false };
        let slot = &mut self.slots[i];
        if slot.state != ThreadState::Blocked || slot.wait != why {
            return false;
        }
        slot.state = ThreadState::Ready;
        slot.wait = WaitWhy::None;
        slot.slice_left = self.quantum;
        true
    }

    fn wake(&mut self, why: WaitWhy) -> u32 {
        let mut n = 0;
        for s in self.slots.iter_mut() {
            if s.state == ThreadState::Blocked && s.wait == why {
                s.state = ThreadState::Ready;
                s.wait = WaitWhy::None;
                s.slice_left = self.quantum;
                n += 1;
            }
        }
        n
    }

    /// PIT tick. When the running slice expires and someone else is Ready,
    /// rotate. Returns `Some(new)` on a preemptive switch.
    pub fn tick(&mut self) -> Option<u32> {
        self.ticks += 1;
        let cur = self.ensure_running()?;
        let i = self.idx(cur)?;
        if self.slots[i].slice_left > 0 {
            self.slots[i].slice_left -= 1;
        }
        if self.slots[i].slice_left == 0 {
            if let Some(next) = self.pick_ready(Some(cur)) {
                self.set_state(cur, ThreadState::Ready);
                self.set_state(next, ThreadState::Running);
                self.current = Some(next);
                self.switches += 1;
                return Some(next);
            }
            self.slots[i].slice_left = self.quantum;
        }
        None
    }
}

impl Default for CpuQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targeted_accel_wake_leaves_other_waiters_blocked() {
        let mut q = CpuQueue::new();
        q.spawn(1);
        q.spawn(2);
        q.spawn(3);
        assert_eq!(q.ensure_running(), Some(1));
        assert_eq!(q.block(WaitWhy::Accel(7)), Some(2));
        assert_eq!(q.block(WaitWhy::Accel(7)), Some(3));
        assert_eq!(q.block(WaitWhy::Recv(7)), None);
        assert!(!q.wake_accel_one(8, 1));
        assert!(!q.wake_accel_one(7, 99));
        assert!(!q.wake_accel_one(7, 3));
        assert_eq!(q.blocked_count(), 3);
        assert!(q.wake_accel_one(7, 1));
        assert!(!q.wake_accel_one(7, 1));
        assert_eq!(q.blocked_count(), 2);
        assert_eq!(q.ensure_running(), Some(1));
        assert_eq!(q.block(WaitWhy::Accel(8)), None);
        assert!(!q.wake_accel_one(7, 1));
        assert!(q.wake_accel_one(7, 2));
        assert_eq!(q.blocked_count(), 2);
    }

    #[test]
    fn targeted_receive_wake_does_not_resume_other_waiters() {
        let mut q = CpuQueue::new();
        q.spawn(1);
        q.spawn(2);
        assert_eq!(q.ensure_running(), Some(1));
        assert_eq!(q.block(WaitWhy::Recv(7)), Some(2));
        assert_eq!(q.block(WaitWhy::Recv(7)), None);
        assert!(!q.wake_recv_one(EndpointId(8), 1));
        assert!(!q.wake_recv_one(EndpointId(7), 99));
        assert_eq!(q.blocked_count(), 2);
        assert!(q.wake_recv_one(EndpointId(7), 1));
        assert_eq!(q.blocked_count(), 1);
        assert!(!q.wake_recv_one(EndpointId(7), 1));
        assert_eq!(q.ensure_running(), Some(1));
        assert_eq!(q.block(WaitWhy::Accel(7)), None);
        assert!(!q.wake_recv_one(EndpointId(7), 1));
        assert!(q.wake_recv_one(EndpointId(7), 2));
        assert_eq!(q.blocked_count(), 1); // accelerator waiter is unaffected
    }

    #[test]
    fn two_threads_preempt_round_robin() {
        let mut q = CpuQueue::new();
        q.quantum = 1;
        assert!(q.spawn(1));
        assert!(q.spawn(2));
        assert_eq!(q.ensure_running(), Some(1));
        assert_eq!(q.tick(), Some(2));
        assert_eq!(q.current, Some(2));
        assert_eq!(q.tick(), Some(1));
        assert_eq!(q.tick(), Some(2));
        assert!(q.switches >= 3);
    }

    #[test]
    fn yield_switches_when_peer_ready() {
        let mut q = CpuQueue::new();
        q.spawn(1);
        q.spawn(2);
        q.ensure_running();
        assert_eq!(q.yield_now(), Some(2));
        assert_eq!(q.yield_now(), Some(1));
    }

    #[test]
    fn recv_blocks_and_wake_reschedules() {
        let mut q = CpuQueue::new();
        q.spawn(1);
        q.spawn(2);
        q.ensure_running();
        assert_eq!(q.block(WaitWhy::Recv(7)), Some(2));
        assert_eq!(q.blocked_count(), 1);
        assert_eq!(q.current, Some(2));
        assert_eq!(q.wake_recv(EndpointId(7)), 1);
        assert_eq!(q.ready_count(), 2);
        assert_eq!(q.yield_now(), Some(1));
        assert_eq!(q.current, Some(1));
    }

    #[test]
    fn accel_wait_blocks_until_complete() {
        let mut q = CpuQueue::new();
        q.spawn(1);
        q.spawn(2);
        q.ensure_running();
        assert_eq!(q.block(WaitWhy::Accel(1)), Some(2));
        assert_eq!(q.wake_accel(1), 1);
        assert!(q.slots.iter().any(|s| s.id == 1 && s.state == ThreadState::Ready));
    }

    #[test]
    fn yield_alone_does_not_spin_off_cpu() {
        let mut q = CpuQueue::new();
        q.spawn(1);
        q.ensure_running();
        assert_eq!(q.yield_now(), Some(1));
        assert_eq!(q.current, Some(1));
    }

    #[test]
    fn four_threads_round_robin_includes_clone() {
        // kthread + /init + /probe + SYS_CLONE sibling.
        let mut q = CpuQueue::new();
        q.quantum = 1;
        assert!(q.spawn(1));
        assert!(q.spawn(2));
        assert!(q.spawn(3));
        assert!(q.spawn(4));
        assert_eq!(q.ensure_running(), Some(1));
        assert_eq!(q.tick(), Some(2));
        assert_eq!(q.tick(), Some(3));
        assert_eq!(q.tick(), Some(4));
        assert_eq!(q.tick(), Some(1));
        assert!(q.switches >= 4);
    }

    #[test]
    fn spawn_refuses_zero_duplicate_and_full_table() {
        let mut q = CpuQueue::new();
        assert!(!q.spawn(0));
        assert!(q.spawn(1));
        assert!(!q.spawn(1));
        for id in 2..=MAX_THREADS as u32 {
            assert!(q.spawn(id));
        }
        assert!(!q.spawn(MAX_THREADS as u32 + 1));
        assert_eq!(q.ready_count(), MAX_THREADS);
        assert!(q.slots.iter().all(|s| s.slice_left == DEFAULT_QUANTUM));
    }

    #[test]
    fn empty_queue_ops_are_noops_but_ticks_count() {
        let mut q = CpuQueue::new();
        assert_eq!(q.ensure_running(), None);
        assert_eq!(q.yield_now(), None);
        assert_eq!(q.block(WaitWhy::Recv(1)), None);
        assert_eq!(q.tick(), None);
        assert_eq!(q.tick(), None);
        assert_eq!(q.ticks, 2);
        assert_eq!(q.switches, 0);
        assert_eq!(q.current, None);
        assert_eq!(q.blocked_count(), 0);
    }

    #[test]
    fn tick_counts_down_slice_before_rotating() {
        let mut q = CpuQueue::new();
        q.quantum = 3;
        q.spawn(1);
        q.spawn(2);
        assert_eq!(q.tick(), None); // elects 1, slice 3 -> 2
        assert_eq!(q.current, Some(1));
        assert_eq!(q.tick(), None);
        assert_eq!(q.tick(), Some(2));
        assert_eq!(q.switches, 1);
        assert_eq!(q.ticks, 3);
        let s1 = q.slots.iter().find(|s| s.id == 1).unwrap();
        assert_eq!((s1.state, s1.slice_left), (ThreadState::Ready, 3));
        let s2 = q.slots.iter().find(|s| s.id == 2).unwrap();
        assert_eq!((s2.state, s2.slice_left), (ThreadState::Running, 3));
    }

    #[test]
    fn lone_thread_tick_refills_slice_without_switching() {
        let mut q = CpuQueue::new();
        q.spawn(1);
        for _ in 0..5 {
            assert_eq!(q.tick(), None);
            assert_eq!(q.current, Some(1));
            assert!(q.slots[0].slice_left >= 1 && q.slots[0].slice_left <= DEFAULT_QUANTUM);
        }
        assert_eq!(q.switches, 0);
    }

    #[test]
    fn broadcast_wake_only_matches_exact_wait_reason() {
        let mut q = CpuQueue::new();
        for id in 1..=4 {
            q.spawn(id);
        }
        q.ensure_running();
        assert_eq!(q.block(WaitWhy::Recv(7)), Some(2));
        assert_eq!(q.block(WaitWhy::Recv(7)), Some(3));
        assert_eq!(q.block(WaitWhy::Recv(9)), Some(4));
        assert_eq!(q.block(WaitWhy::Accel(7)), None);
        assert_eq!(q.blocked_count(), 4);
        assert_eq!(q.wake_recv(EndpointId(8)), 0);
        assert_eq!(q.wake_recv(EndpointId(7)), 2);
        assert_eq!(q.wake_recv(EndpointId(7)), 0);
        assert_eq!(q.blocked_count(), 2);
        for s in q.slots.iter().filter(|s| s.id == 1 || s.id == 2) {
            assert_eq!(
                (s.state, s.wait, s.slice_left),
                (ThreadState::Ready, WaitWhy::None, q.quantum)
            );
        }
        assert_eq!(q.wake_accel(9), 0);
        assert_eq!(q.wake_accel(7), 1);
        assert_eq!(q.wake_recv(EndpointId(9)), 1);
        assert_eq!(q.blocked_count(), 0);
    }

    #[test]
    fn cancel_recv_if_wakes_only_selected_receive_waiters() {
        let mut q = CpuQueue::new();
        for id in 1..=4 {
            q.spawn(id);
        }
        q.ensure_running();
        assert_eq!(q.block(WaitWhy::Recv(7)), Some(2));
        assert_eq!(q.block(WaitWhy::Recv(8)), Some(3));
        assert_eq!(q.block(WaitWhy::Accel(7)), Some(4));
        let mut seen = [(0u32, 0u32); MAX_THREADS];
        let mut n = 0;
        let woke = q.cancel_recv_if(|id, ep| {
            seen[n] = (id, ep.0);
            n += 1;
            ep.0 == 7
        });
        assert_eq!(woke, 1);
        // The callback sees receive waiters only, in slot order.
        assert_eq!(&seen[..n], &[(1, 7), (2, 8)]);
        let s = |q: &CpuQueue, id| *q.slots.iter().find(|s| s.id == id).unwrap();
        assert_eq!(
            (s(&q, 1).state, s(&q, 1).wait),
            (ThreadState::Ready, WaitWhy::None)
        );
        assert_eq!(
            (s(&q, 2).state, s(&q, 2).wait),
            (ThreadState::Blocked, WaitWhy::Recv(8))
        );
        assert_eq!(
            (s(&q, 3).state, s(&q, 3).wait),
            (ThreadState::Blocked, WaitWhy::Accel(7))
        );
        assert_eq!(q.current, Some(4));
        assert_eq!(q.cancel_recv_if(|_, _| false), 0);
        assert_eq!(q.blocked_count(), 2);
        assert_eq!(q.cancel_recv_if(|_, _| true), 1);
        assert_eq!(q.blocked_count(), 1); // accelerator waiter is never cancelled
    }

    #[test]
    fn rotation_skips_blocked_slots_and_wraps() {
        let mut q = CpuQueue::new();
        q.spawn(1);
        q.spawn(2);
        q.spawn(3);
        q.ensure_running();
        assert_eq!(q.yield_now(), Some(2));
        assert_eq!(q.block(WaitWhy::Recv(5)), Some(3));
        assert_eq!(q.yield_now(), Some(1)); // wraps past blocked 2
        assert_eq!(q.yield_now(), Some(3));
        assert_eq!(q.switches, 4);
        assert!(q.wake_recv_one(EndpointId(5), 2));
        assert_eq!(q.yield_now(), Some(1)); // slot order after 3 wraps to 1
        assert_eq!(q.yield_now(), Some(2));
    }
}
