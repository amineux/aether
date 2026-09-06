//! Tile scheduler: CPU threads and accelerator waves as peer jobs.
//!
//! Traditional kernels enqueue GPU work as a device ioctl and hope. Aether
//! places both `Thread` and `AccelWave` on the same fabric scheduler so
//! priority, deadlines, bank affinity, and work-stealing apply uniformly.

use crate::cut::{AffinityGraph, CutError, SpectralCut, MAX_CUTS_SCHED};
use crate::partition::{PartitionError, PartitionProfile};
use crate::phase::Phase;
use crate::types::{BankId, TileId, MAX_TILES};

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
    cuts: [Option<SpectralCut>; MAX_CUTS_SCHED],
    partition: Option<PartitionProfile>,
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
            cuts: [None; MAX_CUTS_SCHED],
            partition: None,
        }
    }

    pub fn bind_partition(&mut self, p: PartitionProfile) {
        self.partition = Some(p);
    }

    pub fn set_graph(&mut self, g: AffinityGraph) {
        self.graph = g;
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
        let mut s = 1000 - job.effective_prio(self.now) as i32 * 100;
        if job.tile_hint == Some(tile.id) {
            s += 50;
        }
        if job.bank_affinity == Some(tile.home_bank) {
            s += 25;
        }
        if let Some(dl) = job.deadline_ticks {
            if dl <= self.now {
                s += 80;
            }
        }
        s
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
    /// We steal the *lowest* scoring compatible job (classic WS).
    pub fn steal(&mut self, thief: TileId) -> Option<Job> {
        let t = *self.tile(thief)?;
        let n = MAX_TILES;
        for k in 0..n {
            let idx = (self.steal_cursor + k) % n;
            let Some(victim) = self.tiles[idx] else { continue };
            if victim.id == thief {
                continue;
            }
            if victim.kind != t.kind {
                continue;
            }
            let mut worst: Option<(usize, i32)> = None;
            for (i, job) in self.ready.iter().enumerate() {
                let Some(job) = job else { continue };
                if job.tile_hint == Some(victim.id) {
                    continue; // hard affinity — do not steal
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
        self.steal_cursor = (self.steal_cursor + 1) % n;
        None
    }

    pub fn pick_or_steal(&mut self, tile: TileId) -> Option<Job> {
        self.pick(tile).or_else(|| self.steal(tile))
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
        });
        let j = s.pick(TileId(0)).unwrap();
        assert_eq!(j.id, 11);
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
            crate::partition::SpatialSlice::single_chiplet(
                crate::types::ChipletId(0),
                1 << 0,
                0b1,
            ),
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
        });
        assert!(s.steal(TileId(2)).is_none());
    }
}
