//! CP-shaped fence / timeline: submit → wait → complete (or timeout).
//!
//! This is a **software model** of what a command processor would retire:
//! a named [`TimelineId`], monotonic seq ([`FenceId`]), in-order retire,
//! and a credit limit on outstanding seqs. It is **not** a silicon
//! timeline, not a CUDA stream, not a Vulkan timeline product, and not
//! a hardware fence unit.
//!
//! Scoped (wave / CU / chiplet / package) timelines, Fleet-shaped
//! hierarchical counters, and optional CPElide CCT elision live in
//! [`crate::chipsync::SoftChipletSync`]. That is **not** UCIe sync and
//! not ChipletFleet placement.
//!
//! Hardware-shaped (what a CP mailbox / IRQ would name):
//! - [`Timeline::submit`] allocates the next seq and takes a credit.
//! - [`Timeline::wait`] polls the retired watermark (`retired >= seq`).
//! - [`Timeline::complete`] is in-order retire (the IRQ path).
//!
//! Soft-only overlays (not what a device writes):
//! - [`Timeline::timeout`] releases a credit without claiming an IRQ.
//! - Credit accounting itself is the partition QoS meter.
//!
//! `wait_for` on submit records a predecessor. An already-issued but
//! not-yet-retired pred is allowed (the CP would stall). A never-issued
//! pred is refused. That is not the old "refuse submit until pred
//! completes" host-side shortcut.

use crate::partition::{PartitionError, PartitionId, PartitionProfile};

/// Named timeline a CP would address. Defaults to the partition id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimelineId(pub u32);

/// Monotonic sequence on a timeline.
///
/// This is the value stored in `AccelJobDesc.fence_id` and `CpCmd`
/// (`u64`, ABI-stable). It is a seq, not a CUDA event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FenceId(pub u64);

/// Software bound on outstanding seqs. Partition credits may be lower.
pub const MAX_IN_FLIGHT: u32 = 32;

/// A point on a named timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fence {
    pub id: FenceId,
    pub timeline: TimelineId,
    pub partition: PartitionId,
    pub wait_for: Option<FenceId>,
    pub submitted: bool,
    pub completed: bool,
    pub timed_out: bool,
}

impl Fence {
    pub const fn new(id: FenceId, timeline: TimelineId, partition: PartitionId) -> Self {
        Self {
            id,
            timeline,
            partition,
            wait_for: None,
            submitted: false,
            completed: false,
            timed_out: false,
        }
    }

    pub const fn after(mut self, pred: FenceId) -> Self {
        self.wait_for = Some(pred);
        self
    }

    /// Seq a CP would retire. Same as `id.0`.
    pub const fn seq(self) -> u64 {
        self.id.0
    }

    const fn after_opt(mut self, pred: Option<FenceId>) -> Self {
        self.wait_for = pred;
        self
    }
}

/// Per-partition credit + CP-shaped fence timeline.
pub struct Timeline {
    id: TimelineId,
    partition: PartitionId,
    next: u64,
    retired: u64,
    in_flight: u32,
    /// `wait_for` seq of outstanding jobs, index 0 = next to retire.
    preds: [Option<u64>; MAX_IN_FLIGHT as usize],
    last_timeout: Option<FenceId>,
}

impl Timeline {
    /// Timeline id equals the partition id (the usual QEMU / Soft-CP case).
    pub const fn new(partition: PartitionId) -> Self {
        Self::named(TimelineId(partition.0), partition)
    }

    /// Named timeline. SoftChipletSync uses one id per [`crate::chipsync::SyncScope`].
    pub const fn named(id: TimelineId, partition: PartitionId) -> Self {
        Self {
            id,
            partition,
            next: 1,
            retired: 0,
            in_flight: 0,
            preds: [None; MAX_IN_FLIGHT as usize],
            last_timeout: None,
        }
    }

    pub const fn id(&self) -> TimelineId {
        self.id
    }

    pub const fn partition(&self) -> PartitionId {
        self.partition
    }

    /// Next seq a submit would allocate.
    pub const fn next_seq(&self) -> u64 {
        self.next
    }

    /// Retired watermark: every seq `<= retired` has complete or timeout.
    pub const fn retired(&self) -> u64 {
        self.retired
    }

    pub const fn in_flight(&self) -> u32 {
        self.in_flight
    }

    fn issued(&self, id: FenceId) -> bool {
        id.0 > 0 && id.0 < self.next
    }

    fn snapshot(&self, id: FenceId, wait_for: Option<FenceId>) -> Fence {
        let settled = id.0 > 0 && id.0 <= self.retired;
        let timed_out = settled && self.last_timeout == Some(id);
        Fence {
            id,
            timeline: self.id,
            partition: self.partition,
            wait_for,
            submitted: self.issued(id) || settled,
            completed: settled && !timed_out,
            timed_out,
        }
    }

    /// Allocate the next seq and take a credit.
    ///
    /// `wait_for` must be a previously issued seq, or `None`. An issued
    /// but not-yet-retired pred is allowed (CP stall). A never-issued
    /// pred is [`PartitionError::FenceNotReady`].
    pub fn submit(
        &mut self,
        profile: &PartitionProfile,
        wait_for: Option<FenceId>,
    ) -> Result<Fence, PartitionError> {
        if profile.id.0 != self.partition.0 {
            return Err(PartitionError::Unbound);
        }
        if self.in_flight >= profile.qos.credits || self.in_flight >= MAX_IN_FLIGHT {
            return Err(PartitionError::CreditExhausted);
        }
        if let Some(w) = wait_for {
            if !self.issued(w) && w.0 != 0 {
                return Err(PartitionError::FenceNotReady);
            }
            if w.0 == 0 {
                return Err(PartitionError::FenceNotReady);
            }
        }
        let seq = FenceId(self.next);
        self.preds[self.in_flight as usize] = wait_for.map(|w| w.0);
        self.next += 1;
        self.in_flight += 1;
        Ok(Fence {
            submitted: true,
            ..Fence::new(seq, self.id, self.partition).after_opt(wait_for)
        })
    }

    /// Poll the retired watermark. Does not consume a credit.
    pub fn wait(&self, id: FenceId) -> Result<Fence, PartitionError> {
        if id.0 == 0 || id.0 > self.retired || id.0 >= self.next {
            return Err(PartitionError::FenceNotReady);
        }
        Ok(self.snapshot(id, None))
    }

    /// In-order CP retire. `id` must be `retired + 1`.
    ///
    /// If that seq named a `wait_for` that is not yet retired, refuse
    /// ([`PartitionError::FenceNotReady`]) — a CP would not retire it.
    pub fn complete(&mut self, id: FenceId) -> Result<Fence, PartitionError> {
        self.settle(id, false)
    }

    /// Software timeout: release the head credit without claiming an IRQ.
    ///
    /// Same in-order rule as [`Self::complete`]. The watermark advances
    /// so the timeline is not stuck; [`Self::wait`] reports `timed_out`
    /// and a later [`Self::complete`] of this seq is refused.
    pub fn timeout(&mut self, id: FenceId) -> Result<Fence, PartitionError> {
        self.settle(id, true)
    }

    /// IRQ helper: retire a named seq. `0` means the job named no fence.
    pub fn retire_seq(&mut self, fence_id: u64) -> Result<Option<Fence>, PartitionError> {
        if fence_id == 0 {
            return Ok(None);
        }
        self.complete(FenceId(fence_id)).map(Some)
    }

    fn settle(&mut self, id: FenceId, timed_out: bool) -> Result<Fence, PartitionError> {
        if self.in_flight == 0 || id.0 != self.retired + 1 || id.0 >= self.next {
            return Err(if id.0 > 0 && id.0 <= self.retired {
                PartitionError::Unbound
            } else {
                PartitionError::FenceNotReady
            });
        }
        let wait_for = self.preds[0].map(FenceId);
        if !timed_out {
            if let Some(pred) = wait_for {
                if pred.0 > self.retired {
                    return Err(PartitionError::FenceNotReady);
                }
            }
        }
        self.shift_preds();
        self.in_flight -= 1;
        self.retired = id.0;
        if timed_out {
            self.last_timeout = Some(id);
        } else if self.last_timeout == Some(id) {
            self.last_timeout = None;
        }
        Ok(Fence {
            id,
            timeline: self.id,
            partition: self.partition,
            wait_for,
            submitted: true,
            completed: !timed_out,
            timed_out,
        })
    }

    fn shift_preds(&mut self) {
        let n = self.in_flight as usize;
        if n <= 1 {
            self.preds[0] = None;
            return;
        }
        for i in 0..n - 1 {
            self.preds[i] = self.preds[i + 1];
        }
        self.preds[n - 1] = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{BlastRadius, PartitionId, PartitionProfile, QosBudget, SpatialSlice};
    use crate::types::ChipletId;

    fn profile(credits: u32) -> PartitionProfile {
        PartitionProfile::new(
            PartitionId(1),
            SpatialSlice::single_chiplet(ChipletId(0), 2, 0b1),
            QosBudget {
                bw_mbps: 100,
                credits,
            },
            BlastRadius {
                max_nodes: 2,
                max_hops: 1,
            },
        )
    }

    #[test]
    fn submit_wait_complete() {
        let p = profile(2);
        let mut t = Timeline::new(PartitionId(1));
        assert_eq!(t.id(), TimelineId(1));
        let a = t.submit(&p, None).unwrap();
        assert!(a.submitted);
        assert_eq!(a.seq(), 1);
        assert_eq!(a.timeline, TimelineId(1));
        assert_eq!(t.wait(a.id), Err(PartitionError::FenceNotReady));
        let done = t.complete(a.id).unwrap();
        assert!(done.completed);
        assert!(!done.timed_out);
        let ready = t.wait(a.id).unwrap();
        assert!(ready.completed);
        assert_eq!(t.retired(), 1);
        assert_eq!(t.in_flight(), 0);
    }

    #[test]
    fn credit_exhausted_refuses() {
        let p = profile(1);
        let mut t = Timeline::new(PartitionId(1));
        let a = t.submit(&p, None).unwrap();
        assert_eq!(t.submit(&p, None), Err(PartitionError::CreditExhausted));
        t.complete(a.id).unwrap();
        assert!(t.submit(&p, Some(a.id)).unwrap().submitted);
    }

    #[test]
    fn timeout_refuses_later_complete() {
        let p = profile(2);
        let mut t = Timeline::new(PartitionId(1));
        let a = t.submit(&p, None).unwrap();
        let timed = t.timeout(a.id).unwrap();
        assert!(timed.timed_out);
        assert!(!timed.completed);
        assert_eq!(t.complete(a.id), Err(PartitionError::Unbound));
        let seen = t.wait(a.id).unwrap();
        assert!(seen.timed_out);
        assert!(!seen.completed);
        assert_eq!(t.in_flight(), 0);
    }

    #[test]
    fn never_issued_wait_for_refused() {
        let p = profile(4);
        let mut t = Timeline::new(PartitionId(1));
        assert_eq!(
            t.submit(&p, Some(FenceId(99))),
            Err(PartitionError::FenceNotReady)
        );
        assert_eq!(t.wait(FenceId(99)), Err(PartitionError::FenceNotReady));
    }

    #[test]
    fn multi_job_in_order() {
        let p = profile(4);
        let mut t = Timeline::named(TimelineId(7), PartitionId(1));
        let a = t.submit(&p, None).unwrap();
        // Issued-but-not-retired pred is allowed (CP stall). Soft-only
        // "refuse submit until pred completes" is not the model.
        let b = t.submit(&p, Some(a.id)).unwrap();
        assert_eq!(b.wait_for, Some(a.id));
        assert_eq!(t.wait(a.id), Err(PartitionError::FenceNotReady));
        assert_eq!(t.wait(b.id), Err(PartitionError::FenceNotReady));
        assert_eq!(t.complete(b.id), Err(PartitionError::FenceNotReady));
        t.complete(a.id).unwrap();
        assert!(t.wait(a.id).unwrap().completed);
        assert_eq!(t.wait(b.id), Err(PartitionError::FenceNotReady));
        t.complete(b.id).unwrap();
        assert!(t.wait(b.id).unwrap().completed);
        assert_eq!(t.retired(), 2);
        assert_eq!(t.in_flight(), 0);
    }

    #[test]
    fn retire_seq_zero_is_noop() {
        let p = profile(1);
        let mut t = Timeline::new(PartitionId(1));
        let a = t.submit(&p, None).unwrap();
        assert!(t.retire_seq(0).unwrap().is_none());
        assert_eq!(t.in_flight(), 1);
        assert!(t.retire_seq(a.seq()).unwrap().unwrap().completed);
    }

    #[test]
    fn unbound_profile_refused() {
        let p = profile(2);
        let mut t = Timeline::new(PartitionId(2));
        assert_eq!(t.submit(&p, None), Err(PartitionError::Unbound));
    }
}
