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

/// Host red-team bank-color clip: Compute foreign bank refused; Exchange still OK.
/// Sell needle is `[redteam] attack=bank-color` — existing [`admit_wave`] only.
/// Not a CrossCut / hops rehash and not a new isolator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BankColorReport {
    pub same_ok: bool,
    pub foreign_bank: bool,
    pub exchange_ok: bool,
}

impl BankColorReport {
    pub fn all_ok(&self) -> bool {
        self.same_ok && self.foreign_bank && self.exchange_ok
    }
}

/// Same-color Compute admits; foreign bank → [`ColorError::ForeignBank`];
/// [`Phase::Exchange`] still admits the foreign bank.
pub fn run_bank_color_demo() -> BankColorReport {
    let home = BankId(0);
    let same = BankColor::new(TenantId(1), home);
    let foreign = BankColor::new(TenantId(1), BankId(1));

    let same_ok = admit_wave(1, Phase::Compute, Some(same), home).is_ok();
    let foreign_bank =
        admit_wave(1, Phase::Compute, Some(foreign), home) == Err(ColorError::ForeignBank);
    let exchange_ok = admit_wave(1, Phase::Exchange, Some(foreign), home).is_ok();

    BankColorReport {
        same_ok,
        foreign_bank,
        exchange_ok,
    }
}

/// Host red-team uncolored-compute clip: Compute with no color refused;
/// Exchange with a color still OK. Sell needle is
/// `[redteam] attack=uncolored-compute` — existing [`admit_wave`] only.
/// Not a ForeignBank / bank-color rehash (that stays on `attack=bank-color`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UncoloredComputeReport {
    pub colored_ok: bool,
    pub uncolored: bool,
    pub exchange_ok: bool,
}

impl UncoloredComputeReport {
    pub fn all_ok(&self) -> bool {
        self.colored_ok && self.uncolored && self.exchange_ok
    }
}

/// Colored Compute admits; `color=None` Compute → [`ColorError::Uncolored`];
/// [`Phase::Exchange`] with a color still admits.
pub fn run_uncolored_compute_demo() -> UncoloredComputeReport {
    let home = BankId(0);
    let colored = BankColor::new(TenantId(1), home);

    let colored_ok = admit_wave(1, Phase::Compute, Some(colored), home).is_ok();
    let uncolored =
        admit_wave(1, Phase::Compute, None, home) == Err(ColorError::Uncolored);
    let exchange_ok = admit_wave(1, Phase::Exchange, Some(colored), home).is_ok();

    UncoloredComputeReport {
        colored_ok,
        uncolored,
        exchange_ok,
    }
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

    #[test]
    fn bank_color_demo_foreign_bank_refuse() {
        let r = run_bank_color_demo();
        assert!(r.same_ok, "same-color Compute admits");
        assert!(r.foreign_bank, "foreign bank Compute → ForeignBank");
        assert!(r.exchange_ok, "Exchange still admits foreign bank");
        assert!(r.all_ok());
    }

    #[test]
    fn uncolored_compute_demo_refuse() {
        let r = run_uncolored_compute_demo();
        assert!(r.colored_ok, "colored Compute admits");
        assert!(r.uncolored, "uncolored Compute → Uncolored");
        assert!(r.exchange_ok, "Exchange with color still admits");
        assert!(r.all_ok());
    }
}
