//! Selection for the two-CPU boot smoke, shared with host regression tests.
//! This is smoke-test policy; TileScheduler::pick remains a general scorer.
use aether_core::sched::{Job, JobKind, TileKind, TileScheduler};
use aether_core::types::{BankId, TileId};
use aether_core::Phase;

pub const JOBS: u32 = 8;

fn smoke_job(id: u32, hint: Option<TileId>) -> Job {
    Job {
        id,
        kind: JobKind::Thread,
        tile_hint: hint,
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
    }
}

pub fn prepare(s: &mut TileScheduler) {
    *s = TileScheduler::new();
    s.add_tile(TileId(0), TileKind::Cpu, BankId(0));
    s.add_tile(TileId(1), TileKind::Cpu, BankId(1));
    // The steal-only AP cannot consume BSP-hinted job 1. The rest are stealable.
    s.enqueue(smoke_job(1, Some(TileId(0))));
    for id in 2..=JOBS {
        s.enqueue(smoke_job(id, None));
    }
}

/// The AP must prove a steal, not fall back to pick (which only scores hints).
/// Otherwise an AP-first run can consume the BSP-hinted job as ordinary work.
pub fn take(s: &mut TileScheduler, cpu: u32) -> Option<(Job, bool)> {
    let tile = TileId(cpu as u16);
    if cpu == 0 {
        s.pick(tile)
            .map(|job| (job, false))
            .or_else(|| s.steal(tile).map(|job| (job, true)))
    } else {
        s.steal(tile).map(|job| (job, true))
    }
}
