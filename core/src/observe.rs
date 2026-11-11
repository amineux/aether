//! Structured kernel events (ring buffer). Early observability for
//! sched / IPC / accel — what a silicon bring-up engineer actually needs.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum EventKind {
    Boot = 1,
    IpcSend = 2,
    IpcRecv = 3,
    CapMint = 4,
    CapGrant = 5,
    ArenaAlloc = 6,
    ArenaXfer = 7,
    SchedPick = 8,
    SchedSteal = 9,
    AccelSubmit = 10,
    AccelComplete = 11,
    IsolationDeny = 12,
    Warn = 13,
    CutBind = 14,
    CutRefuse = 15,
    HodgeAdmit = 16,
    HodgeRefuse = 17,
    ActivityBind = 18,
    FenceSubmit = 19,
    FenceComplete = 20,
    SpaceRefuse = 21,
    CapRevoke = 22,
    /// Soft SMMU pinned a [`crate::window::TypedWindow`].
    WindowMap = 23,
    /// Foreign-tenant typed window (SpectralCut `CrossCut`) or wrong SID.
    WindowRefuse = 24,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelEvent {
    pub ts: u64,
    pub kind: EventKind,
    pub a: u64,
    pub b: u64,
}

pub const RING_CAP: usize = 64;

#[derive(Clone, Debug)]
pub struct EventRing {
    buf: [Option<KernelEvent>; RING_CAP],
    head: usize,
    len: usize,
    ts: u64,
}

impl EventRing {
    pub const fn new() -> Self {
        Self {
            buf: [None; RING_CAP],
            head: 0,
            len: 0,
            ts: 0,
        }
    }

    pub fn emit(&mut self, kind: EventKind, a: u64, b: u64) {
        self.ts += 1;
        let ev = KernelEvent {
            ts: self.ts,
            kind,
            a,
            b,
        };
        let i = (self.head + self.len) % RING_CAP;
        if self.len == RING_CAP {
            self.buf[self.head] = Some(ev);
            self.head = (self.head + 1) % RING_CAP;
        } else {
            self.buf[i] = Some(ev);
            self.len += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// True when no events are buffered.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn last(&self) -> Option<KernelEvent> {
        if self.len == 0 {
            return None;
        }
        let i = (self.head + self.len - 1) % RING_CAP;
        self.buf[i]
    }

    pub fn iter(&self) -> impl Iterator<Item = KernelEvent> + '_ {
        (0..self.len).filter_map(move |k| self.buf[(self.head + k) % RING_CAP])
    }

    pub fn count_kind(&self, kind: EventKind) -> usize {
        self.iter().filter(|e| e.kind == kind).count()
    }
}

impl Default for EventRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_empty_tracks_len() {
        let mut r = EventRing::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        r.emit(EventKind::Boot, 0, 0);
        assert!(!r.is_empty());
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn wrap_preserves_newest() {
        let mut r = EventRing::new();
        for i in 0..(RING_CAP as u64 + 5) {
            r.emit(EventKind::Boot, i, 0);
        }
        assert_eq!(r.len(), RING_CAP);
        assert_eq!(r.last().unwrap().a, RING_CAP as u64 + 4);
        let first = r.iter().next().unwrap();
        assert_eq!(first.a, 5);
    }

    #[test]
    fn count_kind() {
        let mut r = EventRing::new();
        r.emit(EventKind::IpcSend, 0, 0);
        r.emit(EventKind::IpcSend, 1, 0);
        r.emit(EventKind::AccelComplete, 2, 0);
        assert_eq!(r.count_kind(EventKind::IpcSend), 2);
        assert_eq!(r.count_kind(EventKind::AccelComplete), 1);
    }

    #[test]
    fn empty_ring_has_no_last_and_wrap_keeps_order_and_timestamps() {
        let mut r = EventRing::default();
        assert_eq!(r.last(), None);
        assert_eq!(r.iter().count(), 0);
        let total = 2 * RING_CAP as u64 + 3;
        for i in 0..total {
            r.emit(EventKind::SchedPick, i, i + 1);
        }
        let evs: Vec<KernelEvent> = r.iter().collect();
        assert_eq!(evs.len(), RING_CAP);
        // Oldest survivor first; timestamps are 1-based and strictly consecutive.
        assert_eq!(evs[0].ts, total - RING_CAP as u64 + 1);
        for w in evs.windows(2) {
            assert_eq!(w[1].ts, w[0].ts + 1);
            assert_eq!(w[1].a, w[0].a + 1);
        }
        assert_eq!(r.last().unwrap().ts, total);
        assert_eq!(r.last(), evs.last().copied());
    }

    #[test]
    fn count_kind_drops_evicted_events_after_wrap() {
        let mut r = EventRing::new();
        r.emit(EventKind::IsolationDeny, 0, 0);
        r.emit(EventKind::IsolationDeny, 1, 0);
        for i in 0..(RING_CAP as u64 - 1) {
            r.emit(EventKind::AccelSubmit, i, 0);
        }
        // One IsolationDeny was overwritten by the wrap; the other survives.
        assert_eq!(r.len(), RING_CAP);
        assert_eq!(r.count_kind(EventKind::IsolationDeny), 1);
        assert_eq!(r.count_kind(EventKind::AccelSubmit), RING_CAP - 1);
        assert_eq!(r.iter().next().unwrap().a, 1);
        assert_eq!(r.count_kind(EventKind::Boot), 0);
    }
}
