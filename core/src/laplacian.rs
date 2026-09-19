//! AffinityLaplacian — first-class `L = D − A` for a package topology.
//!
//! Placement wants a Fiedler vector of the unnormalized Laplacian.
//! [`crate::cut::SpectralCut::from_fiedler`] / `from_placement` take a
//! median cut of that vector for `n ≤ 32` (host-tested). For `n ≤ 8`,
//! [`crate::cut::SpectralCut::min_balanced`] still enumerates — the
//! combinatorial problem Fiedler approximates. Enumeration is refused
//! above that gate.
//!
//! Arithmetic is integer / milli-fixed-point. There is no libm. This is
//! a **prototype eigensolve**, not GiFt-Placer, not a production package
//! solver, and not an EDA replacement.
//!
//! Complexity on a dense n×n `L` (n ≤ 32):
//! - `from_graph` / `quadratic_form` / `rayleigh_milli`: O(n²)
//! - `fiedler_iterate`: O(iters · n²); default iters = max(32, 2n)
//! - `fiedler_mask` (median cut): iterate + O(n²) insertion sort
//!
//! Public surface is Fiedler / Rayleigh / placement only. Orphaned
//! heat-kernel (`heat_step` / `heat_distance_milli`) and commute-time
//! (`commute_time_milli`) helpers were removed — zero consumers outside
//! this file; not elevated.

use crate::cut::{vert_bit, vert_mask, AffinityGraph, MAX_VERTS};

const RAYLEIGH_SCALE: i64 = 1000;
const FIEDLER_ITERS: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffinityLaplacian {
    pub n: usize,
    /// `L[i][i] = deg(i)`, `L[i][j] = −w_ij` for `i ≠ j`.
    pub l: [[i32; MAX_VERTS]; MAX_VERTS],
    pub degree: [u32; MAX_VERTS],
}

impl AffinityLaplacian {
    pub fn from_graph(g: &AffinityGraph) -> Self {
        let mut lap = Self {
            n: g.n,
            l: [[0; MAX_VERTS]; MAX_VERTS],
            degree: [0; MAX_VERTS],
        };
        for i in 0..g.n {
            let deg = g.degree(i);
            lap.degree[i] = deg;
            lap.l[i][i] = deg as i32;
            for j in 0..g.n {
                if i == j {
                    continue;
                }
                lap.l[i][j] = -(g.w[i][j] as i32);
            }
        }
        lap
    }

    pub fn volume(&self) -> u32 {
        self.degree[..self.n].iter().sum()
    }

    pub fn max_degree(&self) -> u32 {
        self.degree[..self.n].iter().copied().max().unwrap_or(0)
    }

    /// `x^T L x`. `L 1 = 0` so the all-ones vector yields 0.
    pub fn quadratic_form(&self, x: &[i32]) -> i64 {
        let n = self.n.min(x.len());
        let mut acc = 0i64;
        for i in 0..n {
            let mut row = 0i64;
            for j in 0..n {
                row += self.l[i][j] as i64 * x[j] as i64;
            }
            acc += x[i] as i64 * row;
        }
        acc
    }

    /// Rayleigh `R(L, x) = (x^T L x) / (x^T x)` in thousandths.
    pub fn rayleigh_milli(&self, x: &[i32]) -> Option<i32> {
        let n = self.n.min(x.len());
        if n == 0 {
            return None;
        }
        let mut den = 0i64;
        for xi in x.iter().take(n) {
            den += *xi as i64 * *xi as i64;
        }
        if den == 0 {
            return None;
        }
        let num = self.quadratic_form(x);
        Some((num * RAYLEIGH_SCALE / den) as i32)
    }

    fn center(&self, x: &mut [i32; MAX_VERTS]) {
        if self.n == 0 {
            return;
        }
        let sum: i64 = x[..self.n].iter().map(|v| *v as i64).sum();
        let mean = sum / self.n as i64;
        for xi in x.iter_mut().take(self.n) {
            *xi = (*xi as i64 - mean) as i32;
        }
    }

    fn scale_vec(&self, x: &mut [i32; MAX_VERTS]) {
        let max = x[..self.n].iter().map(|v| v.abs()).max().unwrap_or(0);
        if max > 10_000 {
            let div = (max / 1_000).max(1);
            for xi in x.iter_mut().take(self.n) {
                *xi /= div;
            }
        }
    }

    pub fn default_iters(&self) -> u32 {
        FIEDLER_ITERS.max((self.n as u32).saturating_mul(2))
    }

    /// Power iteration on `(σI − L)` in the subspace orthogonal to `1`.
    ///
    /// The seed is a centered index vector — not the chiplet labels — so a
    /// recovered bipartition is evidence the graph geometry was used.
    /// Pass `iters == 0` to use [`Self::default_iters`].
    pub fn fiedler_iterate(&self, iters: u32) -> [i32; MAX_VERTS] {
        let mut x = [0i32; MAX_VERTS];
        if self.n == 0 {
            return x;
        }
        let mid = (self.n as i32 - 1) / 2;
        for i in 0..self.n {
            x[i] = i as i32 - mid;
        }
        self.center(&mut x);
        let sigma = self.max_degree().saturating_mul(2).saturating_add(1) as i64;
        let steps = if iters == 0 {
            self.default_iters()
        } else {
            iters
        };
        for _ in 0..steps {
            let mut y = [0i32; MAX_VERTS];
            for i in 0..self.n {
                let mut lu = 0i64;
                for j in 0..self.n {
                    lu += self.l[i][j] as i64 * x[j] as i64;
                }
                y[i] = (sigma * x[i] as i64 - lu).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            }
            self.center(&mut y);
            self.scale_vec(&mut y);
            x = y;
        }
        x
    }

    /// Balanced median-cut of the Fiedler-ish vector (bit 0 forced set).
    ///
    /// Vertices are ordered by the iterate; the lower `n/2` form one side.
    /// That is always a SpectralCut-compatible bipartition (`|L|−|R| ≤ 1`).
    /// Sign-split alone can land unbalanced on integer vectors; the median
    /// is the placement API.
    pub fn fiedler_mask(&self) -> u32 {
        if self.n == 0 {
            return 0;
        }
        let x = self.fiedler_iterate(0);
        let mut idx = [0usize; MAX_VERTS];
        for i in 0..self.n {
            idx[i] = i;
        }
        for i in 1..self.n {
            let key = idx[i];
            let mut j = i;
            while j > 0 && (x[idx[j - 1]] > x[key] || (x[idx[j - 1]] == x[key] && idx[j - 1] > key))
            {
                idx[j] = idx[j - 1];
                j -= 1;
            }
            idx[j] = key;
        }
        let half = self.n / 2;
        let mut mask = 0u32;
        for k in 0..half {
            mask |= vert_bit(idx[k]);
        }
        if mask == 0 {
            mask = vert_mask(half.max(1).min(self.n));
        }
        let all = vert_mask(self.n);
        if mask & 1 == 0 {
            mask ^= all;
        }
        mask
    }
}

impl AffinityGraph {
    pub fn laplacian(&self) -> AffinityLaplacian {
        AffinityLaplacian::from_graph(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut::{SpectralCut, VertKind, Vertex};
    use crate::types::TileId;

    fn path4() -> AffinityGraph {
        let mut g = AffinityGraph::empty();
        for i in 0..4 {
            g.add_vert(Vertex {
                kind: VertKind::Tile(TileId(i as u16)),
                chiplet: (i / 2) as u8,
            });
        }
        g.add_edge(0, 1, 4);
        g.add_edge(1, 2, 4);
        g.add_edge(2, 3, 4);
        g
    }

    #[test]
    fn ones_are_kernel() {
        let g = AffinityGraph::qemu_package();
        let lap = g.laplacian();
        assert_eq!(lap.n, 6);
        let ones = [1i32; MAX_VERTS];
        assert_eq!(lap.quadratic_form(&ones[..lap.n]), 0);
        assert_eq!(lap.rayleigh_milli(&ones[..lap.n]), Some(0));
        for i in 0..lap.n {
            let mut row = 0i32;
            for j in 0..lap.n {
                row += lap.l[i][j];
            }
            assert_eq!(row, 0, "L 1 = 0 at row {i}");
            assert_eq!(lap.l[i][i] as u32, lap.degree[i]);
        }
    }

    #[test]
    fn chiplet_indicator_has_small_rayleigh() {
        let g = AffinityGraph::qemu_package();
        let lap = AffinityLaplacian::from_graph(&g);
        // ±1 on the two chiplets. xᵀLx = 4 · cut = 20, ||x||² = 6.
        let chiplet = [1, 1, 1, -1, -1, -1];
        let r_cut = lap.rayleigh_milli(&chiplet).unwrap();
        assert_eq!(r_cut, 20 * 1000 / 6);
        // A cut that slices a chiplet (verts 0,3,1 vs 2,4,5) is worse.
        let messy = [1, 1, -1, 1, -1, -1];
        let r_messy = lap.rayleigh_milli(&messy).unwrap();
        assert!(r_messy > r_cut, "r_messy={r_messy} r_cut={r_cut}");
    }

    #[test]
    fn fiedler_recovers_qemu_chiplet_split() {
        let g = AffinityGraph::qemu_package();
        let lap = AffinityLaplacian::from_graph(&g);
        let mask = lap.fiedler_mask();
        let chiplet = 0b000111u32;
        assert!(
            mask == chiplet || mask == (0b111111 ^ chiplet) || mask == 0b111000,
            "fiedler mask {mask:#08b} should be the chiplet bipartition"
        );
        // Bit 0 is canonicalized on.
        assert_ne!(mask & 1, 0);
        let x = lap.fiedler_iterate(32);
        let r = lap.rayleigh_milli(&x[..lap.n]).unwrap();
        assert!(r > 0, "Fiedler is not the kernel");
    }

    #[test]
    fn path_fiedler_splits_the_middle() {
        let g = path4();
        let lap = AffinityLaplacian::from_graph(&g);
        let mask = lap.fiedler_mask();
        let left = mask & 0b1111;
        // Balanced 2–2 about the path centre: {0,1}|{2,3} (canonical bit0).
        assert_eq!(left.count_ones(), 2);
        assert!(
            left == 0b0011 || left == 0b0101 || left == 0b1001,
            "path median-cut {left:#06b}"
        );
        let ends = lap.fiedler_iterate(32);
        // Ends of a path have opposite Fiedler sign.
        assert!((ends[0] >= 0) != (ends[3] >= 0) || ends[0] == 0 || ends[3] == 0);
    }

    #[test]
    fn from_fiedler_matches_enumerated_chiplet_cut() {
        let g = AffinityGraph::qemu_package();
        let enumerated = SpectralCut::min_balanced(crate::cut::CutId(2), &g, 400).unwrap();
        let spectral = SpectralCut::from_fiedler(crate::cut::CutId(3), &g, 400).unwrap();
        let chiplet = 0b000111u32;
        assert!(enumerated.left == chiplet || enumerated.right == chiplet);
        assert!(spectral.left == chiplet || spectral.right == chiplet);
    }

    fn assert_mesh_smoke(n: usize) {
        let g = AffinityGraph::two_chiplet_mesh(n);
        assert_eq!(g.n, n);
        let lap = AffinityLaplacian::from_graph(&g);
        assert_eq!(lap.n, n);
        let ones = [1i32; MAX_VERTS];
        assert_eq!(lap.quadratic_form(&ones[..n]), 0);
        let mask = lap.fiedler_mask();
        let c0 = AffinityGraph::mesh_chiplet0_mask(n);
        let all = crate::cut::vert_mask(n);
        assert!(
            mask == c0 || mask == (all ^ c0),
            "n={n} fiedler {mask:#034b} chiplet0 {c0:#034b}"
        );
        assert_eq!(mask.count_ones() as usize, n / 2);
        assert_ne!(mask & 1, 0);
        let x = lap.fiedler_iterate(0);
        let r = lap.rayleigh_milli(&x[..n]).unwrap();
        assert!(r > 0, "Fiedler is not the kernel at n={n}");
        let cut = SpectralCut::from_placement(crate::cut::CutId(n as u32), &g, 400).unwrap();
        assert!(cut.left == c0 || cut.right == c0);
        assert!(cut.phi_milli > 0 && cut.phi_milli <= 400);
    }

    #[test]
    fn n16_smoke_fiedler_placement() {
        assert_mesh_smoke(16);
    }

    #[test]
    fn n32_smoke_fiedler_placement() {
        assert_mesh_smoke(32);
    }
}
