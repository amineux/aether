//! NUMA / bank-aware tensor arenas.
//!
//! Allocations are *contiguous* (huge-page friendly), *pinned* (no swap — we
//! have no swap), and carry explicit ownership. The allocator never implies
//! cache coherence: a transfer from CPU tile to NPU tile is an ownership
//! handoff, not a shared mapping.

use crate::color::BankColor;
use crate::space::MemorySpace;
use crate::types::{BankId, TenantId, PAGE_2M, PAGE_4K, PhysAddr};

pub const MAX_ARENAS: usize = 16;
pub const MAX_FREE: usize = 24;
pub const MAX_BANKS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArenaError {
    NoSpace,
    BadAlign,
    BadSize,
    UnknownBank,
    UnknownArena,
    NotOwner,
    ArenaLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaRequest {
    pub size: u64,
    pub align: u64,
    pub bank_pref: Option<BankId>,
    pub pinned: bool,
    pub dma: bool,
    pub huge: bool,
    /// Typed place this buffer is bound to. UNIFIED_MEMORY is a cap bit, not a space.
    pub space: MemorySpace,
    /// Tenant / bank paint. `None` = uncolored until an explicit transfer.
    pub color: Option<BankColor>,
}

impl ArenaRequest {
    pub fn tensor(size: u64, bank: Option<BankId>) -> Self {
        Self {
            size,
            align: PAGE_4K,
            bank_pref: bank,
            pinned: true,
            dma: true,
            huge: size >= PAGE_2M,
            space: MemorySpace::Host,
            color: None,
        }
    }

    pub const fn in_space(mut self, space: MemorySpace) -> Self {
        self.space = space;
        self
    }

    /// Paint the allocation with a tenant/bank color at birth.
    pub const fn with_color(mut self, color: BankColor) -> Self {
        self.color = Some(color);
        self
    }

    pub const fn for_tenant(mut self, tenant: TenantId) -> Self {
        if let Some(bank) = self.bank_pref {
            self.color = Some(BankColor::new(tenant, bank));
        } else {
            self.color = Some(BankColor::new(tenant, BankId(0)));
        }
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arena {
    pub id: ArenaId,
    pub base: PhysAddr,
    pub size: u64,
    pub bank: BankId,
    pub pinned: bool,
    pub dma: bool,
    pub huge: bool,
    /// Owning tile. `None` means kernel / unassigned.
    pub owner_tile: Option<u16>,
    pub owner_tenant: Option<u32>,
    pub space: MemorySpace,
    /// Tenant + bank paint. Updated by [`ArenaAllocator::transfer_owner`].
    pub color: BankColor,
}

#[derive(Clone, Copy, Debug)]
struct FreeSpan {
    base: u64,
    size: u64,
    bank: BankId,
}

#[derive(Clone, Debug)]
pub struct ArenaAllocator {
    free: [Option<FreeSpan>; MAX_FREE],
    arenas: [Option<Arena>; MAX_ARENAS],
    next_id: u32,
    banks: u8,
}

impl ArenaAllocator {
    /// `banks` is a list of (bank, base, size) physical windows.
    pub fn new(banks: &[(BankId, PhysAddr, u64)]) -> Result<Self, ArenaError> {
        if banks.is_empty() || banks.len() > MAX_BANKS {
            return Err(ArenaError::UnknownBank);
        }
        let mut free = [None; MAX_FREE];
        for (i, &(bank, base, size)) in banks.iter().enumerate() {
            if size == 0 {
                return Err(ArenaError::BadSize);
            }
            free[i] = Some(FreeSpan {
                base: base.0,
                size,
                bank,
            });
        }
        Ok(Self {
            free,
            arenas: [None; MAX_ARENAS],
            next_id: 1,
            banks: banks.len() as u8,
        })
    }

    pub fn bank_count(&self) -> u8 {
        self.banks
    }

    fn align_up(addr: u64, align: u64) -> Option<u64> {
        if align == 0 || !align.is_power_of_two() {
            return None;
        }
        addr.checked_add(align - 1).map(|x| x & !(align - 1))
    }

    fn alloc_slot(&mut self) -> Result<usize, ArenaError> {
        self.arenas
            .iter()
            .position(|a| a.is_none())
            .ok_or(ArenaError::ArenaLimit)
    }

    /// First-fit in the preferred bank, then any bank. Splits the free span.
    pub fn alloc(&mut self, req: ArenaRequest) -> Result<Arena, ArenaError> {
        if req.size == 0 {
            return Err(ArenaError::BadSize);
        }
        let align = if req.huge {
            core::cmp::max(req.align, PAGE_2M)
        } else {
            core::cmp::max(req.align, PAGE_4K)
        };
        if !align.is_power_of_two() {
            return Err(ArenaError::BadAlign);
        }
        let size = Self::align_up(req.size, align).ok_or(ArenaError::BadSize)?;

        let order: [Option<BankId>; MAX_BANKS] = {
            let mut o = [None; MAX_BANKS];
            let mut n = 0;
            if let Some(pref) = req.bank_pref {
                o[n] = Some(pref);
                n += 1;
            }
            // probe remaining banks in index order via free list
            for span in self.free.iter().flatten() {
                if o.iter().any(|b| *b == Some(span.bank)) {
                    continue;
                }
                if n < MAX_BANKS {
                    o[n] = Some(span.bank);
                    n += 1;
                }
            }
            o
        };

        for pref in order.iter().flatten() {
            if let Some(arena) = self.try_alloc_in_bank(*pref, size, align, req) {
                return Ok(arena);
            }
        }
        Err(ArenaError::NoSpace)
    }

    fn try_alloc_in_bank(
        &mut self,
        bank: BankId,
        size: u64,
        align: u64,
        req: ArenaRequest,
    ) -> Option<Arena> {
        let mut found: Option<(usize, u64, u64, u64)> = None; // idx, aligned, prefix, span_size
        for (i, span) in self.free.iter().enumerate() {
            let Some(span) = span else { continue };
            if span.bank != bank {
                continue;
            }
            let aligned = Self::align_up(span.base, align)?;
            if aligned < span.base {
                continue;
            }
            let prefix = aligned - span.base;
            if prefix.checked_add(size)? <= span.size {
                found = Some((i, aligned, prefix, span.size));
                break;
            }
        }
        let (i, aligned, prefix, span_size) = found?;
        let span = self.free[i].take().unwrap();
        // leftover before
        if prefix > 0 {
            self.insert_free(FreeSpan {
                base: span.base,
                size: prefix,
                bank: span.bank,
            });
        }
        let end = aligned + size;
        let span_end = span.base + span_size;
        if end < span_end {
            self.insert_free(FreeSpan {
                base: end,
                size: span_end - end,
                bank: span.bank,
            });
        }
        let slot = self.alloc_slot().ok()?;
        let arena = Arena {
            id: ArenaId(self.next_id),
            base: PhysAddr(aligned),
            size,
            bank,
            pinned: req.pinned,
            dma: req.dma,
            huge: req.huge && align >= PAGE_2M,
            owner_tile: None,
            owner_tenant: req.color.map(|c| c.tenant.0),
            space: req.space,
            color: req
                .color
                .map(|c| BankColor::new(c.tenant, bank))
                .unwrap_or(BankColor::unassigned(bank)),
        };
        self.next_id += 1;
        self.arenas[slot] = Some(arena);
        Some(arena)
    }

    fn insert_free(&mut self, span: FreeSpan) {
        // coalesce with neighbours in the same bank
        let mut base = span.base;
        let mut size = span.size;
        for slot in self.free.iter_mut() {
            if let Some(s) = slot {
                if s.bank != span.bank {
                    continue;
                }
                if s.base + s.size == base {
                    base = s.base;
                    size += s.size;
                    *slot = None;
                } else if base + size == s.base {
                    size += s.size;
                    *slot = None;
                }
            }
        }
        if let Some(empty) = self.free.iter_mut().find(|s| s.is_none()) {
            *empty = Some(FreeSpan {
                base,
                size,
                bank: span.bank,
            });
        }
        // if no free slot, the span is leaked — prototype limit (MAX_FREE)
    }

    pub fn get(&self, id: ArenaId) -> Result<&Arena, ArenaError> {
        self.arenas
            .iter()
            .flatten()
            .find(|a| a.id == id)
            .ok_or(ArenaError::UnknownArena)
    }

    pub fn get_mut(&mut self, id: ArenaId) -> Result<&mut Arena, ArenaError> {
        self.arenas
            .iter_mut()
            .flatten()
            .find(|a| a.id == id)
            .ok_or(ArenaError::UnknownArena)
    }

    /// Explicit ownership transfer. No implicit coherence: the previous owner
    /// must not touch the range after this returns.
    pub fn transfer_owner(
        &mut self,
        id: ArenaId,
        from_tile: Option<u16>,
        to_tile: u16,
        tenant: u32,
    ) -> Result<(), ArenaError> {
        let a = self.get_mut(id)?;
        if a.owner_tile != from_tile {
            return Err(ArenaError::NotOwner);
        }
        a.owner_tile = Some(to_tile);
        a.owner_tenant = Some(tenant);
        a.color = BankColor::new(TenantId(tenant), a.bank);
        Ok(())
    }

    pub fn free(&mut self, id: ArenaId) -> Result<(), ArenaError> {
        let pos = self
            .arenas
            .iter()
            .position(|a| a.as_ref().map(|x| x.id) == Some(id))
            .ok_or(ArenaError::UnknownArena)?;
        let a = self.arenas[pos].take().unwrap();
        self.insert_free(FreeSpan {
            base: a.base.0,
            size: a.size,
            bank: a.bank,
        });
        Ok(())
    }

    pub fn free_bytes(&self, bank: BankId) -> u64 {
        self.free
            .iter()
            .flatten()
            .filter(|s| s.bank == bank)
            .map(|s| s.size)
            .sum()
    }
}

/// Host red-team report for arena ownership-handoff refuse.
///
/// Sell line `[redteam] attack=arena-not-owner` — existing
/// [`ArenaAllocator::transfer_owner`] only. Handoff is an explicit ownership
/// transfer, not a shared mapping: a tile that does not own the arena, a
/// "kernel / unassigned" claim on an already-owned arena, and the previous
/// owner after a handoff are all refused as [`ArenaError::NotOwner`], and the
/// owner tile, owner tenant, and bank color stay unchanged. A handoff of a
/// freed arena id is [`ArenaError::UnknownArena`]. `free` stays a
/// kernel-trust primitive and is not claimed here. **Not** bank-color
/// (`ForeignBank`) / foreign-tenant-color / uncolored-compute; no new
/// opcodes; software path only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaNotOwnerReport {
    /// Control: kernel → tile 2 → tile 3 handoffs admit (explicit chain).
    pub handoff_ok: bool,
    /// Non-owner tile naming itself as `from` → `NotOwner`; state unchanged.
    pub foreign_tile_refused: bool,
    /// `from = None` (kernel / unassigned) on an owned arena → `NotOwner`.
    pub reclaim_refused: bool,
    /// Previous owner after handoff → `NotOwner`; new owner keeps the range.
    pub stale_owner_refused: bool,
    /// Handoff of a freed arena id → `UnknownArena`.
    pub freed_refused: bool,
}

impl ArenaNotOwnerReport {
    pub fn all_ok(&self) -> bool {
        self.handoff_ok
            && self.foreign_tile_refused
            && self.reclaim_refused
            && self.stale_owner_refused
            && self.freed_refused
    }
}

/// Non-owner / stale-owner arena handoff → [`ArenaError::NotOwner`].
pub fn run_arena_not_owner_demo() -> ArenaNotOwnerReport {
    const A: u32 = 1;
    const B: u32 = 2;
    let fail = ArenaNotOwnerReport {
        handoff_ok: false,
        foreign_tile_refused: false,
        reclaim_refused: false,
        stale_owner_refused: false,
        freed_refused: false,
    };
    let Ok(mut arenas) = ArenaAllocator::new(&[(BankId(0), PhysAddr(0x0100_0000), 8 * PAGE_2M)])
    else {
        return fail;
    };
    let Ok(arena) = arenas.alloc(ArenaRequest::tensor(PAGE_4K, Some(BankId(0))).for_tenant(TenantId(A)))
    else {
        return fail;
    };
    let id = arena.id;
    let state = |al: &ArenaAllocator| {
        al.get(id)
            .map(|x| (x.owner_tile, x.owner_tenant, x.color.tenant))
            .ok()
    };
    let owned_by = |tile: u16| Some((Some(tile), Some(A), TenantId(A)));

    let to_two = arenas.transfer_owner(id, None, 2, A).is_ok() && state(&arenas) == owned_by(2);

    let foreign_tile_refused = arenas.transfer_owner(id, Some(5), 5, B) == Err(ArenaError::NotOwner)
        && arenas.transfer_owner(id, Some(3), 5, B) == Err(ArenaError::NotOwner)
        && state(&arenas) == owned_by(2);

    let reclaim_refused = arenas.transfer_owner(id, None, 5, B) == Err(ArenaError::NotOwner)
        && state(&arenas) == owned_by(2);

    let to_three = arenas.transfer_owner(id, Some(2), 3, A).is_ok() && state(&arenas) == owned_by(3);
    let stale_owner_refused = arenas.transfer_owner(id, Some(2), 2, A) == Err(ArenaError::NotOwner)
        && state(&arenas) == owned_by(3);

    let freed_refused = arenas.free(id).is_ok()
        && arenas.transfer_owner(id, Some(3), 5, B) == Err(ArenaError::UnknownArena)
        && arenas.get(id) == Err(ArenaError::UnknownArena);

    ArenaNotOwnerReport {
        handoff_ok: to_two && to_three,
        foreign_tile_refused,
        reclaim_refused,
        stale_owner_refused,
        freed_refused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_not_owner_demo_all_ok() {
        let r = run_arena_not_owner_demo();
        assert!(r.handoff_ok, "kernel → tile 2 → tile 3 handoffs admit: {r:?}");
        assert!(r.foreign_tile_refused, "non-owner tile handoff → NotOwner");
        assert!(r.reclaim_refused, "from=None on owned arena → NotOwner");
        assert!(r.stale_owner_refused, "previous owner after handoff → NotOwner");
        assert!(r.freed_refused, "freed arena id → UnknownArena");
        assert!(r.all_ok());
    }

    fn mk() -> ArenaAllocator {
        ArenaAllocator::new(&[
            (BankId(0), PhysAddr(0x0100_0000), 8 * 1024 * 1024),
            (BankId(1), PhysAddr(0x0900_0000), 8 * 1024 * 1024),
        ])
        .unwrap()
    }

    #[test]
    fn alloc_prefers_requested_bank() {
        let mut a = mk();
        let ar = a
            .alloc(ArenaRequest::tensor(64 * 1024, Some(BankId(1))))
            .unwrap();
        assert_eq!(ar.bank, BankId(1));
        assert_eq!(ar.base.0, 0x0900_0000);
        assert!(ar.pinned && ar.dma);
        assert_eq!(ar.size, 64 * 1024);
    }

    #[test]
    fn fallback_to_other_bank() {
        let mut a = mk();
        // exhaust bank 0
        a.alloc(ArenaRequest {
            size: 8 * 1024 * 1024,
            align: PAGE_4K,
            bank_pref: Some(BankId(0)),
            pinned: true,
            dma: true,
            huge: false,
            space: MemorySpace::Host,
            color: None,
        })
        .unwrap();
        let ar = a
            .alloc(ArenaRequest::tensor(4096, Some(BankId(0))))
            .unwrap();
        assert_eq!(ar.bank, BankId(1));
    }

    #[test]
    fn huge_page_alignment() {
        let mut a = mk();
        let ar = a
            .alloc(ArenaRequest {
                size: PAGE_2M,
                align: PAGE_4K,
                bank_pref: Some(BankId(0)),
                pinned: true,
                dma: true,
            huge: true,
            space: MemorySpace::Host,
            color: None,
        })
            .unwrap();
        assert!(ar.base.is_aligned(PAGE_2M));
        assert!(ar.huge);
        assert_eq!(ar.size, PAGE_2M);
    }

    #[test]
    fn ownership_transfer() {
        let mut a = mk();
        let ar = a.alloc(ArenaRequest::tensor(4096, Some(BankId(0)))).unwrap();
        a.transfer_owner(ar.id, None, 3, 1).unwrap();
        assert_eq!(a.get(ar.id).unwrap().owner_tile, Some(3));
        assert_eq!(
            a.transfer_owner(ar.id, Some(9), 4, 1).unwrap_err(),
            ArenaError::NotOwner
        );
        a.transfer_owner(ar.id, Some(3), 4, 1).unwrap();
        assert_eq!(a.get(ar.id).unwrap().owner_tile, Some(4));
        assert_eq!(
            a.get(ar.id).unwrap().color,
            crate::color::BankColor::new(crate::types::TenantId(1), BankId(0))
        );
    }

    #[test]
    fn alloc_takes_tenant_color() {
        let mut a = mk();
        let ar = a
            .alloc(
                ArenaRequest::tensor(4096, Some(BankId(1)))
                    .for_tenant(crate::types::TenantId(3)),
            )
            .unwrap();
        assert_eq!(ar.bank, BankId(1));
        assert_eq!(ar.color.tenant.0, 3);
        assert_eq!(ar.color.bank, BankId(1));
        assert_eq!(ar.owner_tenant, Some(3));
    }

    #[test]
    fn free_coalesces() {
        let mut a = mk();
        let x = a.alloc(ArenaRequest::tensor(4096, Some(BankId(0)))).unwrap();
        let y = a.alloc(ArenaRequest::tensor(4096, Some(BankId(0)))).unwrap();
        a.free(x.id).unwrap();
        a.free(y.id).unwrap();
        assert_eq!(a.free_bytes(BankId(0)), 8 * 1024 * 1024);
    }

    #[test]
    fn no_space() {
        let mut a = mk();
        a.alloc(ArenaRequest {
            size: 8 * 1024 * 1024,
            align: PAGE_4K,
            bank_pref: Some(BankId(0)),
            pinned: true,
            dma: false,
            huge: false,
            space: MemorySpace::Host,
            color: None,
        })
        .unwrap();
        a.alloc(ArenaRequest {
            size: 8 * 1024 * 1024,
            align: PAGE_4K,
            bank_pref: Some(BankId(1)),
            pinned: true,
            dma: false,
            huge: false,
            space: MemorySpace::Host,
            color: None,
        })
        .unwrap();
        assert_eq!(
            a.alloc(ArenaRequest::tensor(4096, None)).unwrap_err(),
            ArenaError::NoSpace
        );
    }

    #[test]
    fn bad_size() {
        let mut a = mk();
        assert_eq!(
            a.alloc(ArenaRequest::tensor(0, None)).unwrap_err(),
            ArenaError::BadSize
        );
    }
}
