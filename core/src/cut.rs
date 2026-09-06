//! ChipletSpectralCut — a first-class partition of the package graph.
//!
//! A `SpectralCut` is a (nearly) balanced bipartition of the chiplet
//! interconnect + memory-affinity graph, minted as a `CapKind::SpectralCut`.
//! Tasks bind via `CapRights::BIND`. The scheduler refuses a placement
//! whose tile and bank sit on opposite sides of the bound cut.
//!
//! Intended construction (large n): Fiedler vector of the unnormalized
//! Laplacian `L = D − A` (see [`crate::laplacian::AffinityLaplacian`]).
//! For n ≤ 8, [`SpectralCut::min_balanced`] still enumerates — the
//! combinatorial problem Fiedler approximates. [`SpectralCut::from_fiedler`]
//! is wired as the optional constructor. The QEMU topology is two chiplets
//! with weak inter-die edges; the min-conductance split is the chiplet cut.

use crate::caps::{CapError, CapKind, CapRights, CapTable, CPtr};
use crate::laplacian::AffinityLaplacian;
use crate::types::{BankId, TileId};

pub const MAX_VERTS: usize = 8;
pub const MAX_CUTS_SCHED: usize = 4;

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
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| self.degree(i))
            .sum()
    }

    pub fn cut_weight(&self, mask: u32) -> u32 {
        let mut c = 0u32;
        for i in 0..self.n {
            if mask & (1 << i) == 0 {
                continue;
            }
            for j in 0..self.n {
                if mask & (1 << j) == 0 {
                    c += self.w[i][j] as u32;
                }
            }
        }
        c
    }

    /// Φ(S) = cut(S,V\S) / min(vol S, vol V\S), in thousandths.
    pub fn conductance_milli(&self, mask: u32) -> Option<u32> {
        let all = (1u32 << self.n) - 1;
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
        let all = (1u32 << g.n) - 1;
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
        let phi = g
            .conductance_milli(left)
            .ok_or(CutError::EmptyPart)?;
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

    /// Sign-split of [`AffinityLaplacian::fiedler_mask`]. Used as the
    /// intended large-n constructor; for n ≤ 8 prefer [`Self::min_balanced`].
    pub fn from_fiedler(
        id: CutId,
        g: &AffinityGraph,
        bound_milli: u32,
    ) -> Result<Self, CutError> {
        let lap = AffinityLaplacian::from_graph(g);
        Self::from_mask(id, g, lap.fiedler_mask(), bound_milli)
    }

    /// Enumerate balanced masks; pick minimum conductance. This is what a
    /// Fiedler sweep approximates when n is large.
    pub fn min_balanced(
        id: CutId,
        g: &AffinityGraph,
        bound_milli: u32,
    ) -> Result<Self, CutError> {
        if g.n == 0 || g.n > MAX_VERTS {
            return Err(CutError::EmptyPart);
        }
        let all = (1u32 << g.n) - 1;
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
        let bit = 1u32 << vi;
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
    fn bind_right_required() {
        let t = TenantId(1);
        let mut tab = CapTable::new(t);
        let (g, cut) = SpectralCut::qemu_chiplet_cut(400).unwrap();
        let p = tab
            .mint(Capability {
                kind: CapKind::SpectralCut,
                rights: CapRights(CapRights::READ), // no BIND
                object: cut.id.0,
                badge: 0,
                generation: 0,
                tenant: t,
            })
            .unwrap();
        assert_eq!(
            bind_place(&tab, p, &cut, &g, TileId(2), Some(BankId(0))).unwrap_err(),
            CutError::NotBound
        );
        let q = tab
            .mint(Capability {
                kind: CapKind::SpectralCut,
                rights: CapRights::CUT_FULL,
                object: cut.id.0,
                badge: 0,
                generation: 0,
                tenant: t,
            })
            .unwrap();
        assert!(bind_place(&tab, q, &cut, &g, TileId(2), Some(BankId(0))).is_ok());
    }
}
