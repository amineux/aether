//! Unforgeable capabilities for memory, endpoints, and accelerator queues.
//!
//! Userspace (and built-in tasks) never hold kernel object pointers. They hold
//! a `CPtr` — an index into their own cap table. The kernel looks up the slot,
//! checks generation + rights + tenant, then touches the object.
//!
//! This is the seL4-inspired invariant Aether bets the fabric on:
//! **no communication or DMA exists outside a capability**.
//!
//! Derivation edges (`parent` / `cdt`) let `revoke` empty descendants. This is
//! a small parent pointer, not a proof. Host property tests (`caps_props.rs`)
//! lock mint → derive → `revoke_in` (named tables only).

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
    SpectralCut = 5,
    FlowQuota = 6,
    /// A compute unit (CPU tile, virt accel, …) published as a fabric object.
    Activity = 7,
    /// Spatial slice + QoS + blast-radius profile.
    Partition = 8,
    /// Compiled collective (tree / ring / torus) bound to a Hodge class.
    OperatorKernel = 9,
}

impl CapKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Empty),
            1 => Some(Self::Memory),
            2 => Some(Self::Endpoint),
            3 => Some(Self::AccelQueue),
            4 => Some(Self::Notification),
            5 => Some(Self::SpectralCut),
            6 => Some(Self::FlowQuota),
            7 => Some(Self::Activity),
            8 => Some(Self::Partition),
            9 => Some(Self::OperatorKernel),
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
    /// Bind a task / job to a SpectralCut or Partition (placement refusal).
    pub const BIND: u16 = 1 << 7;
    /// Explicit UNIFIED_MEMORY. Never implied by [`Self::MEM_FULL`].
    /// A buffer in a typed space stays non-coherent unless this bit is granted.
    pub const UNIFIED: u16 = 1 << 8;

    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(0x1FF);
    pub const MEM_FULL: Self = Self(Self::READ | Self::WRITE | Self::GRANT | Self::MAP);
    pub const EP_FULL: Self = Self(Self::READ | Self::WRITE | Self::GRANT);
    pub const ACCEL_FULL: Self = Self(Self::SUBMIT | Self::WAIT | Self::GRANT | Self::READ);
    pub const CUT_FULL: Self = Self(Self::READ | Self::BIND | Self::GRANT);
    pub const HODGE_FULL: Self = Self(Self::READ | Self::WRITE | Self::GRANT);
    pub const ACTIVITY_FULL: Self = Self(Self::SUBMIT | Self::WAIT | Self::BIND | Self::GRANT);
    pub const PARTITION_FULL: Self = Self(Self::BIND | Self::SUBMIT | Self::GRANT);
    /// Bind + inject a compiled collective; GRANT to derive / transfer.
    pub const OPKERNEL_FULL: Self = Self(Self::READ | Self::BIND | Self::SUBMIT | Self::GRANT);

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

/// Derivation-tree identity. Unique among caps minted in one table
/// (`owner` + `node`). Children store the parent's node, including when
/// the child lives in another table after GRANT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CdtNode {
    pub owner: TenantId,
    pub node: u32,
}

impl CdtNode {
    pub const fn new(owner: TenantId, node: u32) -> Self {
        Self { owner, node }
    }

    pub const fn is_set(self) -> bool {
        self.node != 0
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
    /// Parent in the derivation tree. `None` = minted root.
    pub parent: Option<CdtNode>,
}

impl Capability {
    /// Derivation identity: table owner at mint + generation.
    pub const fn cdt(self) -> CdtNode {
        CdtNode::new(self.tenant, self.generation)
    }

    pub const fn new(kind: CapKind, rights: CapRights, object: u32, tenant: TenantId) -> Self {
        Self {
            kind,
            rights,
            object,
            badge: 0,
            generation: 0,
            tenant,
            parent: None,
        }
    }

    pub const fn with_badge(mut self, badge: u64) -> Self {
        self.badge = badge;
        self
    }

    pub const fn with_generation(mut self, generation: u32) -> Self {
        self.generation = generation;
        self
    }
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

/// Fixed-size kill set for revoke walks. `CAP_SLOTS` per table × two tables
/// covers the host/QEMU demo (one table, or parent + grant target).
const CDT_KILL_CAP: usize = CAP_SLOTS * 2;

struct KillSet {
    nodes: [Option<CdtNode>; CDT_KILL_CAP],
    len: usize,
}

impl KillSet {
    fn new() -> Self {
        Self {
            nodes: [None; CDT_KILL_CAP],
            len: 0,
        }
    }

    fn contains(&self, n: CdtNode) -> bool {
        self.nodes.iter().take(self.len).any(|s| *s == Some(n))
    }

    fn insert(&mut self, n: CdtNode) -> bool {
        if self.contains(n) {
            return false;
        }
        if self.len >= CDT_KILL_CAP {
            return false;
        }
        self.nodes[self.len] = Some(n);
        self.len += 1;
        true
    }
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

    pub fn require(&self, cptr: CPtr, kind: CapKind, need: u16) -> Result<&Capability, CapError> {
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

    /// Empty one slot without walking descendants. Used for GRANT-move.
    fn take_slot(&mut self, cptr: CPtr) -> Result<Capability, CapError> {
        let i = cptr.0 as usize;
        if i >= CAP_SLOTS {
            return Err(CapError::InvalidCptr);
        }
        self.slots[i].take().ok_or(CapError::EmptySlot)
    }

    /// Delete `cptr` and every descendant in this table.
    ///
    /// A stale CPtr cannot be reused on the same index: the slot is empty
    /// until a later mint. Cross-table children need [`Self::revoke_in`].
    pub fn revoke(&mut self, cptr: CPtr) -> Result<Capability, CapError> {
        self.revoke_in(cptr, &mut [])
    }

    /// Revoke `cptr` in this table and empty descendants in `others` too.
    ///
    /// Caller must not pass `self` in `others`. This is an explicit walk of
    /// named tables — not a kernel-global CNode broadcast.
    pub fn revoke_in(
        &mut self,
        cptr: CPtr,
        others: &mut [&mut CapTable],
    ) -> Result<Capability, CapError> {
        let cap = self.take_slot(cptr)?;
        let mut kill = KillSet::new();
        kill.insert(cap.cdt());
        loop {
            let before = kill.len;
            self.collect_into(&mut kill);
            for t in others.iter() {
                t.collect_into(&mut kill);
            }
            if kill.len == before {
                break;
            }
        }
        self.empty_killed(&kill);
        for t in others.iter_mut() {
            t.empty_killed(&kill);
        }
        Ok(cap)
    }

    fn collect_into(&self, kill: &mut KillSet) {
        for cap in self.slots.iter().flatten() {
            if let Some(parent) = cap.parent {
                if kill.contains(parent) {
                    kill.insert(cap.cdt());
                }
            }
        }
    }

    fn empty_killed(&mut self, kill: &KillSet) {
        for slot in self.slots.iter_mut() {
            if let Some(cap) = slot {
                if kill.contains(cap.cdt()) {
                    *slot = None;
                }
            }
        }
    }

    /// Move (GRANT) or copy-with-subset (derive) a cap into `dest`.
    ///
    /// A copy records a derivation edge so later [`Self::revoke_in`] of `src`
    /// empties the dest child. A move relocates the slot and does not revoke
    /// existing descendants.
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
        if r#move {
            let cptr = dest.mint(minted)?;
            let _ = self.take_slot(src);
            Ok(cptr)
        } else {
            minted.parent = Some(cap.cdt());
            dest.mint(minted)
        }
    }

    /// Derive a weaker cap in the *same* table. Child stores `src` as parent.
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
        child.parent = Some(cap.cdt());
        self.mint(child)
    }

    /// Isolation helper: does this table hold any cap to `object` of `kind`?
    pub fn holds(&self, kind: CapKind, object: u32) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|c| c.kind == kind && c.object == object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_cap(obj: u32, tenant: TenantId) -> Capability {
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, obj, tenant)
    }

    fn ep_cap(obj: u32, tenant: TenantId) -> Capability {
        Capability::new(CapKind::Endpoint, CapRights::EP_FULL, obj, tenant)
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
        let weak = tab.derive(p, CapRights(CapRights::READ)).unwrap();
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
            .transfer(
                p,
                &mut tb,
                CapRights(CapRights::READ | CapRights::MAP),
                true,
            )
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

    #[test]
    fn unified_is_not_in_mem_full() {
        assert!(!CapRights::MEM_FULL.contains(CapRights::UNIFIED));
        assert!(CapRights::ALL.contains(CapRights::UNIFIED));
        assert_eq!(CapKind::from_u8(7), Some(CapKind::Activity));
        assert_eq!(CapKind::from_u8(8), Some(CapKind::Partition));
        assert_eq!(CapKind::from_u8(9), Some(CapKind::OperatorKernel));
    }

    #[test]
    fn derive_records_parent() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let parent = tab.mint(mem_cap(1, t)).unwrap();
        let child = tab.derive(parent, CapRights(CapRights::READ)).unwrap();
        let p = tab.lookup(parent).unwrap();
        let c = tab.lookup(child).unwrap();
        assert_eq!(c.parent, Some(p.cdt()));
        assert_ne!(c.cdt(), p.cdt());
        assert!(c.cdt().is_set());
    }

    #[test]
    fn revoke_parent_empties_descendants_unrelated_lives() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let parent = tab.mint(mem_cap(1, t)).unwrap();
        let child = tab
            .derive(parent, CapRights(CapRights::READ | CapRights::GRANT))
            .unwrap();
        let grandchild = tab.derive(child, CapRights(CapRights::READ)).unwrap();
        let other = tab.mint(mem_cap(99, t)).unwrap();
        tab.revoke(parent).unwrap();
        assert_eq!(tab.lookup(parent).unwrap_err(), CapError::EmptySlot);
        assert_eq!(
            tab.require(child, CapKind::Memory, CapRights::READ)
                .unwrap_err(),
            CapError::EmptySlot
        );
        assert_eq!(
            tab.require(grandchild, CapKind::Memory, CapRights::READ)
                .unwrap_err(),
            CapError::EmptySlot
        );
        assert!(tab.require(other, CapKind::Memory, CapRights::READ).is_ok());
    }

    #[test]
    fn revoke_child_does_not_kill_parent() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let parent = tab.mint(mem_cap(1, t)).unwrap();
        let child = tab.derive(parent, CapRights(CapRights::READ)).unwrap();
        tab.revoke(child).unwrap();
        assert_eq!(tab.lookup(child).unwrap_err(), CapError::EmptySlot);
        assert!(tab
            .require(parent, CapKind::Memory, CapRights::READ)
            .is_ok());
    }

    #[test]
    fn revoke_parent_empties_grant_child_in_other_table() {
        let a = TenantId(1);
        let b = TenantId(2);
        let mut ta = CapTable::new(a);
        let mut tb = CapTable::new(b);
        let parent = ta.mint(mem_cap(7, a)).unwrap();
        let child = ta
            .transfer(parent, &mut tb, CapRights(CapRights::READ), false)
            .unwrap();
        let unrelated = tb.mint(ep_cap(3, b)).unwrap();
        ta.revoke_in(parent, &mut [&mut tb]).unwrap();
        assert_eq!(ta.lookup(parent).unwrap_err(), CapError::EmptySlot);
        assert_eq!(
            tb.require(child, CapKind::Memory, CapRights::READ)
                .unwrap_err(),
            CapError::EmptySlot
        );
        assert!(tb
            .require(unrelated, CapKind::Endpoint, CapRights::READ)
            .is_ok());
        assert!(!tb.holds(CapKind::Memory, 7));
    }

    #[test]
    fn revoke_without_others_leaves_foreign_child() {
        let a = TenantId(1);
        let b = TenantId(2);
        let mut ta = CapTable::new(a);
        let mut tb = CapTable::new(b);
        let parent = ta.mint(mem_cap(7, a)).unwrap();
        let child = ta
            .transfer(parent, &mut tb, CapRights(CapRights::READ), false)
            .unwrap();
        ta.revoke(parent).unwrap();
        assert_eq!(ta.lookup(parent).unwrap_err(), CapError::EmptySlot);
        assert!(tb.require(child, CapKind::Memory, CapRights::READ).is_ok());
    }
}

/// Property / exhaustive CDT tests. See `docs/SECURITY.md`.
#[cfg(test)]
#[path = "caps_props.rs"]
mod caps_props;
