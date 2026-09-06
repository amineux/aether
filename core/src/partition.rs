//! Partition profile: spatial slice + QoS + blast-radius isolation.
//!
//! Isolation is spatial (slices/columns) first, temporal second.
//! The scheduler and every accel activity bind to a partition; jobs that
//! escape the slice or exceed QoS / blast radius are refused.

use crate::caps::{CapError, CapKind, CapRights, CapTable, Capability, CPtr};
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
        table.mint(Capability {
            kind: CapKind::Partition,
            rights: CapRights::PARTITION_FULL,
            object: self.id.0,
            badge: 0,
            generation: 0,
            tenant: table.owner(),
        })
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
        let mut t = CapTable::new(TenantId(1));
        let c = p.mint(&mut t).unwrap();
        assert_eq!(t.lookup(c).unwrap().kind, CapKind::Partition);
    }
}
