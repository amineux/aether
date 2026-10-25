//! SYS_MAP physical-address source check.

use aether_core::sysnr::map_pin_addr;

/// Bound: `arena_base` and `vaddr` range over all of `u64`.
///
/// Property: `map_pin_addr` returns the pinned physical address **only** from
/// the arena capability base. It returns `Some(base)` exactly when
/// `vaddr == 0` or `vaddr == base`, and otherwise refuses with `None`; it
/// never returns a caller-supplied address different from the arena base.
#[kani::proof]
fn map_pin_addr_only_from_arena_cap() {
    let base: u64 = kani::any();
    let vaddr: u64 = kani::any();
    match map_pin_addr(base, vaddr) {
        Some(pa) => {
            assert!(pa == base);
            assert!(vaddr == 0 || vaddr == base);
        }
        None => {
            assert!(vaddr != 0 && vaddr != base);
        }
    }
}
