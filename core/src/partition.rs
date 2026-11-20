//! Partition profile: spatial slice + QoS + blast-radius isolation.
//!
//! Isolation is spatial (slices/columns) first, temporal second.
//! The scheduler and every accel activity bind to a partition; jobs that
//! escape the slice or exceed QoS / blast radius are refused.

use crate::caps::{CPtr, CapError, CapKind, CapRights, CapTable, Capability};
use crate::types::{BankId, ChipletId, TileId};

/// Namespace-local partition identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PartitionId(pub u32);

/// Spatial slice: which chiplets / tiles / banks a partition may touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpatialSlice {
    pub chiplet_lo: ChipletId,
    pub chiplet_hi: ChipletId,
    pub tile_mask: u32,
    pub bank_mask: u32,
}

impl SpatialSlice {
    pub const fn single_chiplet(c: ChipletId, tile_mask: u32, banks: u32) -> Self {
        Self {
            chiplet_lo: c,
            chiplet_hi: c,
            tile_mask,
            bank_mask: banks,
        }
    }

    pub const fn contains_chiplet(self, c: ChipletId) -> bool {
        c.0 >= self.chiplet_lo.0 && c.0 <= self.chiplet_hi.0
    }

    pub const fn contains_tile(self, tile: TileId) -> bool {
        tile.0 < 32 && (self.tile_mask & (1u32 << tile.0 as u32)) != 0
    }

    pub const fn contains_bank(self, bank: BankId) -> bool {
        (bank.0 as u32) < 32 && (self.bank_mask & (1u32 << (bank.0 as u32))) != 0
    }
}

/// Bandwidth / credit QoS attached to a partition (software meters today).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QosBudget {
    pub bw_mbps: u32,
    pub credits: u32,
}

/// Blast-radius cap: how far a fault / flood may propagate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlastRadius {
    pub max_nodes: u16,
    pub max_hops: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionError {
    OutsideSlice,
    /// Soft HBM bandwidth over [`QosBudget::bw_mbps`] on the
    /// [`SoftHbmBwMeter`] / [`crate::window::TypedWindow`]
    /// ([`crate::window::WindowKind::Hbm`]) path. QoS credits use
    /// [`Self::CreditExhausted`] via [`crate::fence::Timeline::submit`] —
    /// do not resurrect `charge_credits`.
    QosExceeded,
    BlastRadius,
    Unbound,
    CreditExhausted,
    FenceNotReady,
}

/// Spatial + QoS + blast-radius object. Scheduler and accel bind to this.
#[derive(Clone, Copy, Debug)]
pub struct PartitionProfile {
    pub id: PartitionId,
    pub slice: SpatialSlice,
    pub qos: QosBudget,
    pub blast: BlastRadius,
}

impl PartitionProfile {
    pub const fn new(
        id: PartitionId,
        slice: SpatialSlice,
        qos: QosBudget,
        blast: BlastRadius,
    ) -> Self {
        Self {
            id,
            slice,
            qos,
            blast,
        }
    }

    pub fn mint(&self, table: &mut CapTable) -> Result<CPtr, CapError> {
        table.mint(Capability::new(
            CapKind::Partition,
            CapRights::PARTITION_FULL,
            self.id.0,
            table.owner(),
        ))
    }

    pub fn admit_chiplet(&self, c: ChipletId) -> Result<(), PartitionError> {
        if self.slice.contains_chiplet(c) {
            Ok(())
        } else {
            Err(PartitionError::OutsideSlice)
        }
    }

    pub fn admit_place(&self, tile: TileId, bank: Option<BankId>) -> Result<(), PartitionError> {
        if !self.slice.contains_tile(tile) {
            return Err(PartitionError::OutsideSlice);
        }
        if let Some(b) = bank {
            if !self.slice.contains_bank(b) {
                return Err(PartitionError::OutsideSlice);
            }
        }
        Ok(())
    }

    pub fn admit_hops(&self, hops: u8) -> Result<(), PartitionError> {
        if hops <= self.blast.max_hops {
            Ok(())
        } else {
            Err(PartitionError::BlastRadius)
        }
    }

    pub fn admit_nodes(&self, nodes: u16) -> Result<(), PartitionError> {
        if nodes <= self.blast.max_nodes {
            Ok(())
        } else {
            Err(PartitionError::BlastRadius)
        }
    }
}

/// Host red-team hops clip: two partitions / two slices.
/// In-budget hops admit; over [`BlastRadius::max_hops`] → [`PartitionError::BlastRadius`].
/// Not a `[blast]` serial line — sell needle is `[redteam] attack=blast-hops`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlastHopsReport {
    pub two_slice: bool,
    pub in_budget: bool,
    pub over_hops: bool,
}

impl BlastHopsReport {
    pub fn all_ok(&self) -> bool {
        self.two_slice && self.in_budget && self.over_hops
    }
}

/// Two tenants, distinct chiplet slices, `max_hops = 1`.
pub fn run_blast_hops_demo() -> BlastHopsReport {
    let a = PartitionProfile::new(
        PartitionId(1),
        SpatialSlice::single_chiplet(ChipletId(0), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );
    let b = PartitionProfile::new(
        PartitionId(2),
        SpatialSlice::single_chiplet(ChipletId(1), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );

    let two_slice = a.admit_chiplet(ChipletId(0)).is_ok()
        && b.admit_chiplet(ChipletId(1)).is_ok()
        && a.admit_chiplet(ChipletId(1)) == Err(PartitionError::OutsideSlice)
        && b.admit_chiplet(ChipletId(0)) == Err(PartitionError::OutsideSlice);

    let in_budget = a.admit_hops(0).is_ok() && a.admit_hops(1).is_ok() && b.admit_hops(1).is_ok();

    let over_hops = a.admit_hops(2) == Err(PartitionError::BlastRadius)
        && b.admit_hops(2) == Err(PartitionError::BlastRadius);

    BlastHopsReport {
        two_slice,
        in_budget,
        over_hops,
    }
}

/// Host red-team nodes clip: two partitions / two slices.
/// In-budget nodes admit; over [`BlastRadius::max_nodes`] → [`PartitionError::BlastRadius`].
/// Not a `[blast]` serial line — sell needle is `[redteam] attack=blast-nodes`.
/// Not a hops rehash — hops stays `attack=blast-hops`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlastNodesReport {
    pub two_slice: bool,
    pub in_budget: bool,
    pub over_nodes: bool,
}

impl BlastNodesReport {
    pub fn all_ok(&self) -> bool {
        self.two_slice && self.in_budget && self.over_nodes
    }
}

/// Two tenants, distinct chiplet slices, `max_nodes = 4`.
pub fn run_blast_nodes_demo() -> BlastNodesReport {
    let a = PartitionProfile::new(
        PartitionId(1),
        SpatialSlice::single_chiplet(ChipletId(0), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );
    let b = PartitionProfile::new(
        PartitionId(2),
        SpatialSlice::single_chiplet(ChipletId(1), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );

    let two_slice = a.admit_chiplet(ChipletId(0)).is_ok()
        && b.admit_chiplet(ChipletId(1)).is_ok()
        && a.admit_chiplet(ChipletId(1)) == Err(PartitionError::OutsideSlice)
        && b.admit_chiplet(ChipletId(0)) == Err(PartitionError::OutsideSlice);

    let in_budget =
        a.admit_nodes(0).is_ok() && a.admit_nodes(4).is_ok() && b.admit_nodes(4).is_ok();

    let over_nodes = a.admit_nodes(5) == Err(PartitionError::BlastRadius)
        && b.admit_nodes(5) == Err(PartitionError::BlastRadius);

    BlastNodesReport {
        two_slice,
        in_budget,
        over_nodes,
    }
}

/// Host red-team QoS credits clip: two partitions / two slices.
/// [`crate::fence::Timeline::submit`] meters [`QosBudget::credits`]:
/// in-budget submits admit; `in_flight >= credits` →
/// [`PartitionError::CreditExhausted`]. Complete / timeout frees a credit
/// and admit resumes. Sell needle is `[redteam] attack=qos-credits` —
/// existing fence meter, not a second charge API / EventRing theater.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QosCreditsReport {
    pub two_slice: bool,
    pub in_budget: bool,
    pub over_credits: bool,
    pub resume: bool,
}

impl QosCreditsReport {
    pub fn all_ok(&self) -> bool {
        self.two_slice && self.in_budget && self.over_credits && self.resume
    }
}

/// Two tenants, distinct chiplet slices, `credits = 4`.
/// Uses [`crate::fence::Timeline`] + [`PartitionProfile`] only.
pub fn run_qos_credits_demo() -> QosCreditsReport {
    use crate::fence::Timeline;

    let a = PartitionProfile::new(
        PartitionId(1),
        SpatialSlice::single_chiplet(ChipletId(0), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );
    let b = PartitionProfile::new(
        PartitionId(2),
        SpatialSlice::single_chiplet(ChipletId(1), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );

    let two_slice = a.admit_chiplet(ChipletId(0)).is_ok()
        && b.admit_chiplet(ChipletId(1)).is_ok()
        && a.admit_chiplet(ChipletId(1)) == Err(PartitionError::OutsideSlice)
        && b.admit_chiplet(ChipletId(0)) == Err(PartitionError::OutsideSlice);

    let mut ta = Timeline::new(PartitionId(1));
    let mut tb = Timeline::new(PartitionId(2));

    // Fill each timeline to qos.credits — all admit.
    let a0 = ta.submit(&a, None);
    let a1 = ta.submit(&a, None);
    let a2 = ta.submit(&a, None);
    let a3 = ta.submit(&a, None);
    let b0 = tb.submit(&b, None);
    let b1 = tb.submit(&b, None);
    let b2 = tb.submit(&b, None);
    let b3 = tb.submit(&b, None);
    let in_budget = a0.is_ok()
        && a1.is_ok()
        && a2.is_ok()
        && a3.is_ok()
        && b0.is_ok()
        && b1.is_ok()
        && b2.is_ok()
        && b3.is_ok()
        && ta.in_flight() == a.qos.credits
        && tb.in_flight() == b.qos.credits;

    // One over budget → CreditExhausted (existing Timeline meter).
    let over_credits = ta.submit(&a, None) == Err(PartitionError::CreditExhausted)
        && tb.submit(&b, None) == Err(PartitionError::CreditExhausted);

    // complete / timeout frees a credit; next submit admits again.
    let resume = match (a0, b0) {
        (Ok(af), Ok(bf)) => {
            ta.complete(af.id).is_ok()
                && ta.submit(&a, None).is_ok()
                && tb.timeout(bf.id).is_ok()
                && tb.submit(&b, None).is_ok()
        }
        _ => false,
    };

    QosCreditsReport {
        two_slice,
        in_budget,
        over_credits,
        resume,
    }
}

/// Host red-team outside-slice clip: two partitions / two chiplet slices.
/// Own chiplet admits via [`PartitionProfile::admit_chiplet`]; foreign
/// chiplet → [`PartitionError::OutsideSlice`]. Sell needle is
/// `[redteam] attack=outside-slice` — existing path only; **not** hops /
/// qos / CrossCut / bank-color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutsideSliceReport {
    pub two_slice: bool,
    pub in_slice: bool,
    pub outside: bool,
}

impl OutsideSliceReport {
    pub fn all_ok(&self) -> bool {
        self.two_slice && self.in_slice && self.outside
    }
}

/// Two tenants, distinct chiplet slices. Existing `admit_chiplet` only.
pub fn run_outside_slice_demo() -> OutsideSliceReport {
    let a = PartitionProfile::new(
        PartitionId(1),
        SpatialSlice::single_chiplet(ChipletId(0), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );
    let b = PartitionProfile::new(
        PartitionId(2),
        SpatialSlice::single_chiplet(ChipletId(1), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );

    let two_slice = a.slice.chiplet_lo == ChipletId(0)
        && a.slice.chiplet_hi == ChipletId(0)
        && b.slice.chiplet_lo == ChipletId(1)
        && b.slice.chiplet_hi == ChipletId(1)
        && a.slice.chiplet_lo != b.slice.chiplet_lo;

    let in_slice = a.admit_chiplet(ChipletId(0)).is_ok() && b.admit_chiplet(ChipletId(1)).is_ok();

    let outside = a.admit_chiplet(ChipletId(1)) == Err(PartitionError::OutsideSlice)
        && b.admit_chiplet(ChipletId(0)) == Err(PartitionError::OutsideSlice);

    OutsideSliceReport {
        two_slice,
        in_slice,
        outside,
    }
}

/// Software Soft HBM bandwidth meter against [`QosBudget::bw_mbps`].
///
/// Charges on the [`crate::window::TypedWindow`] / [`crate::window::WindowKind::Hbm`]
/// path only. Over budget → [`PartitionError::QosExceeded`]. Release frees
/// budget so admit resumes. Software meter only — not silicon BW, not FLOPs,
/// not `charge_credits` / CapTable / SoftNPU / BAR0 / CXL productization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftHbmBwMeter {
    partition: PartitionId,
    used_mbps: u32,
}

impl SoftHbmBwMeter {
    pub const fn new(partition: PartitionId) -> Self {
        Self {
            partition,
            used_mbps: 0,
        }
    }

    pub const fn used_mbps(self) -> u32 {
        self.used_mbps
    }

    /// Charge `mbps` against `profile.qos.bw_mbps` for an HBM typed window.
    ///
    /// Wrong partition → [`PartitionError::Unbound`]. Non-HBM window →
    /// [`PartitionError::Unbound`]. `used + mbps > bw_mbps` →
    /// [`PartitionError::QosExceeded`].
    pub fn charge(
        &mut self,
        profile: &PartitionProfile,
        win: &crate::window::TypedWindow,
        mbps: u32,
    ) -> Result<(), PartitionError> {
        if profile.id.0 != self.partition.0 {
            return Err(PartitionError::Unbound);
        }
        if win.kind != crate::window::WindowKind::Hbm {
            return Err(PartitionError::Unbound);
        }
        let next = self.used_mbps.saturating_add(mbps);
        if next > profile.qos.bw_mbps {
            return Err(PartitionError::QosExceeded);
        }
        self.used_mbps = next;
        Ok(())
    }

    /// Free previously charged HBM bandwidth.
    pub fn release(&mut self, mbps: u32) {
        self.used_mbps = self.used_mbps.saturating_sub(mbps);
    }
}

/// Host red-team Soft HBM BW clip: two partitions / two slices.
/// [`SoftHbmBwMeter::charge`] meters [`QosBudget::bw_mbps`] on HBM
/// [`crate::window::TypedWindow`]s: in-budget admits; over budget →
/// [`PartitionError::QosExceeded`]. Release frees budget and admit resumes.
/// Sell needle is `[redteam] attack=hbm-bw` — software meter only; not
/// silicon BW / FLOPs / charge_credits / CapTable / SoftNPU / BAR0 / CXL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HbmBwReport {
    pub two_slice: bool,
    pub in_budget: bool,
    pub over_bw: bool,
    pub resume: bool,
    pub non_hbm: bool,
}

impl HbmBwReport {
    pub fn all_ok(&self) -> bool {
        self.two_slice && self.in_budget && self.over_bw && self.resume && self.non_hbm
    }
}

/// Two tenants, distinct chiplet slices, `bw_mbps = 100`.
/// Uses [`SoftHbmBwMeter`] + HBM [`crate::window::TypedWindow`] only.
pub fn run_hbm_bw_demo() -> HbmBwReport {
    use crate::iommu::StreamId;
    use crate::types::{PhysAddr, TenantId};
    use crate::window::{TypedWindow, WindowKind};

    let a = PartitionProfile::new(
        PartitionId(1),
        SpatialSlice::single_chiplet(ChipletId(0), 0b1, 0b1),
        QosBudget {
            bw_mbps: 100,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );
    let b = PartitionProfile::new(
        PartitionId(2),
        SpatialSlice::single_chiplet(ChipletId(1), 0b1, 0b1),
        QosBudget {
            bw_mbps: 100,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    );

    let two_slice = a.admit_chiplet(ChipletId(0)).is_ok()
        && b.admit_chiplet(ChipletId(1)).is_ok()
        && a.admit_chiplet(ChipletId(1)) == Err(PartitionError::OutsideSlice)
        && b.admit_chiplet(ChipletId(0)) == Err(PartitionError::OutsideSlice);

    let sid_a = StreamId::accel(ChipletId(0), crate::types::TileId(0), 0);
    let sid_b = StreamId::accel(ChipletId(1), crate::types::TileId(0), 0);
    let hbm_a = TypedWindow::new(
        PhysAddr(0xB000),
        0x1000,
        WindowKind::Hbm,
        sid_a,
        TenantId(1),
    );
    let hbm_b = TypedWindow::new(
        PhysAddr(0xC000),
        0x1000,
        WindowKind::Hbm,
        sid_b,
        TenantId(2),
    );
    let dram_a = TypedWindow::new(
        PhysAddr(0xD000),
        0x1000,
        WindowKind::Dram,
        sid_a,
        TenantId(1),
    );

    let mut ma = SoftHbmBwMeter::new(PartitionId(1));
    let mut mb = SoftHbmBwMeter::new(PartitionId(2));

    // Fill each meter to qos.bw_mbps — all admit.
    let in_budget = ma.charge(&a, &hbm_a, 40).is_ok()
        && ma.charge(&a, &hbm_a, 60).is_ok()
        && mb.charge(&b, &hbm_b, 100).is_ok()
        && ma.used_mbps() == a.qos.bw_mbps
        && mb.used_mbps() == b.qos.bw_mbps;

    // One over budget → QosExceeded (soft HBM BW meter).
    let over_bw = ma.charge(&a, &hbm_a, 1) == Err(PartitionError::QosExceeded)
        && mb.charge(&b, &hbm_b, 1) == Err(PartitionError::QosExceeded);

    // Non-HBM typed window is unbound on this meter (HBM path only).
    let non_hbm = ma.charge(&a, &dram_a, 1) == Err(PartitionError::Unbound);

    // release frees budget; next charge admits again.
    ma.release(40);
    mb.release(50);
    let resume = ma.charge(&a, &hbm_a, 40).is_ok() && mb.charge(&b, &hbm_b, 50).is_ok();

    HbmBwReport {
        two_slice,
        in_budget,
        over_bw,
        resume,
        non_hbm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TenantId;

    #[test]
    fn spatial_first() {
        let p = PartitionProfile::new(
            PartitionId(1),
            SpatialSlice::single_chiplet(ChipletId(0), (1 << 0) | (1 << 2), 0b1),
            QosBudget {
                bw_mbps: 1000,
                credits: 4,
            },
            BlastRadius {
                max_nodes: 4,
                max_hops: 1,
            },
        );
        assert!(p.admit_chiplet(ChipletId(0)).is_ok());
        assert_eq!(
            p.admit_chiplet(ChipletId(1)),
            Err(PartitionError::OutsideSlice)
        );
        assert!(p.admit_place(TileId(0), Some(BankId(0))).is_ok());
        assert_eq!(
            p.admit_place(TileId(1), Some(BankId(0))),
            Err(PartitionError::OutsideSlice)
        );
        assert_eq!(p.admit_hops(2), Err(PartitionError::BlastRadius));
        assert_eq!(p.admit_nodes(5), Err(PartitionError::BlastRadius));
        let mut t = CapTable::new(TenantId(1));
        let c = p.mint(&mut t).unwrap();
        assert_eq!(t.lookup(c).unwrap().kind, CapKind::Partition);
    }

    #[test]
    fn blast_hops_demo_two_slice_refuse() {
        let r = run_blast_hops_demo();
        assert!(r.two_slice, "two tenants / two slices");
        assert!(r.in_budget, "in-budget hops admit");
        assert!(r.over_hops, "over max_hops → BlastRadius");
        assert!(r.all_ok());
    }

    #[test]
    fn blast_nodes_demo_two_slice_refuse() {
        let r = run_blast_nodes_demo();
        assert!(r.two_slice, "two tenants / two slices");
        assert!(r.in_budget, "in-budget nodes admit");
        assert!(r.over_nodes, "over max_nodes → BlastRadius");
        assert!(r.all_ok());
    }

    #[test]
    fn qos_credits_demo_in_budget_and_refuse() {
        let r = run_qos_credits_demo();
        assert!(r.two_slice, "two tenants / two slices");
        assert!(r.in_budget, "in-budget Timeline::submit admits");
        assert!(r.over_credits, "over credits → CreditExhausted");
        assert!(r.resume, "complete/timeout frees credit; admit resumes");
        assert!(r.all_ok());
    }

    #[test]
    fn outside_slice_demo_own_admit_foreign_refuse() {
        let r = run_outside_slice_demo();
        assert!(r.two_slice, "two tenants / two chiplet slices");
        assert!(r.in_slice, "own chiplet admit_chiplet admits");
        assert!(r.outside, "foreign chiplet → OutsideSlice");
        assert!(r.all_ok());
    }

    #[test]
    fn hbm_bw_demo_in_budget_and_refuse() {
        let r = run_hbm_bw_demo();
        assert!(r.two_slice, "two tenants / two slices");
        assert!(r.in_budget, "in-budget SoftHbmBwMeter::charge admits");
        assert!(r.over_bw, "over bw_mbps → QosExceeded");
        assert!(r.resume, "release frees budget; admit resumes");
        assert!(r.non_hbm, "non-HBM TypedWindow → Unbound on HBM meter");
        assert!(r.all_ok());
    }

    fn profile(slice: SpatialSlice, bw_mbps: u32) -> PartitionProfile {
        PartitionProfile::new(
            PartitionId(7),
            slice,
            QosBudget {
                bw_mbps,
                credits: 4,
            },
            BlastRadius {
                max_nodes: 4,
                max_hops: 1,
            },
        )
    }

    fn hbm_window() -> crate::window::TypedWindow {
        use crate::iommu::StreamId;
        use crate::types::PhysAddr;
        use crate::window::{TypedWindow, WindowKind};
        TypedWindow::new(
            PhysAddr(0xB000),
            0x1000,
            WindowKind::Hbm,
            StreamId::accel(ChipletId(0), TileId(0), 0),
            TenantId(1),
        )
    }

    #[test]
    fn slice_masks_cover_bit_31_and_refuse_ids_past_the_mask_width() {
        let s = SpatialSlice::single_chiplet(ChipletId(0), u32::MAX, u32::MAX);
        assert!(s.contains_tile(TileId(0)));
        assert!(s.contains_tile(TileId(31)));
        // TileId is u16 and BankId is u8: ids >= 32 have no mask bit and must
        // refuse even with a full mask (no shift wrap onto a low bit).
        for t in [32u16, 33, 63, 64, u16::MAX] {
            assert!(!s.contains_tile(TileId(t)), "tile {t}");
        }
        assert!(s.contains_bank(BankId(31)));
        for b in [32u8, 33, 64, u8::MAX] {
            assert!(!s.contains_bank(BankId(b)), "bank {b}");
        }
        let empty = SpatialSlice::single_chiplet(ChipletId(0), 0, 0);
        assert!(!empty.contains_tile(TileId(0)));
        assert!(!empty.contains_bank(BankId(0)));
    }

    #[test]
    fn chiplet_range_is_inclusive_and_an_inverted_range_admits_nothing() {
        let span = SpatialSlice {
            chiplet_lo: ChipletId(1),
            chiplet_hi: ChipletId(3),
            tile_mask: 0b1,
            bank_mask: 0b1,
        };
        assert!(!span.contains_chiplet(ChipletId(0)));
        for c in 1..=3u8 {
            assert!(span.contains_chiplet(ChipletId(c)), "chiplet {c}");
        }
        assert!(!span.contains_chiplet(ChipletId(4)));
        let p = profile(span, 100);
        assert!(p.admit_chiplet(ChipletId(3)).is_ok());
        assert_eq!(
            p.admit_chiplet(ChipletId(4)),
            Err(PartitionError::OutsideSlice)
        );

        let inverted = SpatialSlice {
            chiplet_lo: ChipletId(3),
            chiplet_hi: ChipletId(1),
            tile_mask: 0b1,
            bank_mask: 0b1,
        };
        for c in 0..=4u8 {
            assert!(!inverted.contains_chiplet(ChipletId(c)), "chiplet {c}");
        }
    }

    #[test]
    fn admit_place_checks_tile_before_bank_and_none_bank_skips_bank_check() {
        // Tiles 0 and 2, bank 1 only.
        let p = profile(SpatialSlice::single_chiplet(ChipletId(0), 0b101, 0b10), 100);
        assert!(p.admit_place(TileId(2), None).is_ok());
        assert!(p.admit_place(TileId(2), Some(BankId(1))).is_ok());
        assert_eq!(
            p.admit_place(TileId(2), Some(BankId(0))),
            Err(PartitionError::OutsideSlice)
        );
        // Foreign tile refuses regardless of bank (including a valid one, or none).
        assert_eq!(
            p.admit_place(TileId(1), None),
            Err(PartitionError::OutsideSlice)
        );
        assert_eq!(
            p.admit_place(TileId(1), Some(BankId(1))),
            Err(PartitionError::OutsideSlice)
        );
        // Boundaries are inclusive for hops/nodes.
        assert!(p.admit_hops(p.blast.max_hops).is_ok());
        assert!(p.admit_nodes(p.blast.max_nodes).is_ok());
        assert_eq!(p.admit_hops(u8::MAX), Err(PartitionError::BlastRadius));
        assert_eq!(p.admit_nodes(u16::MAX), Err(PartitionError::BlastRadius));
    }

    #[test]
    fn hbm_meter_refusals_leave_usage_unchanged() {
        let p = profile(SpatialSlice::single_chiplet(ChipletId(0), 0b1, 0b1), 100);
        let other = PartitionProfile::new(PartitionId(8), p.slice, p.qos, p.blast);
        let hbm = hbm_window();
        let mut m = SoftHbmBwMeter::new(p.id);
        m.charge(&p, &hbm, 30).unwrap();

        // Wrong partition profile → Unbound, nothing charged.
        assert_eq!(m.charge(&other, &hbm, 10), Err(PartitionError::Unbound));
        assert_eq!(m.used_mbps(), 30);
        // Over budget → QosExceeded, nothing charged (no partial fill).
        assert_eq!(m.charge(&p, &hbm, 71), Err(PartitionError::QosExceeded));
        assert_eq!(m.used_mbps(), 30);
        // u32::MAX saturates instead of wrapping below the budget.
        assert_eq!(
            m.charge(&p, &hbm, u32::MAX),
            Err(PartitionError::QosExceeded)
        );
        assert_eq!(m.used_mbps(), 30);
        // Exact fill admits; a zero charge at the full budget still admits.
        m.charge(&p, &hbm, 70).unwrap();
        assert_eq!(m.used_mbps(), p.qos.bw_mbps);
        assert!(m.charge(&p, &hbm, 0).is_ok());
        assert_eq!(m.charge(&p, &hbm, 1), Err(PartitionError::QosExceeded));
    }

    #[test]
    fn hbm_meter_release_saturates_at_zero_and_zero_budget_refuses() {
        let p = profile(SpatialSlice::single_chiplet(ChipletId(0), 0b1, 0b1), 100);
        let hbm = hbm_window();
        let mut m = SoftHbmBwMeter::new(p.id);
        m.charge(&p, &hbm, 20).unwrap();
        m.release(50);
        assert_eq!(m.used_mbps(), 0);
        m.release(u32::MAX);
        assert_eq!(m.used_mbps(), 0);
        // Over-release does not bank credit beyond bw_mbps.
        assert_eq!(m.charge(&p, &hbm, 101), Err(PartitionError::QosExceeded));
        assert!(m.charge(&p, &hbm, 100).is_ok());

        let zero = profile(p.slice, 0);
        let mut mz = SoftHbmBwMeter::new(zero.id);
        assert_eq!(mz.charge(&zero, &hbm, 1), Err(PartitionError::QosExceeded));
        assert!(mz.charge(&zero, &hbm, 0).is_ok());
        assert_eq!(mz.used_mbps(), 0);
    }

    #[test]
    fn report_all_ok_requires_every_flag() {
        assert!(!BlastHopsReport {
            two_slice: false,
            in_budget: true,
            over_hops: true
        }
        .all_ok());
        assert!(!BlastHopsReport {
            two_slice: true,
            in_budget: false,
            over_hops: true
        }
        .all_ok());
        assert!(!BlastHopsReport {
            two_slice: true,
            in_budget: true,
            over_hops: false
        }
        .all_ok());
        assert!(!BlastNodesReport {
            two_slice: false,
            in_budget: true,
            over_nodes: true
        }
        .all_ok());
        assert!(!BlastNodesReport {
            two_slice: true,
            in_budget: false,
            over_nodes: true
        }
        .all_ok());
        assert!(!BlastNodesReport {
            two_slice: true,
            in_budget: true,
            over_nodes: false
        }
        .all_ok());
        assert!(!OutsideSliceReport {
            two_slice: false,
            in_slice: true,
            outside: true
        }
        .all_ok());
        assert!(!OutsideSliceReport {
            two_slice: true,
            in_slice: false,
            outside: true
        }
        .all_ok());
        assert!(!OutsideSliceReport {
            two_slice: true,
            in_slice: true,
            outside: false
        }
        .all_ok());
        let q = QosCreditsReport {
            two_slice: true,
            in_budget: true,
            over_credits: true,
            resume: true,
        };
        assert!(q.all_ok());
        for k in 0..4 {
            let mut r = q;
            match k {
                0 => r.two_slice = false,
                1 => r.in_budget = false,
                2 => r.over_credits = false,
                _ => r.resume = false,
            }
            assert!(!r.all_ok(), "qos flag {k}");
        }
        let h = HbmBwReport {
            two_slice: true,
            in_budget: true,
            over_bw: true,
            resume: true,
            non_hbm: true,
        };
        assert!(h.all_ok());
        for k in 0..5 {
            let mut r = h;
            match k {
                0 => r.two_slice = false,
                1 => r.in_budget = false,
                2 => r.over_bw = false,
                3 => r.resume = false,
                _ => r.non_hbm = false,
            }
            assert!(!r.all_ok(), "hbm flag {k}");
        }
    }

    #[test]
    fn red_team_clips_are_deterministic() {
        assert_eq!(run_blast_hops_demo(), run_blast_hops_demo());
        assert_eq!(run_blast_nodes_demo(), run_blast_nodes_demo());
        assert_eq!(run_qos_credits_demo(), run_qos_credits_demo());
        assert_eq!(run_outside_slice_demo(), run_outside_slice_demo());
        assert_eq!(run_hbm_bw_demo(), run_hbm_bw_demo());
    }
}
