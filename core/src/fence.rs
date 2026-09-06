//! Fence-ordered jobs: submit → fence/timeline → complete/timeout.
//!
//! This is not a CUDA stream. Ordering is a first-class fence object.
//! Submission is credit-limited per partition.

use crate::partition::{PartitionError, PartitionId, PartitionProfile};

/// Monotonic fence / timeline identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FenceId(pub u64);

/// A point on a per-partition timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fence {
    pub id: FenceId,
    pub partition: PartitionId,
    pub wait_for: Option<FenceId>,
    pub submitted: bool,
    pub completed: bool,
    pub timed_out: bool,
}

impl Fence {
    pub const fn new(id: FenceId, partition: PartitionId) -> Self {
        Self {
            id,
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
}

/// Per-partition credit + fence timeline.
pub struct Timeline {
    partition: PartitionId,
    next: u64,
    in_flight: u32,
    last_complete: Option<FenceId>,
}

impl Timeline {
    pub const fn new(partition: PartitionId) -> Self {
        Self {
            partition,
            next: 1,
            in_flight: 0,
            last_complete: None,
        }
    }

    pub fn submit(
        &mut self,
        profile: &PartitionProfile,
        wait_for: Option<FenceId>,
    ) -> Result<Fence, PartitionError> {
        if profile.id.0 != self.partition.0 {
            return Err(PartitionError::Unbound);
        }
        if self.in_flight >= profile.qos.credits {
            return Err(PartitionError::CreditExhausted);
        }
        if let Some(w) = wait_for {
            match self.last_complete {
                Some(c) if c.0 >= w.0 => {}
                _ => return Err(PartitionError::FenceNotReady),
            }
        }
        let f = Fence::new(FenceId(self.next), self.partition).after_opt(wait_for);
        self.next += 1;
        self.in_flight += 1;
        Ok(Fence {
            submitted: true,
            ..f
        })
    }

    pub fn complete(&mut self, id: FenceId) -> Result<Fence, PartitionError> {
        if self.in_flight == 0 {
            return Err(PartitionError::Unbound);
        }
        self.in_flight -= 1;
        self.last_complete = Some(id);
        Ok(Fence {
            id,
            partition: self.partition,
            wait_for: None,
            submitted: true,
            completed: true,
            timed_out: false,
        })
    }

    pub fn timeout(&mut self, id: FenceId) -> Fence {
        if self.in_flight > 0 {
            self.in_flight -= 1;
        }
        Fence {
            id,
            partition: self.partition,
            wait_for: None,
            submitted: true,
            completed: false,
            timed_out: true,
        }
    }

    pub const fn in_flight(&self) -> u32 {
        self.in_flight
    }
}

impl Fence {
    const fn after_opt(mut self, pred: Option<FenceId>) -> Self {
        self.wait_for = pred;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChipletId;
    use crate::partition::{BlastRadius, PartitionId, PartitionProfile, QosBudget, SpatialSlice};

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
    fn credit_limited_not_a_stream() {
        let p = profile(1);
        let mut t = Timeline::new(PartitionId(1));
        let a = t.submit(&p, None).unwrap();
        assert!(a.submitted);
        assert_eq!(t.submit(&p, None), Err(PartitionError::CreditExhausted));
        t.complete(a.id).unwrap();
        assert!(t.submit(&p, Some(a.id)).unwrap().submitted);
    }

    #[test]
    fn wait_before_ready_refused() {
        let p = profile(4);
        let mut t = Timeline::new(PartitionId(1));
        assert_eq!(
            t.submit(&p, Some(FenceId(99))),
            Err(PartitionError::FenceNotReady)
        );
    }
}
