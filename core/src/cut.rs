//! ChipletSpectralCut — a first-class partition of the package graph.
//!
//! A `SpectralCut` is a (nearly) balanced bipartition of the chiplet
//! interconnect + memory-affinity graph, minted as a `CapKind::SpectralCut`.
//! Tasks bind via `CapRights::BIND`. The scheduler refuses a placement
//! whose tile and bank sit on opposite sides of the bound cut.
//!
//! Placement for `n ≤ 32` uses the Fiedler median-cut of
//! [`crate::laplacian::AffinityLaplacian`] ([`SpectralCut::from_fiedler`] /
//! [`SpectralCut::from_placement`]). For `n ≤ ENUM_MAX` (8),
//! [`SpectralCut::min_balanced`] still enumerates — the combinatorial
//! problem Fiedler approximates. Enumeration is O(2ⁿ·n²) and is refused
//! above that gate. This is a prototype eigensolve, not GiFt-Placer and
//! not an EDA package solver.
//!
//! The QEMU topology is two chiplets with weak inter-die edges; the
//! min-conductance split is the chiplet cut.

use crate::caps::{CPtr, CapError, CapKind, CapRights, CapTable};
use crate::laplacian::AffinityLaplacian;
use crate::types::{BankId, TenantId, TileId};
use crate::window::TypedWindow;

/// Dense affinity-graph capacity. Masks are `u32`, so this is also the
/// host-tested placement ceiling.
pub const MAX_VERTS: usize = 32;
/// Balanced-mask enumeration stays at this n. Above it, use Fiedler.
pub const ENUM_MAX: usize = 8;
pub const MAX_CUTS_SCHED: usize = 4;

/// Bitmask of vertices `0..n`. `n == 32` is `u32::MAX` (a `1u32 << 32`
/// shift is undefined).
pub const fn vert_mask(n: usize) -> u32 {
    if n == 0 {
        0
    } else if n >= 32 {
        u32::MAX
    } else {
        (1u32 << n) - 1
    }
}

/// Vertex bit `1 << i`, or 0 if `i` is out of the u32 mask.
pub const fn vert_bit(i: usize) -> u32 {
    if i >= 32 {
        0
    } else {
        1u32 << i
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertKind {
    Tile(TileId),
    Bank(BankId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub kind: VertKind,
    pub chiplet: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffinityGraph {
    pub n: usize,
    pub verts: [Vertex; MAX_VERTS],
    pub w: [[u16; MAX_VERTS]; MAX_VERTS],
}

impl AffinityGraph {
    pub const fn empty() -> Self {
        Self {
            n: 0,
            verts: [Vertex {
                kind: VertKind::Tile(TileId(0)),
                chiplet: 0,
            }; MAX_VERTS],
            w: [[0; MAX_VERTS]; MAX_VERTS],
        }
    }

    pub fn add_vert(&mut self, v: Vertex) -> Option<usize> {
        if self.n >= MAX_VERTS {
            return None;
        }
        let i = self.n;
        self.verts[i] = v;
        self.n += 1;
        Some(i)
    }

    pub fn add_edge(&mut self, a: usize, b: usize, weight: u16) {
        if a >= self.n || b >= self.n || a == b {
            return;
        }
        self.w[a][b] = self.w[a][b].saturating_add(weight);
        self.w[b][a] = self.w[b][a].saturating_add(weight);
    }

    pub fn find_tile(&self, t: TileId) -> Option<usize> {
        self.verts[..self.n]
            .iter()
            .position(|v| v.kind == VertKind::Tile(t))
    }

    pub fn find_bank(&self, b: BankId) -> Option<usize> {
        self.verts[..self.n]
            .iter()
            .position(|v| v.kind == VertKind::Bank(b))
    }

    pub fn degree(&self, i: usize) -> u32 {
        self.w[i][..self.n].iter().map(|x| *x as u32).sum()
    }

    pub fn vol(&self, mask: u32) -> u32 {
        (0..self.n)
            .filter(|i| mask & vert_bit(*i) != 0)
            .map(|i| self.degree(i))
            .sum()
    }

    pub fn cut_weight(&self, mask: u32) -> u32 {
        let mut c = 0u32;
        for i in 0..self.n {
            if mask & vert_bit(i) == 0 {
                continue;
            }
            for j in 0..self.n {
                if mask & vert_bit(j) == 0 {
                    c += self.w[i][j] as u32;
                }
            }
        }
        c
    }

    /// Φ(S) = cut(S,V\S) / min(vol S, vol V\S), in thousandths.
    pub fn conductance_milli(&self, mask: u32) -> Option<u32> {
        let all = vert_mask(self.n);
        if mask == 0 || mask == all {
            return None;
        }
        let vs = self.vol(mask);
        let vt = self.vol(all & !mask);
        let den = vs.min(vt);
        if den == 0 {
            return None;
        }
        Some(self.cut_weight(mask).saturating_mul(1000) / den)
    }

    /// QEMU research package: 2 chiplets × (CPU + accel + HBM).
    ///
    /// ```text
    /// chiplet 0                chiplet 1
    ///  tile0 CPU ── tile2 NPU    tile1 CPU ── tile3 GPU
    ///       \        /                 \        /
    ///          bank0                      bank1
    ///              \────── EMIB ──────/
    /// ```
    pub fn qemu_package() -> Self {
        let mut g = Self::empty();
        g.add_vert(Vertex {
            kind: VertKind::Tile(TileId(0)),
            chiplet: 0,
        });
        g.add_vert(Vertex {
            kind: VertKind::Tile(TileId(2)),
            chiplet: 0,
        });
        g.add_vert(Vertex {
            kind: VertKind::Bank(BankId(0)),
            chiplet: 0,
        });
        g.add_vert(Vertex {
            kind: VertKind::Tile(TileId(1)),
            chiplet: 1,
        });
        g.add_vert(Vertex {
            kind: VertKind::Tile(TileId(3)),
            chiplet: 1,
        });
        g.add_vert(Vertex {
            kind: VertKind::Bank(BankId(1)),
            chiplet: 1,
        });
        // Strong intra-chiplet (HBM + on-die mesh).
        g.add_edge(0, 1, 8);
        g.add_edge(0, 2, 10);
        g.add_edge(1, 2, 10);
        g.add_edge(3, 4, 8);
        g.add_edge(3, 5, 10);
        g.add_edge(4, 5, 10);
        // Weak inter-chiplet (EMIB / UALink stand-in).
        g.add_edge(0, 3, 2);
        g.add_edge(1, 4, 2);
        g.add_edge(2, 5, 1);
        g
    }

    /// Two-chiplet synthetic package used for n=16 / n=32 placement smokes.
    ///
    /// Not a package netlist. Chiplet 0 owns verts `0..n/2` (last is
    /// `BankId(0)`); chiplet 1 owns the rest (`BankId(1)` at `n-1`). Strong
    /// intra-die ring + bank star; weak EMIB between corresponding verts.
    /// `n` must be even and in `4..=MAX_VERTS`.
    pub fn two_chiplet_mesh(n: usize) -> Self {
        let mut g = Self::empty();
        if n < 4 || n > MAX_VERTS || n % 2 != 0 {
            return g;
        }
        let half = n / 2;
        for i in 0..n {
            let chiplet = if i < half { 0u8 } else { 1u8 };
            let is_bank = i == half - 1 || i == n - 1;
            let v = if is_bank {
                Vertex {
                    kind: VertKind::Bank(BankId(chiplet)),
                    chiplet,
                }
            } else {
                Vertex {
                    kind: VertKind::Tile(TileId(i as u16)),
                    chiplet,
                }
            };
            g.add_vert(v);
        }
        for c in 0..2 {
            let base = c * half;
            for k in 0..half {
                let a = base + k;
                let b = base + (k + 1) % half;
                g.add_edge(a, b, 8);
                let d = base + (k + 2) % half;
                g.add_edge(a, d, 6);
            }
            let bank = base + half - 1;
            for k in 0..half - 1 {
                g.add_edge(base + k, bank, 10);
            }
        }
        for k in 0..half {
            g.add_edge(k, half + k, 1);
        }
        g
    }

    /// Chiplet-0 mask for [`two_chiplet_mesh`]: the low `n/2` bits.
    pub fn mesh_chiplet0_mask(n: usize) -> u32 {
        vert_mask(n / 2)
    }

    pub fn first_tile_on(&self, chiplet: u8) -> Option<TileId> {
        self.nth_tile_on(chiplet, 0)
    }

    /// `which`-th tile on `chiplet` in vertex order (0-based).
    pub fn nth_tile_on(&self, chiplet: u8, which: usize) -> Option<TileId> {
        self.verts[..self.n]
            .iter()
            .filter_map(|v| match v.kind {
                VertKind::Tile(t) if v.chiplet == chiplet => Some(t),
                _ => None,
            })
            .nth(which)
    }

    pub fn chiplet_of_tile(&self, t: TileId) -> Option<u8> {
        self.verts[..self.n].iter().find_map(|v| match v.kind {
            VertKind::Tile(id) if id == t => Some(v.chiplet),
            _ => None,
        })
    }

    pub fn bank_on(&self, chiplet: u8) -> Option<BankId> {
        self.verts[..self.n].iter().find_map(|v| match v.kind {
            VertKind::Bank(b) if v.chiplet == chiplet => Some(b),
            _ => None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CutId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutError {
    CrossCut,
    ConductanceExceeded,
    Unbalanced,
    EmptyPart,
    UnknownVertex,
    NoCut,
    NotBound,
    /// Enumeration (`min_balanced`) refused: n > [`ENUM_MAX`].
    TooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectralCut {
    pub id: CutId,
    pub left: u32,
    pub right: u32,
    pub phi_milli: u32,
    pub bound_milli: u32,
}

impl SpectralCut {
    pub fn from_mask(
        id: CutId,
        g: &AffinityGraph,
        left: u32,
        bound_milli: u32,
    ) -> Result<Self, CutError> {
        let all = vert_mask(g.n);
        let left = left & all;
        let right = all & !left;
        if left == 0 || right == 0 {
            return Err(CutError::EmptyPart);
        }
        let sl = left.count_ones();
        let sr = right.count_ones();
        if sl.abs_diff(sr) > 1 {
            return Err(CutError::Unbalanced);
        }
        let phi = g.conductance_milli(left).ok_or(CutError::EmptyPart)?;
        if phi > bound_milli {
            return Err(CutError::ConductanceExceeded);
        }
        Ok(Self {
            id,
            left,
            right,
            phi_milli: phi,
            bound_milli,
        })
    }

    /// Hand-built chiplet bipartition (Fiedler-aligned on the QEMU graph).
    pub fn qemu_chiplet_cut(bound_milli: u32) -> Result<(AffinityGraph, Self), CutError> {
        let g = AffinityGraph::qemu_package();
        let left = 0b000111; // verts 0,1,2 = chiplet 0
        let cut = Self::from_mask(CutId(1), &g, left, bound_milli)?;
        Ok((g, cut))
    }

    /// Median-cut of [`AffinityLaplacian::fiedler_mask`]. This is the
    /// n≤32 placement constructor. Enumeration stays on [`Self::min_balanced`]
    /// for n ≤ [`ENUM_MAX`] only.
    pub fn from_fiedler(id: CutId, g: &AffinityGraph, bound_milli: u32) -> Result<Self, CutError> {
        if g.n == 0 || g.n > MAX_VERTS {
            return Err(CutError::EmptyPart);
        }
        let lap = AffinityLaplacian::from_graph(g);
        Self::from_mask(id, g, lap.fiedler_mask(), bound_milli)
    }

    /// Alias for [`Self::from_fiedler`]: laplacian-informed placement.
    pub fn from_placement(
        id: CutId,
        g: &AffinityGraph,
        bound_milli: u32,
    ) -> Result<Self, CutError> {
        Self::from_fiedler(id, g, bound_milli)
    }

    /// Enumerate balanced masks; pick minimum conductance. This is what a
    /// Fiedler sweep approximates when n is large. Refuses n > [`ENUM_MAX`].
    pub fn min_balanced(id: CutId, g: &AffinityGraph, bound_milli: u32) -> Result<Self, CutError> {
        if g.n == 0 {
            return Err(CutError::EmptyPart);
        }
        if g.n > ENUM_MAX {
            return Err(CutError::TooLarge);
        }
        let all = vert_mask(g.n);
        let target = g.n as u32 / 2;
        let mut best: Option<(u32, u32)> = None; // mask, phi
        for mask in 1..all {
            let c = mask.count_ones();
            if c.abs_diff(target) > 1 {
                continue;
            }
            // Canonical: only masks with bit 0 set, so we do not double-count.
            if mask & 1 == 0 {
                continue;
            }
            let Some(phi) = g.conductance_milli(mask) else {
                continue;
            };
            match best {
                None => best = Some((mask, phi)),
                Some((_, bp)) if phi < bp => best = Some((mask, phi)),
                _ => {}
            }
        }
        let (left, _) = best.ok_or(CutError::EmptyPart)?;
        Self::from_mask(id, g, left, bound_milli)
    }

    pub fn side_of(&self, vi: usize) -> Option<Side> {
        let bit = vert_bit(vi);
        if self.left & bit != 0 {
            Some(Side::Left)
        } else if self.right & bit != 0 {
            Some(Side::Right)
        } else {
            None
        }
    }

    pub fn allow_place(
        &self,
        g: &AffinityGraph,
        tile: TileId,
        bank: Option<BankId>,
    ) -> Result<Side, CutError> {
        if self.phi_milli > self.bound_milli {
            return Err(CutError::ConductanceExceeded);
        }
        let ti = g.find_tile(tile).ok_or(CutError::UnknownVertex)?;
        let ts = self.side_of(ti).ok_or(CutError::UnknownVertex)?;
        if let Some(b) = bank {
            let bi = g.find_bank(b).ok_or(CutError::UnknownVertex)?;
            let bs = self.side_of(bi).ok_or(CutError::UnknownVertex)?;
            if ts != bs {
                return Err(CutError::CrossCut);
            }
        }
        Ok(ts)
    }

    /// Typed-window bind. A foreign-tenant window is [`CutError::CrossCut`].
    ///
    /// This is the SpectralCut gate for Exploration E (`TypedWindow`).
    /// It does not program CXL.mem and does not replace [`Self::allow_place`].
    pub fn allow_window(&self, win: &TypedWindow, caller: TenantId) -> Result<(), CutError> {
        let _ = self;
        if win.tenant != caller {
            Err(CutError::CrossCut)
        } else {
            Ok(())
        }
    }
}

/// Cap surface: BIND is required. Without it the cut object is inert.
pub fn require_cut_bind(tab: &CapTable, cptr: CPtr) -> Result<&crate::caps::Capability, CapError> {
    tab.require(cptr, CapKind::SpectralCut, CapRights::BIND)
}

/// Bind + placement check used by the scheduler and the demo.
pub fn bind_place(
    tab: &CapTable,
    cptr: CPtr,
    cut: &SpectralCut,
    g: &AffinityGraph,
    tile: TileId,
    bank: Option<BankId>,
) -> Result<Side, CutError> {
    require_cut_bind(tab, cptr).map_err(|_| CutError::NotBound)?;
    cut.allow_place(g, tile, bank)
}

/// BIND a SpectralCut, then refuse a foreign-tenant [`TypedWindow`].
pub fn bind_window(
    tab: &CapTable,
    cptr: CPtr,
    cut: &SpectralCut,
    win: &TypedWindow,
    caller: TenantId,
) -> Result<(), CutError> {
    require_cut_bind(tab, cptr).map_err(|_| CutError::NotBound)?;
    cut.allow_window(win, caller)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{CapTable, Capability};
    use crate::types::TenantId;

    #[test]
    fn qemu_cut_under_bound() {
        let (g, cut) = SpectralCut::qemu_chiplet_cut(400).unwrap();
        assert_eq!(cut.left, 0b000111);
        assert!(cut.phi_milli > 0 && cut.phi_milli <= 400);
        assert_eq!(g.n, 6);
        // Same side: NPU + its HBM.
        assert!(cut.allow_place(&g, TileId(2), Some(BankId(0))).is_ok());
        // Cross chiplet: CPU1 + bank0.
        assert_eq!(
            cut.allow_place(&g, TileId(1), Some(BankId(0))).unwrap_err(),
            CutError::CrossCut
        );
    }

    #[test]
    fn mint_refuses_tight_bound() {
        let g = AffinityGraph::qemu_package();
        let err = SpectralCut::from_mask(CutId(1), &g, 0b000111, 1).unwrap_err();
        assert_eq!(err, CutError::ConductanceExceeded);
    }

    #[test]
    fn min_balanced_is_chiplet_split() {
        let g = AffinityGraph::qemu_package();
        let cut = SpectralCut::min_balanced(CutId(2), &g, 400).unwrap();
        // Fiedler / min-Φ should isolate the weak EMIB, i.e. chiplet halves.
        assert!(cut.left == 0b000111 || cut.right == 0b000111);
    }

    #[test]
    fn fiedler_constructor_is_chiplet_split() {
        let g = AffinityGraph::qemu_package();
        let cut = SpectralCut::from_fiedler(CutId(3), &g, 400).unwrap();
        assert!(cut.left == 0b000111 || cut.right == 0b000111);
        assert!(cut.phi_milli > 0 && cut.phi_milli <= 400);
    }

    #[test]
    fn unbalanced_rejected() {
        let g = AffinityGraph::qemu_package();
        assert_eq!(
            SpectralCut::from_mask(CutId(1), &g, 0b000001, 400).unwrap_err(),
            CutError::Unbalanced
        );
    }

    #[test]
    fn min_balanced_refuses_above_enum_max() {
        let g = AffinityGraph::two_chiplet_mesh(16);
        assert_eq!(g.n, 16);
        assert_eq!(
            SpectralCut::min_balanced(CutId(4), &g, 400).unwrap_err(),
            CutError::TooLarge
        );
    }

    #[test]
    fn fiedler_placement_n16_is_chiplet_split() {
        let g = AffinityGraph::two_chiplet_mesh(16);
        let cut = SpectralCut::from_placement(CutId(16), &g, 400).unwrap();
        let c0 = AffinityGraph::mesh_chiplet0_mask(16);
        assert!(cut.left == c0 || cut.right == c0);
        assert_eq!(cut.left.count_ones(), 8);
        assert!(cut.phi_milli > 0 && cut.phi_milli <= 400);
        let t0 = g.first_tile_on(0).unwrap();
        let t1 = g.first_tile_on(1).unwrap();
        let b0 = g.bank_on(0).unwrap();
        assert!(cut.allow_place(&g, t0, Some(b0)).is_ok());
        assert_eq!(
            cut.allow_place(&g, t1, Some(b0)).unwrap_err(),
            CutError::CrossCut
        );
    }

    #[test]
    fn fiedler_placement_n32_is_chiplet_split() {
        let g = AffinityGraph::two_chiplet_mesh(32);
        assert_eq!(g.n, 32);
        let cut = SpectralCut::from_fiedler(CutId(32), &g, 400).unwrap();
        let c0 = AffinityGraph::mesh_chiplet0_mask(32);
        assert!(cut.left == c0 || cut.right == c0);
        assert_eq!(cut.left.count_ones(), 16);
        assert_eq!(cut.left | cut.right, u32::MAX);
        assert!(cut.phi_milli > 0 && cut.phi_milli <= 400);
        let t0 = g.first_tile_on(0).unwrap();
        let t1 = g.first_tile_on(1).unwrap();
        let b0 = g.bank_on(0).unwrap();
        assert!(cut.allow_place(&g, t0, Some(b0)).is_ok());
        assert_eq!(
            cut.allow_place(&g, t1, Some(b0)).unwrap_err(),
            CutError::CrossCut
        );
    }

    #[test]
    fn bind_right_required_on_n16_mesh() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let g = AffinityGraph::two_chiplet_mesh(16);
        let cut = SpectralCut::from_placement(CutId(16), &g, 400).unwrap();
        let t0 = g.first_tile_on(0).unwrap();
        let b0 = g.bank_on(0).unwrap();
        let p = tab
            .mint(Capability::new(
                CapKind::SpectralCut,
                CapRights(CapRights::READ),
                cut.id.0,
                t,
            ))
            .unwrap();
        assert_eq!(
            bind_place(&tab, p, &cut, &g, t0, Some(b0)).unwrap_err(),
            CutError::NotBound
        );
        let q = tab
            .mint(Capability::new(
                CapKind::SpectralCut,
                CapRights::CUT_FULL,
                cut.id.0,
                t,
            ))
            .unwrap();
        assert!(bind_place(&tab, q, &cut, &g, t0, Some(b0)).is_ok());
        let t1 = g.first_tile_on(1).unwrap();
        assert_eq!(
            bind_place(&tab, q, &cut, &g, t1, Some(b0)).unwrap_err(),
            CutError::CrossCut
        );
    }

    #[test]
    fn bind_right_required() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let (g, cut) = SpectralCut::qemu_chiplet_cut(400).unwrap();
        let p = tab
            .mint(Capability::new(
                CapKind::SpectralCut,
                CapRights(CapRights::READ), // no BIND
                cut.id.0,
                t,
            ))
            .unwrap();
        assert_eq!(
            bind_place(&tab, p, &cut, &g, TileId(2), Some(BankId(0))).unwrap_err(),
            CutError::NotBound
        );
        let q = tab
            .mint(Capability::new(
                CapKind::SpectralCut,
                CapRights::CUT_FULL,
                cut.id.0,
                t,
            ))
            .unwrap();
        assert!(bind_place(&tab, q, &cut, &g, TileId(2), Some(BankId(0))).is_ok());
    }

    #[test]
    fn mesh_nth_tile_and_chiplet_lookup() {
        let g = AffinityGraph::two_chiplet_mesh(16);
        assert_eq!(g.first_tile_on(0), Some(TileId(0)));
        assert_eq!(g.nth_tile_on(0, 1), Some(TileId(1)));
        assert_eq!(g.nth_tile_on(1, 0), Some(TileId(8)));
        assert_eq!(g.nth_tile_on(1, 1), Some(TileId(9)));
        assert_eq!(g.chiplet_of_tile(TileId(0)), Some(0));
        assert_eq!(g.chiplet_of_tile(TileId(8)), Some(1));
        assert_eq!(g.chiplet_of_tile(TileId(99)), None);
        let g32 = AffinityGraph::two_chiplet_mesh(32);
        assert_eq!(g32.nth_tile_on(1, 0), Some(TileId(16)));
        assert_eq!(g32.chiplet_of_tile(TileId(16)), Some(1));
    }

    #[test]
    fn bind_window_refuses_foreign_tenant() {
        use crate::iommu::StreamId;
        use crate::types::{ChipletId, PhysAddr};
        use crate::window::{TypedWindow, WindowKind};

        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let (g, cut) = SpectralCut::qemu_chiplet_cut(400).unwrap();
        let _ = g;
        let q = tab
            .mint(Capability::new(
                CapKind::SpectralCut,
                CapRights::CUT_FULL,
                cut.id.0,
                t,
            ))
            .unwrap();
        let own = TypedWindow::new(
            PhysAddr(0xB000),
            0x1000,
            WindowKind::CxlMemStub,
            StreamId::accel(ChipletId(0), TileId(2), 2),
            t,
        );
        let foreign = TypedWindow::new(
            PhysAddr(0xC000),
            0x1000,
            WindowKind::Hbm,
            StreamId::accel(ChipletId(1), TileId(1), 0),
            TenantId(2),
        );
        assert!(bind_window(&tab, q, &cut, &own, t).is_ok());
        assert_eq!(
            bind_window(&tab, q, &cut, &foreign, t).unwrap_err(),
            CutError::CrossCut
        );
        let empty = CapTable::new(TenantId(2));
        assert_eq!(
            bind_window(&empty, q, &cut, &own, TenantId(2)).unwrap_err(),
            CutError::NotBound
        );
    }
}
