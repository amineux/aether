//! CapTable CDT properties: exhaustive small cases + a seeded random walk.
//!
//! These are tests, not a seL4 proof and not an MDB. Identifiers `P-Revoke`,
//! `P-Unrelated`, `P-Named`, `P-Unforge`, and `P-Monotone` match
//! `docs/SECURITY.md`.

use super::*;

fn mem_cap(obj: u32, tenant: TenantId) -> Capability {
    Capability::new(CapKind::Memory, CapRights::MEM_FULL, obj, tenant)
}

fn ep_cap(obj: u32, tenant: TenantId) -> Capability {
    Capability::new(CapKind::Endpoint, CapRights::EP_FULL, obj, tenant)
}

fn live(tab: &CapTable) -> Vec<(CPtr, Capability)> {
    (0..CAP_SLOTS)
        .filter_map(|i| {
            let p = CPtr(i as u16);
            tab.lookup(p).ok().copied().map(|c| (p, c))
        })
        .collect()
}

fn live_caps(tabs: &[&CapTable]) -> Vec<Capability> {
    tabs.iter()
        .flat_map(|t| live(t).into_iter().map(|(_, c)| c))
        .collect()
}

/// Transitive kill set of `root` among `caps` (includes `root`).
fn kill_set(root: CdtNode, caps: &[Capability]) -> Vec<CdtNode> {
    let mut kill = vec![root];
    loop {
        let n = kill.len();
        for c in caps {
            if let Some(p) = c.parent {
                if kill.contains(&p) && !kill.contains(&c.cdt()) {
                    kill.push(c.cdt());
                }
            }
        }
        if kill.len() == n {
            break;
        }
    }
    kill
}

fn well_formed(tabs: &[&CapTable]) {
    let mut seen = Vec::new();
    for tab in tabs {
        for (_, c) in live(tab) {
            assert_eq!(
                c.tenant,
                tab.owner(),
                "P-Unforge: live cap tenant matches table"
            );
            assert_ne!(c.kind, CapKind::Empty);
            assert!(c.cdt().is_set(), "mint generation never uses 0");
            assert!(
                !seen.contains(&c.cdt()),
                "live CdtNode must be unique across named tables"
            );
            seen.push(c.cdt());
        }
    }
    let all = live_caps(tabs);
    for tab in tabs {
        for (_, c) in live(tab) {
            if let Some(p) = c.parent {
                if let Some(parent) = all.iter().find(|x| x.cdt() == p) {
                    assert!(
                        parent.rights.can_derive(c.rights),
                        "P-Monotone: live child rights ⊆ live parent"
                    );
                }
            }
        }
    }
}

fn assert_emptied(tab: &CapTable, cptr: CPtr) {
    assert_eq!(tab.lookup(cptr).unwrap_err(), CapError::EmptySlot);
}

fn assert_live_mem(tab: &CapTable, cptr: CPtr) {
    assert!(tab.require(cptr, CapKind::Memory, CapRights::READ).is_ok());
}

/// Build a Memory lineage: `depth` derive steps, `branch` children per node.
/// Intermediate nodes keep GRANT so the next level can derive.
fn build_lineage(
    tab: &mut CapTable,
    tenant: TenantId,
    obj: u32,
    depth: usize,
    branch: usize,
) -> (CPtr, Vec<CPtr>) {
    let root = tab.mint(mem_cap(obj, tenant)).unwrap();
    let mut descendants = Vec::new();
    let mut frontier = vec![root];
    for d in 0..depth {
        let mut next = Vec::new();
        let rights = if d + 1 == depth {
            CapRights(CapRights::READ | CapRights::GRANT)
        } else {
            CapRights(CapRights::READ | CapRights::GRANT | CapRights::MAP)
        };
        for &p in &frontier {
            for _ in 0..branch {
                let c = tab.derive(p, rights).unwrap();
                descendants.push(c);
                next.push(c);
            }
        }
        frontier = next;
    }
    (root, descendants)
}

fn submasks(mask: u16) -> Vec<CapRights> {
    let bits: Vec<u16> = (0..16)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| 1u16 << i)
        .collect();
    let n = 1usize << bits.len();
    (0..n)
        .map(|combo| {
            let mut v = 0u16;
            for (i, b) in bits.iter().enumerate() {
                if combo & (1 << i) != 0 {
                    v |= b;
                }
            }
            CapRights(v)
        })
        .collect()
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x as u32
    }

    fn bounded(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.next() % n
    }
}

fn pick_live(rng: &mut Rng, tab: &CapTable, skip: &[CPtr]) -> Option<CPtr> {
    let caps: Vec<CPtr> = live(tab)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| !skip.contains(p))
        .collect();
    if caps.is_empty() {
        return None;
    }
    Some(caps[rng.bounded(caps.len() as u32) as usize])
}

fn pick_grant(rng: &mut Rng, tab: &CapTable, skip: &[CPtr]) -> Option<CPtr> {
    let caps: Vec<CPtr> = live(tab)
        .into_iter()
        .filter(|(p, c)| !skip.contains(p) && c.rights.contains(CapRights::GRANT))
        .map(|(p, _)| p)
        .collect();
    if caps.is_empty() {
        return None;
    }
    Some(caps[rng.bounded(caps.len() as u32) as usize])
}

fn subset_rights(rng: &mut Rng, have: CapRights) -> CapRights {
    CapRights(have.0 & rng.next() as u16)
}

fn assert_revoke_in_empties(origin: &mut CapTable, others: &mut [&mut CapTable], cptr: CPtr) {
    let before = {
        let mut tabs: Vec<&CapTable> = vec![origin];
        for t in others.iter() {
            tabs.push(t);
        }
        live_caps(&tabs)
    };
    let cap = *origin.lookup(cptr).unwrap();
    let kill = kill_set(cap.cdt(), &before);
    origin.revoke_in(cptr, others).unwrap();
    let after = {
        let mut tabs: Vec<&CapTable> = vec![origin];
        for t in others.iter() {
            tabs.push(t);
        }
        live_caps(&tabs)
    };
    for c in after {
        assert!(
            !kill.contains(&c.cdt()),
            "P-Revoke: descendant {:?} still live after revoke_in",
            c.cdt()
        );
    }
}

// --- P-Revoke / P-Unrelated: exhaustive small trees ---

#[test]
fn p_revoke_exhaustive_small_trees() {
    let a = TenantId(1);
    let b = TenantId(2);
    for depth in 0..=3 {
        for branch in 1..=2 {
            let mut ta = CapTable::new(a);
            let mut tb = CapTable::new(b);
            let unrelated_a = ta.mint(mem_cap(0xA0, a)).unwrap();
            let unrelated_b = tb.mint(ep_cap(0xB0, b)).unwrap();
            let (root, descendants) = build_lineage(&mut ta, a, 0xCD, depth, branch);
            well_formed(&[&ta, &tb]);
            ta.revoke_in(root, &mut [&mut tb]).unwrap();
            assert_emptied(&ta, root);
            for d in descendants {
                assert_emptied(&ta, d);
            }
            assert_live_mem(&ta, unrelated_a);
            assert!(tb
                .require(unrelated_b, CapKind::Endpoint, CapRights::READ)
                .is_ok());
            well_formed(&[&ta, &tb]);
        }
    }
}

#[test]
fn p_revoke_cross_table_grandchild() {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut ta = CapTable::new(a);
    let mut tb = CapTable::new(b);
    let unrelated_b = tb.mint(ep_cap(3, b)).unwrap();
    let root = ta.mint(mem_cap(7, a)).unwrap();
    let child = ta
        .derive(root, CapRights(CapRights::READ | CapRights::GRANT))
        .unwrap();
    let granted = ta
        .transfer(
            child,
            &mut tb,
            CapRights(CapRights::READ | CapRights::GRANT),
            false,
        )
        .unwrap();
    let grand = tb.derive(granted, CapRights(CapRights::READ)).unwrap();
    let before = live_caps(&[&ta, &tb]);
    let kill = kill_set(ta.lookup(root).unwrap().cdt(), &before);
    assert!(kill.len() >= 4, "root + child + grant-copy + B-derive");
    ta.revoke_in(root, &mut [&mut tb]).unwrap();
    assert_emptied(&ta, root);
    assert_emptied(&ta, child);
    assert_emptied(&tb, granted);
    assert_emptied(&tb, grand);
    assert!(tb
        .require(unrelated_b, CapKind::Endpoint, CapRights::READ)
        .is_ok());
}

// --- P-Named: GRANT-across-tables only with revoke_in ---

#[test]
fn p_named_grant_copy_survives_revoke_without_others() {
    let a = TenantId(1);
    let b = TenantId(2);
    for depth in 0..=2 {
        let mut ta = CapTable::new(a);
        let mut tb = CapTable::new(b);
        let unrelated_b = tb.mint(ep_cap(9, b)).unwrap();
        let (root, descendants) = build_lineage(&mut ta, a, 11, depth, 1);
        let src = descendants.last().copied().unwrap_or(root);
        // Last-level derive keeps GRANT (see build_lineage).
        let foreign = ta
            .transfer(src, &mut tb, CapRights(CapRights::READ), false)
            .unwrap();
        ta.revoke(root).unwrap();
        assert_emptied(&ta, root);
        for d in &descendants {
            assert_emptied(&ta, *d);
        }
        assert!(
            tb.require(foreign, CapKind::Memory, CapRights::READ)
                .is_ok(),
            "P-Named: GRANT-copy in an unnamed table survives revoke"
        );
        assert!(tb
            .require(unrelated_b, CapKind::Endpoint, CapRights::READ)
            .is_ok());
    }
}

#[test]
fn p_named_grant_copy_dies_only_with_revoke_in() {
    let a = TenantId(1);
    let b = TenantId(2);
    for depth in 0..=2 {
        let mut ta = CapTable::new(a);
        let mut tb = CapTable::new(b);
        let unrelated_b = tb.mint(ep_cap(9, b)).unwrap();
        let (root, descendants) = build_lineage(&mut ta, a, 11, depth, 1);
        let src = descendants.last().copied().unwrap_or(root);
        let foreign = ta
            .transfer(src, &mut tb, CapRights(CapRights::READ), false)
            .unwrap();
        ta.revoke_in(root, &mut [&mut tb]).unwrap();
        assert_emptied(&ta, root);
        for d in &descendants {
            assert_emptied(&ta, *d);
        }
        assert_emptied(&tb, foreign);
        assert!(tb
            .require(unrelated_b, CapKind::Endpoint, CapRights::READ)
            .is_ok());
        assert!(!tb.holds(CapKind::Memory, 11));
    }
}

#[test]
fn p_named_grant_move_is_not_a_derivation_edge() {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut ta = CapTable::new(a);
    let mut tb = CapTable::new(b);
    let root = ta.mint(mem_cap(1, a)).unwrap();
    let child = ta
        .derive(root, CapRights(CapRights::READ | CapRights::GRANT))
        .unwrap();
    let moved = ta
        .transfer(child, &mut tb, CapRights(CapRights::READ), true)
        .unwrap();
    assert_emptied(&ta, child);
    // Move relocates the slot; A still holds the parent. Revoke of the
    // vacated CPtr is EmptySlot — there is no leftover derivation node
    // at `child` to walk.
    assert_eq!(ta.revoke(child).unwrap_err(), CapError::EmptySlot);
    assert_live_mem(&ta, root);
    assert!(tb.require(moved, CapKind::Memory, CapRights::READ).is_ok());
    // The moved cap kept the parent edge to root, so revoke_in of root
    // *does* collect it. That is parent-pointer revoke, not seL4 MDB move.
    ta.revoke_in(root, &mut [&mut tb]).unwrap();
    assert_emptied(&tb, moved);
}

#[test]
fn p_named_grant_move_of_root_does_not_walk_old_descendants() {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut ta = CapTable::new(a);
    let mut tb = CapTable::new(b);
    let root = ta.mint(mem_cap(1, a)).unwrap();
    let child = ta
        .derive(root, CapRights(CapRights::READ | CapRights::GRANT))
        .unwrap();
    let moved = ta
        .transfer(root, &mut tb, CapRights::MEM_FULL, true)
        .unwrap();
    assert_emptied(&ta, root);
    assert_live_mem(&ta, child);
    tb.revoke_in(moved, &mut [&mut ta]).unwrap();
    assert_emptied(&tb, moved);
    assert_live_mem(&ta, child);
}

// --- P-Unforge ---

#[test]
fn p_unforge_mint_rejects_foreign_tenant() {
    let mut ta = CapTable::new(TenantId(1));
    for tid in [2u32, 3, 99] {
        assert_eq!(
            ta.mint(mem_cap(1, TenantId(tid))).unwrap_err(),
            CapError::CrossTenant
        );
    }
    assert_eq!(ta.occupied(), 0);
}

#[test]
fn p_unforge_cptr_is_per_table() {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut ta = CapTable::new(a);
    let tb = CapTable::new(b);
    let mut a_ptrs = Vec::new();
    for i in 0..CAP_SLOTS {
        a_ptrs.push(ta.mint(mem_cap(1000 + i as u32, a)).unwrap());
    }
    for p in a_ptrs {
        let cap = ta.lookup(p).unwrap();
        assert_eq!(cap.tenant, a);
        assert_eq!(tb.lookup(p).unwrap_err(), CapError::EmptySlot);
        assert_eq!(
            tb.require(p, CapKind::Memory, CapRights::READ).unwrap_err(),
            CapError::EmptySlot
        );
    }
    assert!(!tb.holds(CapKind::Memory, 1000));
}

#[test]
fn p_unforge_same_slot_index_is_not_the_same_cap() {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut ta = CapTable::new(a);
    let mut tb = CapTable::new(b);
    let pa = ta.mint(mem_cap(7, a)).unwrap();
    let pb = tb.mint(mem_cap(7, b)).unwrap();
    assert_eq!(pa, pb, "both tables fill slot 0 first");
    let ca = ta.lookup(pa).unwrap();
    let cb = tb.lookup(pb).unwrap();
    assert_eq!(ca.tenant, a);
    assert_eq!(cb.tenant, b);
    assert_ne!(ca.cdt(), cb.cdt());
    assert!(ta.require(pa, CapKind::Memory, CapRights::READ).is_ok());
    assert!(tb.require(pb, CapKind::Memory, CapRights::READ).is_ok());
    // A's CPtr used in B names B's object, never A's capability record.
    let via_b = tb.lookup(pa).unwrap();
    assert_eq!(via_b.tenant, b);
    assert_ne!(via_b.cdt(), ca.cdt());
}

#[test]
fn p_unforge_grant_copy_retargets_tenant() {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut ta = CapTable::new(a);
    let mut tb = CapTable::new(b);
    let p = ta.mint(mem_cap(42, a)).unwrap();
    let q = ta
        .transfer(p, &mut tb, CapRights(CapRights::READ), false)
        .unwrap();
    assert_eq!(tb.lookup(q).unwrap().tenant, b);
    assert_eq!(tb.owner(), b);
    well_formed(&[&ta, &tb]);
}

#[test]
fn p_unforge_stale_cptr_after_revoke_until_remint() {
    let t = TenantId(1);
    let mut tab = CapTable::new(t);
    let p = tab.mint(mem_cap(1, t)).unwrap();
    let gen = tab.lookup(p).unwrap().generation;
    tab.revoke(p).unwrap();
    assert_eq!(tab.lookup(p).unwrap_err(), CapError::EmptySlot);
    let q = tab.mint(mem_cap(2, t)).unwrap();
    let fresh = tab.lookup(q).unwrap();
    assert_ne!(fresh.generation, gen);
    assert_eq!(fresh.object, 2);
}

// --- P-Monotone ---

#[test]
fn p_monotone_all_mem_full_subsets() {
    let t = TenantId(1);
    let parent_rights = CapRights::MEM_FULL;
    for new in submasks(parent_rights.0) {
        let mut tab = CapTable::new(t);
        let p = tab.mint(mem_cap(1, t)).unwrap();
        let r = tab.derive(p, new);
        if parent_rights.can_derive(new) {
            let c = *tab.lookup(r.unwrap()).unwrap();
            assert_eq!(c.rights, new);
            assert_eq!(c.parent, Some(tab.lookup(p).unwrap().cdt()));
        } else {
            assert_eq!(r.unwrap_err(), CapError::WouldEscalate);
        }
    }
    // UNIFIED is never in MEM_FULL — deriving it is escalation.
    let mut tab = CapTable::new(t);
    let p = tab.mint(mem_cap(1, t)).unwrap();
    assert_eq!(
        tab.derive(p, CapRights(CapRights::READ | CapRights::UNIFIED))
            .unwrap_err(),
        CapError::WouldEscalate
    );
}

#[test]
fn p_monotone_grant_required() {
    let t = TenantId(1);
    let mut tab = CapTable::new(t);
    let p = tab
        .mint(Capability::new(
            CapKind::Memory,
            CapRights(CapRights::READ | CapRights::WRITE | CapRights::MAP),
            1,
            t,
        ))
        .unwrap();
    assert_eq!(
        tab.derive(p, CapRights(CapRights::READ)).unwrap_err(),
        CapError::InsufficientRights
    );
    let mut dest = CapTable::new(TenantId(2));
    assert_eq!(
        tab.transfer(p, &mut dest, CapRights(CapRights::READ), false)
            .unwrap_err(),
        CapError::InsufficientRights
    );
}

#[test]
fn p_monotone_transfer_rejects_escalation() {
    let a = TenantId(1);
    let mut ta = CapTable::new(a);
    let mut tb = CapTable::new(TenantId(2));
    let p = ta.mint(mem_cap(1, a)).unwrap();
    assert_eq!(
        ta.transfer(p, &mut tb, CapRights::ALL, false).unwrap_err(),
        CapError::WouldEscalate
    );
    assert_eq!(
        ta.transfer(p, &mut tb, CapRights::ALL, true).unwrap_err(),
        CapError::WouldEscalate
    );
}

// --- Seeded random walk over mint / derive / grant / revoke_in ---

#[test]
fn p_random_walk_mint_derive_grant_revoke() {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut rng = Rng(0xA37E_4CD7);
    let mut ta = CapTable::new(a);
    let mut tb = CapTable::new(b);
    let sent_a = ta.mint(mem_cap(0x51, a)).unwrap();
    let sent_b = tb.mint(ep_cap(0x52, b)).unwrap();
    let skip_a = [sent_a];
    let skip_b = [sent_b];

    for _ in 0..400 {
        well_formed(&[&ta, &tb]);
        match rng.bounded(10) {
            0 | 1 => {
                let obj = 0x100 + rng.bounded(32);
                let _ = ta.mint(mem_cap(obj, a));
            }
            2 => {
                let obj = 0x200 + rng.bounded(32);
                let _ = tb.mint(ep_cap(obj, b));
            }
            3 | 4 => {
                if let Some(src) = pick_grant(&mut rng, &ta, &skip_a) {
                    let have = ta.lookup(src).unwrap().rights;
                    let new = subset_rights(&mut rng, have);
                    let _ = ta.derive(src, new);
                }
            }
            5 => {
                if let Some(src) = pick_grant(&mut rng, &tb, &skip_b) {
                    let have = tb.lookup(src).unwrap().rights;
                    let new = subset_rights(&mut rng, have);
                    let _ = tb.derive(src, new);
                }
            }
            6 => {
                if let Some(src) = pick_grant(&mut rng, &ta, &skip_a) {
                    let have = ta.lookup(src).unwrap().rights;
                    let new = subset_rights(&mut rng, have);
                    let _ = ta.transfer(src, &mut tb, new, false);
                }
            }
            7 => {
                if let Some(src) = pick_live(&mut rng, &ta, &skip_a) {
                    assert_revoke_in_empties(&mut ta, &mut [&mut tb], src);
                    assert_live_mem(&ta, sent_a);
                    assert!(tb
                        .require(sent_b, CapKind::Endpoint, CapRights::READ)
                        .is_ok());
                }
            }
            8 => {
                if let Some(src) = pick_live(&mut rng, &ta, &skip_a) {
                    // P-Named: revoke without others must not empty B sentinels
                    // and must not empty GRANT-children in B.
                    let before_b: Vec<(CPtr, CdtNode, Option<CdtNode>)> = live(&tb)
                        .into_iter()
                        .map(|(p, c)| (p, c.cdt(), c.parent))
                        .collect();
                    let cap = *ta.lookup(src).unwrap();
                    let a_only = live_caps(&[&ta]);
                    let kill_a = kill_set(cap.cdt(), &a_only);
                    ta.revoke(src).unwrap();
                    for (p, cdt, parent) in before_b {
                        let still = tb.lookup(p);
                        if parent.map(|par| kill_a.contains(&par)).unwrap_or(false)
                            || kill_a.contains(&cdt)
                        {
                            // Foreign child of the revoked lineage — must survive
                            // because B was not named.
                            assert!(
                                still.is_ok(),
                                "P-Named: GRANT-child in unnamed table must live"
                            );
                        }
                    }
                    assert!(tb
                        .require(sent_b, CapKind::Endpoint, CapRights::READ)
                        .is_ok());
                    assert_live_mem(&ta, sent_a);
                }
            }
            _ => {
                if let Some(src) = pick_live(&mut rng, &tb, &skip_b) {
                    assert_revoke_in_empties(&mut tb, &mut [&mut ta], src);
                    assert_live_mem(&ta, sent_a);
                    assert!(tb
                        .require(sent_b, CapKind::Endpoint, CapRights::READ)
                        .is_ok());
                }
            }
        }
    }
    well_formed(&[&ta, &tb]);
    assert_live_mem(&ta, sent_a);
    assert!(tb
        .require(sent_b, CapKind::Endpoint, CapRights::READ)
        .is_ok());
}

#[test]
fn p_mint_roots_have_no_parent() {
    let t = TenantId(1);
    let mut tab = CapTable::new(t);
    for i in 0..8 {
        let p = tab.mint(mem_cap(i, t)).unwrap();
        let c = tab.lookup(p).unwrap();
        assert_eq!(c.parent, None);
        assert_eq!(c.cdt().owner, t);
    }
}
