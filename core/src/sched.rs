//! Tile scheduler: CPU threads and accelerator waves as peer jobs.
//!
//! Traditional kernels enqueue GPU work as a device ioctl and hope. Aether
//! places both `Thread` and `AccelWave` on the same fabric scheduler so
//! priority, deadlines, bank affinity, and work-stealing apply uniformly.
//!
//! Chiplet-local work uses [`ChipletTaskScope`]: pick/steal prefer (Soft) or
//! require (Strict) the job's chiplet. Fleet's Chiplet-task is inspiration
//! only — not a Fleet runtime, not an L2-coherence claim, and not a
//! reproduction of unpublished Fleet numbers.

use crate::color::{admit_wave, BankColor, ColorError};
use crate::cut::{vert_bit, AffinityGraph, CutError, CutId, SpectralCut, MAX_CUTS_SCHED};
use crate::laplacian::AffinityLaplacian;
use crate::partition::{PartitionError, PartitionProfile};
use crate::phase::Phase;
use crate::types::{BankId, ChipletId, TileId, MAX_TILES};

/// Soft same-chiplet score when [`Job::chiplet_scope`] matches the tile.
const CHIPLET_SCOPE_BONUS: i32 = 40;

pub const MAX_JOBS: usize = 32;
pub const N_PRIO: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    Cpu,
    Npu,
    Gpu,
    Asic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Thread,
    AccelWave,
}

/// A job named for one chiplet's tiles (Fleet Chiplet-task *inspiration*).
///
/// Pick and steal consult this against the bound affinity graph. A bound
/// [`SpectralCut`] still refuses [`CutError::CrossCut`] independently.
/// This is not a Fleet runtime and not an L2-coherence scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChipletTaskScope {
    pub chiplet: ChipletId,
}

impl ChipletTaskScope {
    pub const fn new(chiplet: ChipletId) -> Self {
        Self { chiplet }
    }
}

/// How pick/steal treat [`Job::chiplet_scope`].
///
/// Unscoped jobs are unrestricted under both modes. Strict is the default:
/// a Chiplet-task stays on its die. Soft is the optional preference-only
/// path (same-chiplet score + steal local-first, still allowed to cross).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipletLocalPolicy {
    /// Score hint only. Steal may still cross chiplets (local pass first).
    Soft,
    /// Scoped jobs run only on that chiplet. Unscoped jobs are unrestricted.
    Strict,
}

impl Default for ChipletLocalPolicy {
    fn default() -> Self {
        Self::Strict
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: u32,
    pub kind: JobKind,
    pub tile_hint: Option<TileId>,
    pub bank_affinity: Option<BankId>,
    /// 0 = highest priority.
    pub priority: u8,
    pub deadline_ticks: Option<u64>,
    pub tenant: u32,
    /// Bound `SpectralCut` object id. `None` = unrestricted (legacy jobs).
    pub cut_id: Option<u32>,
    /// Named phase tag. The kernel admits the tag; it does not fuse phases.
    pub phase: Phase,
    /// Bound partition profile. `None` = unrestricted (legacy jobs).
    pub partition_id: Option<u32>,
    /// Fence / timeline id for submit → complete ordering.
    pub fence_id: Option<u64>,
    /// Arena tenant/bank paint. `None` = unrestricted (legacy jobs).
    pub arena_color: Option<BankColor>,
    /// Chiplet-task scope. `None` = unrestricted (legacy jobs).
    pub chiplet_scope: Option<ChipletTaskScope>,
}

impl Job {
    pub fn compatible(kind: JobKind, tile: TileKind) -> bool {
        match (kind, tile) {
            (JobKind::Thread, TileKind::Cpu) => true,
            (JobKind::AccelWave, TileKind::Npu | TileKind::Gpu | TileKind::Asic) => true,
            _ => false,
        }
    }

    /// Effective priority: deadline-imminent jobs sort ahead of their base prio.
    pub fn effective_prio(&self, now: u64) -> u8 {
        if let Some(dl) = self.deadline_ticks {
            if dl <= now {
                return 0;
            }
            let slack = dl - now;
            if slack < 10 {
                return self.priority.min(1);
            }
        }
        self.priority.min((N_PRIO - 1) as u8)
    }

    /// Bank-color gate used by pick and by accel submit.
    pub fn color_ok(&self, tile_home: BankId) -> Result<(), ColorError> {
        if self.kind != JobKind::AccelWave {
            return Ok(());
        }
        if self.arena_color.is_none() {
            return Ok(());
        }
        admit_wave(self.tenant, self.phase, self.arena_color, tile_home)
    }
}

#[derive(Clone, Copy, Debug)]
struct Tile {
    id: TileId,
    kind: TileKind,
    home_bank: BankId,
}

#[derive(Clone, Debug)]
pub struct TileScheduler {
    tiles: [Option<Tile>; MAX_TILES],
    /// Ready jobs. Not a heap — scanned; N is tiny and this stays obvious.
    ready: [Option<Job>; MAX_JOBS],
    next_id: u32,
    steal_cursor: usize,
    now: u64,
    graph: AffinityGraph,
    /// Cached Fiedler median-cut of `graph`. 0 if the graph is empty.
    fiedler_mask: u32,
    cuts: [Option<SpectralCut>; MAX_CUTS_SCHED],
    partition: Option<PartitionProfile>,
    chiplet_policy: ChipletLocalPolicy,
}

impl TileScheduler {
    pub const fn new() -> Self {
        Self {
            tiles: [None; MAX_TILES],
            ready: [None; MAX_JOBS],
            next_id: 1,
            steal_cursor: 0,
            now: 0,
            graph: AffinityGraph::empty(),
            fiedler_mask: 0,
            cuts: [None; MAX_CUTS_SCHED],
            partition: None,
            chiplet_policy: ChipletLocalPolicy::Strict,
        }
    }

    pub fn bind_partition(&mut self, p: PartitionProfile) {
        self.partition = Some(p);
    }

    pub fn set_chiplet_policy(&mut self, policy: ChipletLocalPolicy) {
        self.chiplet_policy = policy;
    }

    pub fn chiplet_policy(&self) -> ChipletLocalPolicy {
        self.chiplet_policy
    }

    pub fn set_graph(&mut self, g: AffinityGraph) {
        self.fiedler_mask = if g.n >= 2 {
            AffinityLaplacian::from_graph(&g).fiedler_mask()
        } else {
            0
        };
        self.graph = g;
    }

    /// Install a SpectralCut built from the cached AffinityLaplacian
    /// placement of the bound graph. This is the n≤32 path — not
    /// enumeration. BIND is still required on the cap surface
    /// ([`crate::cut::bind_place`]); this only mints the cut object
    /// the scheduler scores against.
    pub fn bind_laplacian_cut(
        &mut self,
        id: CutId,
        bound_milli: u32,
    ) -> Result<SpectralCut, CutError> {
        if self.graph.n < 2 {
            return Err(CutError::EmptyPart);
        }
        let cut = SpectralCut::from_placement(id, &self.graph, bound_milli)?;
        if !self.install_cut(cut) {
            return Err(CutError::NoCut);
        }
        Ok(cut)
    }

    pub fn fiedler_mask(&self) -> u32 {
        self.fiedler_mask
    }

    pub fn install_cut(&mut self, cut: SpectralCut) -> bool {
        if let Some(slot) = self.cuts.iter_mut().find(|c| c.is_none()) {
            *slot = Some(cut);
            true
        } else {
            false
        }
    }

    fn cut(&self, id: u32) -> Option<&SpectralCut> {
        self.cuts.iter().flatten().find(|c| c.id.0 == id)
    }

    /// Placement gate used by pick/steal. Unbound cut/partition always pass.
    pub fn place_ok(&self, job: &Job, tile: TileId) -> Result<(), CutError> {
        if let Some(cid) = job.cut_id {
            let cut = self.cut(cid).ok_or(CutError::NoCut)?;
            cut.allow_place(&self.graph, tile, job.bank_affinity)
                .map(|_| ())?;
        }
        self.partition_ok(job, tile)
            .map_err(|_| CutError::CrossCut)?;
        job.color_ok(self.tile(tile).map(|t| t.home_bank).unwrap_or(BankId(0)))
            .map_err(|_| CutError::CrossCut)
    }

    fn partition_ok(&self, job: &Job, tile: TileId) -> Result<(), PartitionError> {
        let Some(pid) = job.partition_id else {
            return Ok(());
        };
        let p = self.partition.ok_or(PartitionError::Unbound)?;
        if p.id.0 != pid {
            return Err(PartitionError::Unbound);
        }
        p.admit_place(tile, job.bank_affinity)
    }

    pub fn add_tile(&mut self, id: TileId, kind: TileKind, home_bank: BankId) -> bool {
        if self.tiles.iter().flatten().any(|t| t.id == id) {
            return false;
        }
        if let Some(slot) = self.tiles.iter_mut().find(|t| t.is_none()) {
            *slot = Some(Tile {
                id,
                kind,
                home_bank,
            });
            true
        } else {
            false
        }
    }

    pub fn tick(&mut self, t: u64) {
        self.now = t;
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn ready_count(&self) -> usize {
        self.ready.iter().filter(|j| j.is_some()).count()
    }

    pub fn enqueue(&mut self, mut job: Job) -> Option<u32> {
        if job.priority as usize >= N_PRIO {
            job.priority = (N_PRIO - 1) as u8;
        }
        if job.id == 0 {
            job.id = self.next_id;
            self.next_id += 1;
        }
        let id = job.id;
        if let Some(slot) = self.ready.iter_mut().find(|s| s.is_none()) {
            *slot = Some(job);
            Some(id)
        } else {
            None
        }
    }

    fn tile(&self, id: TileId) -> Option<&Tile> {
        self.tiles.iter().flatten().find(|t| t.id == id)
    }

    fn score(&self, job: &Job, tile: &Tile) -> i32 {
        if !Job::compatible(job.kind, tile.kind) {
            return i32::MIN;
        }
        if self.place_ok(job, tile.id).is_err() {
            return i32::MIN;
        }
        if !self.scope_ok(job, tile.id) {
            return i32::MIN;
        }
        let mut s = 1000 - job.effective_prio(self.now) as i32 * 100;
        if job.tile_hint == Some(tile.id) {
            s += 50;
        }
        if job.bank_affinity == Some(tile.home_bank) {
            s += 25;
        }
        s += self.laplacian_bonus(job, tile);
        s += self.chiplet_scope_bonus(job, tile);
        if let Some(dl) = job.deadline_ticks {
            if dl <= self.now {
                s += 80;
            }
        }
        s
    }

    /// Soft Fiedler-side hint: same-side tile/bank of the cached
    /// laplacian placement. Refuse stays on the bound cut + BIND path.
    fn laplacian_bonus(&self, job: &Job, tile: &Tile) -> i32 {
        let Some(bank) = job.bank_affinity else {
            return 0;
        };
        if self.graph.n < 2 || self.fiedler_mask == 0 {
            return 0;
        }
        let Some(ti) = self.graph.find_tile(tile.id) else {
            return 0;
        };
        let Some(bi) = self.graph.find_bank(bank) else {
            return 0;
        };
        let t_left = self.fiedler_mask & vert_bit(ti) != 0;
        let b_left = self.fiedler_mask & vert_bit(bi) != 0;
        if t_left == b_left {
            15
        } else {
            0
        }
    }

    fn chiplet_of(&self, tile: TileId) -> Option<ChipletId> {
        self.graph.chiplet_of_tile(tile).map(ChipletId)
    }

    fn same_chiplet(&self, a: TileId, b: TileId) -> Option<bool> {
        match (self.chiplet_of(a), self.chiplet_of(b)) {
            (Some(x), Some(y)) => Some(x == y),
            _ => None,
        }
    }

    /// Without a bound graph, scope cannot be enforced (fail-open).
    fn scope_ok(&self, job: &Job, tile: TileId) -> bool {
        let Some(scope) = job.chiplet_scope else {
            return true;
        };
        match self.chiplet_of(tile) {
            None => true,
            Some(c) => match self.chiplet_policy {
                ChipletLocalPolicy::Soft => true,
                ChipletLocalPolicy::Strict => c == scope.chiplet,
            },
        }
    }

    /// Eligible in the chiplet-local steal pass: unscoped, unknown topology,
    /// or a scope that matches the thief.
    fn job_local_to(&self, job: &Job, thief: TileId) -> bool {
        match (job.chiplet_scope, self.chiplet_of(thief)) {
            (None, _) => true,
            (Some(_), None) => true,
            (Some(scope), Some(c)) => scope.chiplet == c,
        }
    }

    fn chiplet_scope_bonus(&self, job: &Job, tile: &Tile) -> i32 {
        if !self.scope_ok(job, tile.id) {
            return 0;
        }
        let Some(scope) = job.chiplet_scope else {
            return 0;
        };
        match self.chiplet_of(tile.id) {
            Some(c) if c == scope.chiplet => CHIPLET_SCOPE_BONUS,
            _ => 0,
        }
    }

    /// Pick the best ready job for `tile`.
    pub fn pick(&mut self, tile: TileId) -> Option<Job> {
        let t = *self.tile(tile)?;
        let mut best: Option<(usize, i32)> = None;
        for (i, job) in self.ready.iter().enumerate() {
            let Some(job) = job else { continue };
            let s = self.score(job, &t);
            if s == i32::MIN {
                continue;
            }
            match best {
                None => best = Some((i, s)),
                Some((_, bs)) if s > bs => best = Some((i, s)),
                _ => {}
            }
        }
        best.and_then(|(i, _)| self.ready[i].take())
    }

    /// Work-steal: take a compatible job that is *not* pinned to another tile
    /// and is not higher-priority than what the victim would keep.
    ///
    /// Job pass 1: Chiplet-task-local / unscoped work. Job pass 2: remote
    /// scoped work (Soft only — Strict already scores it `i32::MIN`).
    /// Inside a pass, same-chiplet victims are visited first. Classic WS
    /// still takes the *lowest* scoring job inside those filters.
    pub fn steal(&mut self, thief: TileId) -> Option<Job> {
        let t = *self.tile(thief)?;
        let n = MAX_TILES;
        for job_local_pass in [true, false] {
            for victim_local_pass in [true, false] {
                for k in 0..n {
                    let idx = (self.steal_cursor + k) % n;
                    let Some(victim) = self.tiles[idx] else {
                        continue;
                    };
                    if victim.id == thief {
                        continue;
                    }
                    if victim.kind != t.kind {
                        continue;
                    }
                    match self.same_chiplet(thief, victim.id) {
                        None => {
                            if !victim_local_pass {
                                continue;
                            }
                        }
                        Some(same) => {
                            if same != victim_local_pass {
                                continue;
                            }
                        }
                    }
                    let mut worst: Option<(usize, i32)> = None;
                    for (i, job) in self.ready.iter().enumerate() {
                        let Some(job) = job else { continue };
                        if job.tile_hint == Some(victim.id) {
                            continue; // hard affinity — do not steal
                        }
                        if self.job_local_to(job, thief) != job_local_pass {
                            continue;
                        }
                        let s = self.score(job, &t);
                        if s == i32::MIN {
                            continue;
                        }
                        match worst {
                            None => worst = Some((i, s)),
                            Some((_, ws)) if s < ws => worst = Some((i, s)),
                            _ => {}
                        }
                    }
                    if let Some((i, _)) = worst {
                        self.steal_cursor = (idx + 1) % n;
                        return self.ready[i].take();
                    }
                }
            }
        }
        self.steal_cursor = (self.steal_cursor + 1) % n;
        None
    }

    pub fn pick_or_steal(&mut self, tile: TileId) -> Option<Job> {
        self.pick(tile).or_else(|| self.steal(tile))
    }

    /// Lockstep stand-in for two CPU harts sharing one ready pool.
    /// Returns `(jobs_on_a, jobs_on_b)`.
    pub fn drive_two_cpu_tiles(&mut self, a: TileId, b: TileId) -> (usize, usize) {
        let mut na = 0usize;
        let mut nb = 0usize;
        loop {
            let ja = self.pick_or_steal(a);
            let jb = self.pick_or_steal(b);
            if ja.is_none() && jb.is_none() {
                break;
            }
            if ja.is_some() {
                na += 1;
            }
            if jb.is_some() {
                nb += 1;
            }
        }
        (na, nb)
    }
}

impl Default for TileScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> TileScheduler {
        let mut s = TileScheduler::new();
        s.add_tile(TileId(0), TileKind::Cpu, BankId(0));
        s.add_tile(TileId(1), TileKind::Cpu, BankId(1));
        s.add_tile(TileId(2), TileKind::Npu, BankId(0));
        s
    }

    #[test]
    fn cpu_does_not_run_waves() {
        let mut s = setup();
        s.enqueue(Job {
            id: 0,
            kind: JobKind::AccelWave,
            tile_hint: None,
            bank_affinity: None,
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        assert!(s.pick(TileId(0)).is_none());
        assert!(s.pick(TileId(2)).is_some());
    }

    #[test]
    fn higher_priority_wins() {
        let mut s = setup();
        s.enqueue(Job {
            id: 0,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: None,
            priority: 5,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        s.enqueue(Job {
            id: 0,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: None,
            priority: 1,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        let j = s.pick(TileId(0)).unwrap();
        assert_eq!(j.priority, 1);
    }

    #[test]
    fn deadline_boost() {
        let mut s = setup();
        s.tick(100);
        s.enqueue(Job {
            id: 0,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: None,
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        s.enqueue(Job {
            id: 0,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: None,
            priority: 7,
            deadline_ticks: Some(100),
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        let j = s.pick(TileId(0)).unwrap();
        assert_eq!(j.deadline_ticks, Some(100));
    }

    #[test]
    fn bank_affinity_tiebreak() {
        let mut s = setup();
        s.enqueue(Job {
            id: 10,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: Some(BankId(1)),
            priority: 3,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        s.enqueue(Job {
            id: 11,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: Some(BankId(0)),
            priority: 3,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        let j = s.pick(TileId(0)).unwrap();
        assert_eq!(j.id, 11);
    }

    #[test]
    fn two_harts_drive_pick_or_steal() {
        let mut s = setup();
        for id in 1..=6 {
            s.enqueue(Job {
                id,
                kind: JobKind::Thread,
                tile_hint: None,
                bank_affinity: None,
                priority: 4,
                deadline_ticks: None,
                tenant: 1,
                cut_id: None,
                phase: Phase::Compute,
                partition_id: None,
                fence_id: None,
                arena_color: None,
                chiplet_scope: None,
            });
        }
        let (c0, c1) = s.drive_two_cpu_tiles(TileId(0), TileId(1));
        assert!(
            c0 >= 1 && c1 >= 1,
            "both CPU tiles must run work, got {c0}+{c1}"
        );
        assert_eq!(c0 + c1, 6);
        assert_eq!(s.ready_count(), 0);
    }

    #[test]
    fn steal_across_cpu_tiles() {
        let mut s = setup();
        s.enqueue(Job {
            id: 1,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: None,
            priority: 4,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        // tile 1 has nothing local; steals from the ready pool
        let j = s.steal(TileId(1)).unwrap();
        assert_eq!(j.kind, JobKind::Thread);
    }

    #[test]
    fn no_steal_hard_affinity() {
        let mut s = setup();
        s.enqueue(Job {
            id: 1,
            kind: JobKind::Thread,
            tile_hint: Some(TileId(0)),
            bank_affinity: None,
            priority: 4,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        assert!(s.steal(TileId(1)).is_none());
        assert!(s.pick(TileId(0)).is_some());
    }

    #[test]
    fn bound_cut_rejects_cross_chiplet_tile() {
        let mut s = setup();
        let (g, cut) = crate::cut::SpectralCut::qemu_chiplet_cut(400).unwrap();
        s.set_graph(g);
        s.install_cut(cut);
        s.enqueue(Job {
            id: 7,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: Some(BankId(0)),
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: Some(cut.id.0),
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        // tile 1 is chiplet 1; bank 0 is chiplet 0 → CrossCut
        assert!(s.pick(TileId(1)).is_none());
        // tile 0 + bank 0 is legal
        assert!(s.pick(TileId(0)).is_some());
    }

    #[test]
    fn bound_partition_rejects_foreign_tile() {
        let mut s = setup();
        let p = crate::partition::PartitionProfile::new(
            crate::partition::PartitionId(1),
            crate::partition::SpatialSlice::single_chiplet(crate::types::ChipletId(0), 1 << 0, 0b1),
            crate::partition::QosBudget {
                bw_mbps: 100,
                credits: 2,
            },
            crate::partition::BlastRadius {
                max_nodes: 2,
                max_hops: 1,
            },
        );
        s.bind_partition(p);
        s.enqueue(Job {
            id: 3,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: Some(BankId(0)),
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: Some(1),
            fence_id: Some(1),
            arena_color: None,
            chiplet_scope: None,
        });
        // tile 1 is not in the slice mask
        assert!(s.pick(TileId(1)).is_none());
        assert!(s.pick(TileId(0)).is_some());
    }

    #[test]
    fn npu_cannot_steal_cpu_thread() {
        let mut s = setup();
        s.enqueue(Job {
            id: 1,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: None,
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        });
        assert!(s.steal(TileId(2)).is_none());
    }

    #[test]
    fn foreign_bank_wave_refused_without_transfer() {
        let mut s = setup();
        s.enqueue(Job {
            id: 20,
            kind: JobKind::AccelWave,
            tile_hint: Some(TileId(2)),
            bank_affinity: Some(BankId(1)),
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: Some(BankColor::new(crate::types::TenantId(1), BankId(1))),
            chiplet_scope: None,
        });
        // NPU tile 2 lives on bank 0; arena color is bank 1.
        assert!(s.pick(TileId(2)).is_none());
        assert_eq!(s.ready_count(), 1);
    }

    #[test]
    fn exchange_phase_allows_foreign_bank() {
        let mut s = setup();
        s.enqueue(Job {
            id: 21,
            kind: JobKind::AccelWave,
            tile_hint: Some(TileId(2)),
            bank_affinity: Some(BankId(1)),
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Exchange,
            partition_id: None,
            fence_id: None,
            arena_color: Some(BankColor::new(crate::types::TenantId(1), BankId(1))),
            chiplet_scope: None,
        });
        let j = s.pick(TileId(2)).unwrap();
        assert_eq!(j.id, 21);
        assert_eq!(j.phase, Phase::Exchange);
    }

    #[test]
    fn transfer_recolors_and_compute_is_admitted() {
        use crate::arena::{ArenaAllocator, ArenaRequest};
        use crate::types::PhysAddr;

        let mut arenas =
            ArenaAllocator::new(&[(BankId(0), PhysAddr(0x0100_0000), 8 * 1024 * 1024)]).unwrap();
        let ar = arenas
            .alloc(
                ArenaRequest::tensor(4096, Some(BankId(0))).for_tenant(crate::types::TenantId(2)),
            )
            .unwrap();
        assert_eq!(
            crate::color::admit_arena_wave(1, Phase::Compute, &ar, BankId(0)).unwrap_err(),
            crate::color::ColorError::ForeignTenant
        );
        arenas.transfer_owner(ar.id, None, 2, 1).unwrap();
        let painted = *arenas.get(ar.id).unwrap();
        crate::color::admit_arena_wave(1, Phase::Compute, &painted, BankId(0)).unwrap();

        let mut s = setup();
        s.enqueue(Job {
            id: 22,
            kind: JobKind::AccelWave,
            tile_hint: Some(TileId(2)),
            bank_affinity: Some(BankId(0)),
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: None,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: Some(painted.color),
            chiplet_scope: None,
        });
        assert_eq!(s.pick(TileId(2)).unwrap().id, 22);
    }

    fn mesh_sched(n: usize) -> (TileScheduler, crate::cut::SpectralCut, TileId, TileId) {
        let g = crate::cut::AffinityGraph::two_chiplet_mesh(n);
        let t0 = g.first_tile_on(0).unwrap();
        let t1 = g.first_tile_on(1).unwrap();
        let mut s = TileScheduler::new();
        s.add_tile(t0, TileKind::Cpu, BankId(0));
        s.add_tile(t1, TileKind::Cpu, BankId(1));
        s.set_graph(g);
        let cut = s
            .bind_laplacian_cut(crate::cut::CutId(n as u32), 400)
            .unwrap();
        (s, cut, t0, t1)
    }

    fn thread_job(id: u32, bank: BankId, cut: Option<u32>) -> Job {
        Job {
            id,
            kind: JobKind::Thread,
            tile_hint: None,
            bank_affinity: Some(bank),
            priority: 0,
            deadline_ticks: None,
            tenant: 1,
            cut_id: cut,
            phase: Phase::Compute,
            partition_id: None,
            fence_id: None,
            arena_color: None,
            chiplet_scope: None,
        }
    }

    #[test]
    fn laplacian_bind_n16_refuses_cross_chiplet() {
        let (mut s, cut, t0, t1) = mesh_sched(16);
        assert_eq!(cut.left.count_ones(), 8);
        let c0 = crate::cut::AffinityGraph::mesh_chiplet0_mask(16);
        assert!(cut.left == c0 || cut.right == c0);
        s.enqueue(thread_job(7, BankId(0), Some(cut.id.0)));
        assert_eq!(
            s.place_ok(s.ready.iter().flatten().next().unwrap(), t1)
                .unwrap_err(),
            CutError::CrossCut
        );
        assert!(s.pick(t1).is_none());
        assert!(s.pick(t0).is_some());
    }

    #[test]
    fn laplacian_bind_n32_refuses_cross_chiplet() {
        let (mut s, cut, t0, t1) = mesh_sched(32);
        assert_eq!(cut.left.count_ones(), 16);
        let c0 = crate::cut::AffinityGraph::mesh_chiplet0_mask(32);
        assert!(cut.left == c0 || cut.right == c0);
        assert_eq!(s.fiedler_mask().count_ones(), 16);
        s.enqueue(thread_job(8, BankId(0), Some(cut.id.0)));
        assert!(s.pick(t1).is_none());
        assert!(s.pick(t0).is_some());
    }

    #[test]
    fn laplacian_soft_hint_prefers_same_side_without_cut() {
        let g = crate::cut::AffinityGraph::two_chiplet_mesh(16);
        let t0 = g.first_tile_on(0).unwrap();
        let t1 = g.first_tile_on(1).unwrap();
        let mut s = TileScheduler::new();
        s.add_tile(t0, TileKind::Cpu, BankId(0));
        s.add_tile(t1, TileKind::Cpu, BankId(1));
        s.set_graph(g);
        s.enqueue(thread_job(10, BankId(1), None));
        s.enqueue(thread_job(11, BankId(0), None));
        // No bound cut: both tiles can run either job, but Fiedler side
        // + home bank should send bank0 to t0.
        let j = s.pick(t0).unwrap();
        assert_eq!(j.id, 11);
        let j = s.pick(t1).unwrap();
        assert_eq!(j.id, 10);
    }

    fn scoped_thread(id: u32, bank: BankId, cut: Option<u32>, chiplet: ChipletId) -> Job {
        Job {
            chiplet_scope: Some(ChipletTaskScope::new(chiplet)),
            ..thread_job(id, bank, cut)
        }
    }

    /// Two CPU tiles per chiplet so steal has a same-die victim.
    fn mesh_sched_pair(
        n: usize,
    ) -> (
        TileScheduler,
        crate::cut::SpectralCut,
        TileId,
        TileId,
        TileId,
        TileId,
    ) {
        let g = crate::cut::AffinityGraph::two_chiplet_mesh(n);
        let t00 = g.nth_tile_on(0, 0).unwrap();
        let t01 = g.nth_tile_on(0, 1).unwrap();
        let t10 = g.nth_tile_on(1, 0).unwrap();
        let t11 = g.nth_tile_on(1, 1).unwrap();
        let mut s = TileScheduler::new();
        s.add_tile(t00, TileKind::Cpu, BankId(0));
        s.add_tile(t01, TileKind::Cpu, BankId(0));
        s.add_tile(t10, TileKind::Cpu, BankId(1));
        s.add_tile(t11, TileKind::Cpu, BankId(1));
        s.set_graph(g);
        let cut = s
            .bind_laplacian_cut(crate::cut::CutId(n as u32), 400)
            .unwrap();
        (s, cut, t00, t01, t10, t11)
    }

    fn assert_chiplet_local_strict(n: usize) {
        let (mut s, cut, t00, t01, t10, _t11) = mesh_sched_pair(n);
        assert_eq!(s.chiplet_policy(), ChipletLocalPolicy::Strict);
        let c0 = crate::cut::AffinityGraph::mesh_chiplet0_mask(n);
        assert!(cut.left == c0 || cut.right == c0);
        // Scope only (no cut_id): CrossCut is not why remote pick fails.
        s.enqueue(scoped_thread(20, BankId(0), None, ChipletId(0)));
        assert!(
            s.pick(t10).is_none(),
            "n={n} Strict Chiplet-task stays on die"
        );
        assert!(s.steal(t10).is_none(), "n={n} Strict refuse remote steal");
        let stolen = s.steal(t01).expect("same-chiplet steal");
        assert_eq!(stolen.id, 20);
        s.enqueue(scoped_thread(21, BankId(0), None, ChipletId(0)));
        assert_eq!(s.pick(t00).unwrap().id, 21);
    }

    #[test]
    fn chiplet_task_scope_n16_strict_pick_and_steal() {
        assert_chiplet_local_strict(16);
    }

    #[test]
    fn chiplet_task_scope_n32_strict_pick_and_steal() {
        assert_chiplet_local_strict(32);
    }

    #[test]
    fn chiplet_local_steal_prefers_unscoped_before_remote_scope() {
        let (mut s, _cut, _t00, _t01, t10, _t11) = mesh_sched_pair(16);
        s.set_chiplet_policy(ChipletLocalPolicy::Soft);
        s.enqueue(scoped_thread(30, BankId(0), None, ChipletId(0)));
        let mut remote_ok = thread_job(31, BankId(1), None);
        remote_ok.priority = 7;
        s.enqueue(remote_ok);
        // Soft: chiplet-0 scoped job is stealable on chiplet 1, but the
        // local-first pass takes the unscoped (bank1) job first.
        let j = s.steal(t10).unwrap();
        assert_eq!(j.id, 31);
        let j = s.steal(t10).unwrap();
        assert_eq!(j.id, 30);
    }

    #[test]
    fn chiplet_scope_soft_pick_prefers_matching_die() {
        let g = crate::cut::AffinityGraph::two_chiplet_mesh(16);
        let t0 = g.first_tile_on(0).unwrap();
        let t1 = g.first_tile_on(1).unwrap();
        let mut s = TileScheduler::new();
        s.add_tile(t0, TileKind::Cpu, BankId(0));
        s.add_tile(t1, TileKind::Cpu, BankId(1));
        s.set_graph(g);
        s.set_chiplet_policy(ChipletLocalPolicy::Soft);
        s.enqueue(scoped_thread(40, BankId(1), None, ChipletId(1)));
        s.enqueue(scoped_thread(41, BankId(0), None, ChipletId(0)));
        assert_eq!(s.pick(t0).unwrap().id, 41);
        assert_eq!(s.pick(t1).unwrap().id, 40);
    }

    #[test]
    fn unscoped_jobs_still_steal_across_chiplets() {
        let g = crate::cut::AffinityGraph::two_chiplet_mesh(16);
        let t00 = g.nth_tile_on(0, 0).unwrap();
        let t10 = g.nth_tile_on(1, 0).unwrap();
        let mut s = TileScheduler::new();
        s.add_tile(t00, TileKind::Cpu, BankId(0));
        s.add_tile(t10, TileKind::Cpu, BankId(1));
        s.set_graph(g);
        s.enqueue(thread_job(51, BankId(0), None));
        assert_eq!(s.steal(t10).unwrap().id, 51);
    }
}
