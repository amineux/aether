//! Tenant / bank coloring for tensor arenas and accel waves.
//!
//! An arena is painted with a [`BankColor`] at alloc (or after an explicit
//! ownership transfer). A Compute wave whose arena color is foreign to the
//! destination tile is refused. [`Phase::Exchange`] is the explicit
//! transfer path that may touch a foreign bank.

use crate::arena::Arena;
use crate::phase::Phase;
use crate::types::{BankId, TenantId};

/// Tenant + bank paint on an arena (and on the wave that names it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BankColor {
    pub tenant: TenantId,
    pub bank: BankId,
}

impl BankColor {
    pub const fn new(tenant: TenantId, bank: BankId) -> Self {
        Self { tenant, bank }
    }

    pub const fn unassigned(bank: BankId) -> Self {
        Self {
            tenant: TenantId(0),
            bank,
        }
    }

    pub const fn is_assigned(self) -> bool {
        self.tenant.0 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorError {
    /// Arena bank is not the tile's home bank, and this is not Exchange.
    ForeignBank,
    /// Arena tenant color does not match the submitting wave.
    ForeignTenant,
    /// Arena has no owner / color; Compute needs an explicit transfer first.
    Uncolored,
}

/// Admit a wave against an arena color and the destination tile's home bank.
///
/// - [`Phase::Exchange`] always passes (explicit ownership / DMA xfer).
/// - Compute requires the arena to be colored for `job_tenant` and the
///   arena bank to match `tile_home`.
pub fn admit_wave(
    job_tenant: u32,
    phase: Phase,
    color: Option<BankColor>,
    tile_home: BankId,
) -> Result<(), ColorError> {
    if phase == Phase::Exchange {
        return Ok(());
    }
    let Some(color) = color else {
        return Err(ColorError::Uncolored);
    };
    if !color.is_assigned() {
        return Err(ColorError::Uncolored);
    }
    if color.tenant.0 != job_tenant {
        return Err(ColorError::ForeignTenant);
    }
    if color.bank != tile_home {
        return Err(ColorError::ForeignBank);
    }
    Ok(())
}

/// Same gate using an [`Arena`] that already carries a color.
pub fn admit_arena_wave(
    job_tenant: u32,
    phase: Phase,
    arena: &Arena,
    tile_home: BankId,
) -> Result<(), ColorError> {
    admit_wave(job_tenant, phase, Some(arena.color), tile_home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_same_color_ok() {
        let c = BankColor::new(TenantId(1), BankId(0));
        assert!(admit_wave(1, Phase::Compute, Some(c), BankId(0)).is_ok());
    }

    #[test]
    fn compute_foreign_bank_refused() {
        let c = BankColor::new(TenantId(1), BankId(1));
        assert_eq!(
            admit_wave(1, Phase::Compute, Some(c), BankId(0)).unwrap_err(),
            ColorError::ForeignBank
        );
    }

    #[test]
    fn compute_foreign_tenant_refused() {
        let c = BankColor::new(TenantId(2), BankId(0));
        assert_eq!(
            admit_wave(1, Phase::Compute, Some(c), BankId(0)).unwrap_err(),
            ColorError::ForeignTenant
        );
    }

    #[test]
    fn exchange_allows_foreign_bank() {
        let c = BankColor::new(TenantId(1), BankId(1));
        assert!(admit_wave(1, Phase::Exchange, Some(c), BankId(0)).is_ok());
    }

    #[test]
    fn uncolored_compute_refused() {
        assert_eq!(
            admit_wave(1, Phase::Compute, None, BankId(0)).unwrap_err(),
            ColorError::Uncolored
        );
        assert_eq!(
            admit_wave(
                1,
                Phase::Compute,
                Some(BankColor::unassigned(BankId(0))),
                BankId(0)
            )
            .unwrap_err(),
            ColorError::Uncolored
        );
    }
}
