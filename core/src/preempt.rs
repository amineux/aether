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

    pub fn wake_accel(&mut self, queue: u32) -> u32 {
        self.wake(WaitWhy::Accel(queue))
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
}
