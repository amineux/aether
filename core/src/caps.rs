//! Unforgeable capabilities for memory, endpoints, and accelerator queues.
//!
//! Userspace (and built-in tasks) never hold kernel object pointers. They hold
//! a `CPtr` — an index into their own cap table. The kernel looks up the slot,
//! checks generation + rights + tenant, then touches the object.
//!
//! This is the seL4-inspired invariant Aether bets the fabric on:
//! **no communication or DMA exists outside a capability**.

use crate::types::TenantId;

pub const CAP_SLOTS: usize = 32;

/// Handle a task presents to the kernel. Not an object id; not forgeable
/// across cap spaces (a slot number in tenant A's table names a different
/// object than the same number in tenant B's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CPtr(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CapKind {
    Empty = 0,
    Memory = 1,
    Endpoint = 2,
    AccelQueue = 3,
    Notification = 4,
}

impl CapKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Empty),
            1 => Some(Self::Memory),
            2 => Some(Self::Endpoint),
            3 => Some(Self::AccelQueue),
            4 => Some(Self::Notification),
            _ => None,
        }
    }
}

/// Rights bits. Derivation may only clear bits, never set them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapRights(pub u16);

impl CapRights {
    pub const READ: u16 = 1 << 0;
    pub const WRITE: u16 = 1 << 1;
    pub const GRANT: u16 = 1 << 2;
    pub const MAP: u16 = 1 << 3;
    pub const SUBMIT: u16 = 1 << 4;
    pub const WAIT: u16 = 1 << 5;
    pub const EXECUTE: u16 = 1 << 6;

    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(0x7F);
    pub const MEM_FULL: Self = Self(Self::READ | Self::WRITE | Self::GRANT | Self::MAP);
    pub const EP_FULL: Self = Self(Self::READ | Self::WRITE | Self::GRANT);
    pub const ACCEL_FULL: Self = Self(Self::SUBMIT | Self::WAIT | Self::GRANT | Self::READ);

    pub const fn contains(self, bits: u16) -> bool {
        self.0 & bits == bits
    }

    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Derivation is monotonic: `new` must be a subset of `self`.
    pub const fn can_derive(self, new: Self) -> bool {
        new.0 & self.0 == new.0
    }
}

/// Kernel-internal capability. Stored only inside a `CapTable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    pub kind: CapKind,
    pub rights: CapRights,
    pub object: u32,
    pub badge: u64,
    pub generation: u32,
    pub tenant: TenantId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapError {
    InvalidCptr,
    EmptySlot,
    WrongKind,
    InsufficientRights,
    TableFull,
    GenerationMismatch,
    CrossTenant,
    WouldEscalate,
}

#[derive(Clone, Debug)]
pub struct CapTable {
    slots: [Option<Capability>; CAP_SLOTS],
    owner: TenantId,
    mint_gen: u32,
}

impl CapTable {
    pub const fn new(owner: TenantId) -> Self {
        Self {
            slots: [None; CAP_SLOTS],
            owner,
            mint_gen: 1,
        }
    }

    pub const fn owner(&self) -> TenantId {
        self.owner
    }

    pub fn occupied(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    fn alloc_slot(&mut self) -> Result<usize, CapError> {
        self.slots
            .iter()
            .position(|s| s.is_none())
            .ok_or(CapError::TableFull)
    }

    /// Kernel-only: mint a new cap into this table. Callers must already
    /// have authority over `object` (typically the creating syscall).
    pub fn mint(&mut self, mut cap: Capability) -> Result<CPtr, CapError> {
        if cap.tenant != self.owner {
            return Err(CapError::CrossTenant);
        }
        if cap.kind == CapKind::Empty {
            return Err(CapError::WrongKind);
        }
        let slot = self.alloc_slot()?;
        cap.generation = self.mint_gen;
        self.mint_gen = self.mint_gen.wrapping_add(1);
        if self.mint_gen == 0 {
            self.mint_gen = 1;
        }
        self.slots[slot] = Some(cap);
        Ok(CPtr(slot as u16))
    }

    pub fn lookup(&self, cptr: CPtr) -> Result<&Capability, CapError> {
        let i = cptr.0 as usize;
        if i >= CAP_SLOTS {
            return Err(CapError::InvalidCptr);
        }
        self.slots[i].as_ref().ok_or(CapError::EmptySlot)
    }

    pub fn lookup_mut(&mut self, cptr: CPtr) -> Result<&mut Capability, CapError> {
        let i = cptr.0 as usize;
        if i >= CAP_SLOTS {
            return Err(CapError::InvalidCptr);
        }
        self.slots[i].as_mut().ok_or(CapError::EmptySlot)
    }

    pub fn require(
        &self,
        cptr: CPtr,
        kind: CapKind,
        need: u16,
    ) -> Result<&Capability, CapError> {
        let cap = self.lookup(cptr)?;
        if cap.kind != kind {
            return Err(CapError::WrongKind);
        }
        if !cap.rights.contains(need) {
            return Err(CapError::InsufficientRights);
        }
        if cap.tenant != self.owner {
            return Err(CapError::CrossTenant);
        }
        Ok(cap)
    }

    /// Delete a slot and bump so a stale CPtr cannot be reused on the same index.
    pub fn revoke(&mut self, cptr: CPtr) -> Result<Capability, CapError> {
        let i = cptr.0 as usize;
        if i >= CAP_SLOTS {
            return Err(CapError::InvalidCptr);
        }
        self.slots[i].take().ok_or(CapError::EmptySlot)
    }

    /// Move (GRANT) or copy-with-subset (derive) a cap into `dest`.
    pub fn transfer(
        &mut self,
        src: CPtr,
        dest: &mut CapTable,
        new_rights: CapRights,
        r#move: bool,
    ) -> Result<CPtr, CapError> {
        let cap = *self.require(src, self.lookup(src)?.kind, CapRights::GRANT)?;
        if !cap.rights.can_derive(new_rights) {
            return Err(CapError::WouldEscalate);
        }
        let mut minted = cap;
        minted.rights = new_rights;
        minted.tenant = dest.owner;
        let cptr = dest.mint(minted)?;
        if r#move {
            let _ = self.revoke(src);
        }
        Ok(cptr)
    }

    /// Derive a weaker cap in the *same* table.
    pub fn derive(&mut self, src: CPtr, new_rights: CapRights) -> Result<CPtr, CapError> {
        let cap = *self.lookup(src)?;
        if !cap.rights.can_derive(new_rights) {
            return Err(CapError::WouldEscalate);
        }
        if !cap.rights.contains(CapRights::GRANT) {
            return Err(CapError::InsufficientRights);
        }
        let mut child = cap;
        child.rights = new_rights;
        self.mint(child)
    }

    /// Isolation helper: does this table hold any cap to `object` of `kind`?
    pub fn holds(&self, kind: CapKind, object: u32) -> bool {
        self.slots.iter().flatten().any(|c| c.kind == kind && c.object == object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_cap(obj: u32, tenant: TenantId) -> Capability {
        Capability {
            kind: CapKind::Memory,
            rights: CapRights::MEM_FULL,
            object: obj,
            badge: 0,
            generation: 0,
            tenant,
        }
    }

    #[test]
    fn mint_and_lookup() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let p = tab.mint(mem_cap(7, t)).unwrap();
        let c = tab.lookup(p).unwrap();
        assert_eq!(c.object, 7);
        assert_eq!(c.kind, CapKind::Memory);
    }

    #[test]
    fn cannot_mint_for_other_tenant() {
        let mut tab = CapTable::new(TenantId(1));
        assert_eq!(
            tab.mint(mem_cap(1, TenantId(2))).unwrap_err(),
            CapError::CrossTenant
        );
    }

    #[test]
    fn revoke_empties_slot() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let p = tab.mint(mem_cap(1, t)).unwrap();
        tab.revoke(p).unwrap();
        assert_eq!(tab.lookup(p).unwrap_err(), CapError::EmptySlot);
    }

    #[test]
    fn derive_cannot_escalate() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let p = tab.mint(mem_cap(1, t)).unwrap();
        let weak = tab
            .derive(p, CapRights(CapRights::READ))
            .unwrap();
        assert!(tab.lookup(weak).unwrap().rights.contains(CapRights::READ));
        assert!(!tab.lookup(weak).unwrap().rights.contains(CapRights::WRITE));
        assert_eq!(
            tab.derive(weak, CapRights::MEM_FULL).unwrap_err(),
            CapError::WouldEscalate
        );
    }

    #[test]
    fn grant_moves_and_retargets_tenant() {
        let a = TenantId(1);
        let b = TenantId(2);
        let mut ta = CapTable::new(a);
        let mut tb = CapTable::new(b);
        let p = ta.mint(mem_cap(42, a)).unwrap();
        let q = ta
            .transfer(p, &mut tb, CapRights(CapRights::READ | CapRights::MAP), true)
            .unwrap();
        assert_eq!(ta.lookup(p).unwrap_err(), CapError::EmptySlot);
        let got = tb.lookup(q).unwrap();
        assert_eq!(got.object, 42);
        assert_eq!(got.tenant, b);
        assert!(!got.rights.contains(CapRights::WRITE));
    }

    #[test]
    fn require_checks_kind_and_rights() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let p = tab.mint(mem_cap(1, t)).unwrap();
        assert!(tab.require(p, CapKind::Memory, CapRights::READ).is_ok());
        assert_eq!(
            tab.require(p, CapKind::Endpoint, CapRights::READ)
                .unwrap_err(),
            CapError::WrongKind
        );
        assert_eq!(
            tab.require(p, CapKind::Memory, CapRights::SUBMIT)
                .unwrap_err(),
            CapError::InsufficientRights
        );
    }

    #[test]
    fn table_full() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        for i in 0..CAP_SLOTS {
            tab.mint(mem_cap(i as u32, t)).unwrap();
        }
        assert_eq!(tab.mint(mem_cap(99, t)).unwrap_err(), CapError::TableFull);
    }

    #[test]
    fn isolation_holds() {
        let a = TenantId(1);
        let b = TenantId(2);
        let mut ta = CapTable::new(a);
        let tb = CapTable::new(b);
        ta.mint(mem_cap(99, a)).unwrap();
        assert!(ta.holds(CapKind::Memory, 99));
        assert!(!tb.holds(CapKind::Memory, 99));
    }

    #[test]
    fn invalid_cptr() {
        let tab = CapTable::new(TenantId(1));
        assert_eq!(
            tab.lookup(CPtr(CAP_SLOTS as u16)).unwrap_err(),
            CapError::InvalidCptr
        );
    }
}
