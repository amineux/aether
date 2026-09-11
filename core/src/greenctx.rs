//! SoftGreenCtx: software SM / work-queue partitions on Soft-CP.
//!
//! SpectraScout M5 digest #1 (after M3 SID-at-submit, M4 XQueue,
//! SoftChipletSync). Soft-CP owns a **fake** SM / WQ pool. XQueues bind
//! to a [`SoftGreenCtx`]. Soft-SMMU SID is unchanged across
//! migrate-to-yield.
//!
//! **Inspiration (not a port, not a product):**
//! - [CUDA Green Contexts](https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__GREEN__CONTEXTS.html):
//!   a lightweight context that owns a subset of streaming multiprocessors
//!   and work queues. Streams / queues bind to that context. Soft
//!   partition — even disjoint SMs do **not** isolate L2 / HBM.
//! - DetShare (arXiv:2603.15042): virtual contexts bound to physical
//!   Green Contexts with SM quotas; **migrate-to-yield** rebinds a queue
//!   to a larger partition at a queue boundary. DetShare has **no**
//!   public repo. GC is a real HW API; this crate is a software model.
//!
//! **Not claimed.** This is not HW MIG, not a BAR firewall, not a CUDA
//! driver, not silicon SM isolation. Host tests measure memcpy-like
//! bandwidth interference in normalized integer units — **not** FLOPs
//! and not a partner benchmark.

use crate::iommu::StreamId;

/// Fake SM pool. Software cap, not a silicon SM count.
pub const SOFT_SM_POOL: u16 = 10;
/// Fake work-queue pool. Software cap, not `CUDA_DEVICE_MAX_CONNECTIONS`.
pub const SOFT_WQ_POOL: u16 = 10;
/// Software Green Context slots on one Soft-CP.
pub const MAX_GREEN_CTX: usize = 4;
/// Large side of the canonical 70/30 split.
pub const SPLIT_70: u16 = 7;
/// Small side of the canonical 70/30 split.
pub const SPLIT_30: u16 = 3;
/// Residual shared-HBM tax (milli) when two queues co-run.
///
/// Partitioned SMs still share a memory system. Honest: **not** MIG.
pub const SHARED_BW_TAX_MILLI: u32 = 100;
/// Canonical memcpy-like clip size (bytes). Not a FLOP count.
pub const DEMO_MEMCPY_BYTES: u32 = 1000;

/// SoftGreenCtx identifier (index into the pool).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GreenCtxId(pub u16);

/// SM + work-queue budget advertised on [`crate::`] AccelDevice / AccelInfo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmWqBudget {
    pub sm: u16,
    pub wq: u16,
}

impl SmWqBudget {
    pub const fn full() -> Self {
        Self {
            sm: SOFT_SM_POOL,
            wq: SOFT_WQ_POOL,
        }
    }

    pub const fn split_70() -> Self {
        Self {
            sm: SPLIT_70,
            wq: SPLIT_70,
        }
    }

    pub const fn split_30() -> Self {
        Self {
            sm: SPLIT_30,
            wq: SPLIT_30,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.sm == 0 && self.wq == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GreenCtxError {
    Exhausted,
    Overcommit,
    Unbound,
    Busy,
    BadArg,
}

/// One software Green Context: exclusive SM/WQ slice + bound XQueue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftGreenCtx {
    pub id: GreenCtxId,
    pub budget: SmWqBudget,
    pub bound_queue: Option<u16>,
}

/// Memcpy-like bandwidth report. Normalized integer units, not FLOPs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemcpyReport {
    pub bytes: u32,
    pub sm: u16,
    pub n_co: u16,
    pub cycles: u32,
    /// `bytes * 1000 / cycles`. Solo on the full pool is 1000.
    pub bw_milli: u32,
}

impl MemcpyReport {
    pub const fn interference_milli(self, solo: Self) -> u32 {
        if solo.bw_milli == 0 || solo.bw_milli <= self.bw_milli {
            0
        } else {
            ((solo.bw_milli - self.bw_milli) as u64 * 1000 / solo.bw_milli as u64) as u32
        }
    }
}

/// Soft-CP SM/WQ pool. Default is unpartitioned (queues share the pool).
pub struct SoftGreenPool {
    total: SmWqBudget,
    allocated_sm: u16,
    allocated_wq: u16,
    ctxs: [Option<SoftGreenCtx>; MAX_GREEN_CTX],
    /// Exclusive SM/WQ slices. Off = unpartitioned baseline.
    partitioned: bool,
    pending_memcpy: u16,
    wave_n: u16,
}

impl SoftGreenPool {
    pub const fn new() -> Self {
        Self {
            total: SmWqBudget::full(),
            allocated_sm: 0,
            allocated_wq: 0,
            ctxs: [None; MAX_GREEN_CTX],
            partitioned: false,
            pending_memcpy: 0,
            wave_n: 1,
        }
    }

    pub const fn total(&self) -> SmWqBudget {
        self.total
    }

    pub const fn partitioned(&self) -> bool {
        self.partitioned
    }

    pub const fn allocated(&self) -> SmWqBudget {
        SmWqBudget {
            sm: self.allocated_sm,
            wq: self.allocated_wq,
        }
    }

    pub fn ctx(&self, id: GreenCtxId) -> Option<&SoftGreenCtx> {
        self.ctxs.get(id.0 as usize)?.as_ref()
    }

    fn ctx_mut(&mut self, id: GreenCtxId) -> Result<&mut SoftGreenCtx, GreenCtxError> {
        self.ctxs
            .get_mut(id.0 as usize)
            .and_then(|s| s.as_mut())
            .ok_or(GreenCtxError::Unbound)
    }

    /// Create a context that charges exclusive SM/WQ (partitioned mode).
    pub fn create(&mut self, budget: SmWqBudget) -> Result<GreenCtxId, GreenCtxError> {
        if budget.sm == 0 || budget.wq == 0 {
            return Err(GreenCtxError::BadArg);
        }
        if self.allocated_sm.saturating_add(budget.sm) > self.total.sm
            || self.allocated_wq.saturating_add(budget.wq) > self.total.wq
        {
            return Err(GreenCtxError::Overcommit);
        }
        for i in 0..MAX_GREEN_CTX {
            if self.ctxs[i].is_none() {
                self.allocated_sm += budget.sm;
                self.allocated_wq += budget.wq;
                let id = GreenCtxId(i as u16);
                self.ctxs[i] = Some(SoftGreenCtx {
                    id,
                    budget,
                    bound_queue: None,
                });
                self.partitioned = true;
                return Ok(id);
            }
        }
        Err(GreenCtxError::Exhausted)
    }

    /// Canonical 70/30 split of the fake SM/WQ pool.
    pub fn split_70_30(&mut self) -> Result<(GreenCtxId, GreenCtxId), GreenCtxError> {
        if self.allocated_sm != 0 || self.allocated_wq != 0 {
            return Err(GreenCtxError::Busy);
        }
        let hi = self.create(SmWqBudget::split_70())?;
        let lo = self.create(SmWqBudget::split_30())?;
        Ok((hi, lo))
    }

    /// Two share-the-pool contexts for the unpartitioned baseline.
    ///
    /// They do **not** charge exclusive SMs. Co-run memcpy divides the
    /// full pool by `n_co`.
    pub fn share_unpartitioned(&mut self) -> Result<(GreenCtxId, GreenCtxId), GreenCtxError> {
        if self.partitioned || self.allocated_sm != 0 {
            return Err(GreenCtxError::Busy);
        }
        let mut ids = [GreenCtxId(0); 2];
        for (k, id) in ids.iter_mut().enumerate() {
            let slot = self
                .ctxs
                .iter()
                .position(|s| s.is_none())
                .ok_or(GreenCtxError::Exhausted)?;
            let gid = GreenCtxId(slot as u16);
            self.ctxs[slot] = Some(SoftGreenCtx {
                id: gid,
                budget: SmWqBudget::full(),
                bound_queue: None,
            });
            *id = gid;
            let _ = k;
        }
        self.partitioned = false;
        Ok((ids[0], ids[1]))
    }

    /// Bind an XQueue to a Green Context. One queue per ctx.
    pub fn bind(&mut self, id: GreenCtxId, queue: u16) -> Result<(), GreenCtxError> {
        if queue as usize >= 2 {
            return Err(GreenCtxError::BadArg);
        }
        for c in self.ctxs.iter().flatten() {
            if c.bound_queue == Some(queue) && c.id != id {
                return Err(GreenCtxError::Busy);
            }
        }
        let ctx = self.ctx_mut(id)?;
        if let Some(q) = ctx.bound_queue {
            if q != queue {
                return Err(GreenCtxError::Busy);
            }
        }
        ctx.bound_queue = Some(queue);
        Ok(())
    }

    pub fn unbind(&mut self, id: GreenCtxId) -> Result<u16, GreenCtxError> {
        let ctx = self.ctx_mut(id)?;
        ctx.bound_queue.take().ok_or(GreenCtxError::Unbound)
    }

    pub fn ctx_for_queue(&self, queue: u16) -> Option<GreenCtxId> {
        self.ctxs
            .iter()
            .flatten()
            .find(|c| c.bound_queue == Some(queue))
            .map(|c| c.id)
    }

    /// Yield at the queue boundary, then rebind `queue` to `dest`.
    ///
    /// Soft-SMMU `sid` is an input/output check only — this pool does
    /// not own the SID. Caller must pass the same value in and out.
    pub fn migrate_to_yield(
        &mut self,
        queue: u16,
        dest: GreenCtxId,
        sid: StreamId,
    ) -> Result<StreamId, GreenCtxError> {
        let src = self.ctx_for_queue(queue).ok_or(GreenCtxError::Unbound)?;
        if src == dest {
            return Ok(sid);
        }
        {
            let dest_ctx = self.ctx(dest).ok_or(GreenCtxError::Unbound)?;
            if let Some(q) = dest_ctx.bound_queue {
                if q != queue {
                    return Err(GreenCtxError::Busy);
                }
            }
        }
        let _ = self.unbind(src)?;
        self.bind(dest, queue)?;
        Ok(sid)
    }

    /// A memcpy-like job was queued. `wave_n` freezes the co-run count
    /// so both sides of a pair see the same `n_co`.
    pub fn note_memcpy_submit(&mut self) {
        self.pending_memcpy = self.pending_memcpy.saturating_add(1);
        self.wave_n = self.pending_memcpy.max(1);
    }

    pub fn wave_n(&self) -> u16 {
        self.wave_n.max(1)
    }

    fn sm_share(&self, ctx: Option<GreenCtxId>, n_co: u16) -> u16 {
        let n = n_co.max(1);
        if self.partitioned {
            ctx.and_then(|id| self.ctx(id))
                .map(|c| c.budget.sm.max(1))
                .unwrap_or(self.total.sm)
        } else {
            (self.total.sm / n).max(1)
        }
    }

    /// Memcpy-like kernel: bytes moved, cycles scaled by SM share.
    ///
    /// Residual HBM tax applies when `n_co > 1` even if SMs are
    /// partitioned. Not a FLOP, not MIG isolation.
    pub fn memcpy(
        &mut self,
        ctx: Option<GreenCtxId>,
        bytes: u32,
        n_co: u16,
    ) -> Result<MemcpyReport, GreenCtxError> {
        if bytes == 0 {
            return Err(GreenCtxError::BadArg);
        }
        let sm = self.sm_share(ctx, n_co);
        let mut cycles = bytes.saturating_mul(self.total.sm as u32) / sm as u32;
        if n_co > 1 {
            cycles = cycles
                .saturating_mul(1000 + SHARED_BW_TAX_MILLI)
                / 1000;
        }
        cycles = cycles.max(1);
        let bw_milli = (bytes as u64 * 1000 / cycles as u64) as u32;
        if self.pending_memcpy > 0 {
            self.pending_memcpy -= 1;
            if self.pending_memcpy == 0 {
                self.wave_n = 1;
            }
        }
        Ok(MemcpyReport {
            bytes,
            sm,
            n_co: n_co.max(1),
            cycles,
            bw_milli,
        })
    }

    /// Solo memcpy on the full pool (unpartitioned, one runner).
    pub fn memcpy_solo(&mut self, bytes: u32) -> Result<MemcpyReport, GreenCtxError> {
        let saved = self.partitioned;
        self.partitioned = false;
        let r = self.memcpy(None, bytes, 1);
        self.partitioned = saved;
        r
    }
}

impl Default for SoftGreenPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Host-identical clip. Kernel prints `[greenctx] …`.
///
/// `*_bw` and `*_interference` are integer milli units already on
/// [`MemcpyReport`] (`bw_milli` / [`MemcpyReport::interference_milli`]).
/// Partner-readable, not FLOPs, not HW MIG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GreenCtxReport {
    pub split_ok: bool,
    pub interference_ok: bool,
    pub migrate_ok: bool,
    pub not_mig: bool,
    pub solo_bw: u32,
    pub unpart_bw: u32,
    pub part70_bw: u32,
    pub part30_bw: u32,
    /// Unpartitioned co-run interference vs solo (`interference_milli`).
    pub unpart_interference: u32,
    /// 70% partition co-run interference vs solo.
    pub part70_interference: u32,
    /// 30% partition co-run interference vs solo.
    pub part30_interference: u32,
}

impl GreenCtxReport {
    pub fn all_ok(&self) -> bool {
        self.split_ok && self.interference_ok && self.migrate_ok && self.not_mig
    }
}

/// 70/30 split, memcpy interference vs unpartitioned, one migrate-to-yield.
///
/// Soft-SMMU SID is a dummy token here (core clip has no CP). Soft-CP
/// host tests pass a real queue SID and assert it does not change.
pub fn run_greenctx_demo() -> GreenCtxReport {
    let sid = StreamId::from_raw(0x00_00_02_01);

    let mut solo_pool = SoftGreenPool::new();
    let solo = solo_pool.memcpy_solo(DEMO_MEMCPY_BYTES).unwrap();

    let mut unpart = SoftGreenPool::new();
    let (ua, ub) = unpart.share_unpartitioned().unwrap();
    unpart.bind(ua, 0).unwrap();
    unpart.bind(ub, 1).unwrap();
    unpart.note_memcpy_submit();
    unpart.note_memcpy_submit();
    let n = unpart.wave_n();
    let u0 = unpart.memcpy(Some(ua), DEMO_MEMCPY_BYTES, n).unwrap();
    let u1 = unpart.memcpy(Some(ub), DEMO_MEMCPY_BYTES, n).unwrap();

    let mut part = SoftGreenPool::new();
    let (hi, lo) = part.split_70_30().unwrap();
    part.bind(hi, 0).unwrap();
    part.bind(lo, 1).unwrap();
    let split_ok = part.partitioned()
        && part.allocated() == SmWqBudget::full()
        && part.ctx(hi).unwrap().budget == SmWqBudget::split_70()
        && part.ctx(lo).unwrap().budget == SmWqBudget::split_30();
    part.note_memcpy_submit();
    part.note_memcpy_submit();
    let n = part.wave_n();
    let p70 = part.memcpy(Some(hi), DEMO_MEMCPY_BYTES, n).unwrap();
    let p30 = part.memcpy(Some(lo), DEMO_MEMCPY_BYTES, n).unwrap();

    // Large partition sees less SM interference than the 50/50 share.
    // Residual HBM tax keeps both below solo — not MIG / BAR firewall.
    let interference_ok = p70.bw_milli > u0.bw_milli
        && u0.bw_milli > p30.bw_milli
        && u0.bw_milli == u1.bw_milli
        && p70.sm == SPLIT_70
        && p30.sm == SPLIT_30;
    let not_mig = p70.bw_milli < solo.bw_milli && u0.bw_milli < solo.bw_milli;

    // Queue A starts on the 30% slice, yields, migrates to 70%. SID sticks.
    let mut mig = SoftGreenPool::new();
    let (hi, lo) = mig.split_70_30().unwrap();
    mig.bind(lo, 0).unwrap();
    let before = sid;
    let after = mig.migrate_to_yield(0, hi, sid).unwrap();
    let migrate_ok = after == before
        && mig.ctx_for_queue(0) == Some(hi)
        && mig.ctx(lo).unwrap().bound_queue.is_none()
        && mig.ctx(hi).unwrap().bound_queue == Some(0);

    GreenCtxReport {
        split_ok,
        interference_ok,
        migrate_ok,
        not_mig,
        solo_bw: solo.bw_milli,
        unpart_bw: u0.bw_milli,
        part70_bw: p70.bw_milli,
        part30_bw: p30.bw_milli,
        unpart_interference: u0.interference_milli(solo),
        part70_interference: p70.interference_milli(solo),
        part30_interference: p30.interference_milli(solo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_70_30_charges_full_pool() {
        let mut p = SoftGreenPool::new();
        let (hi, lo) = p.split_70_30().unwrap();
        assert_eq!(p.total(), SmWqBudget::full());
        assert_eq!(p.allocated(), SmWqBudget::full());
        assert!(p.partitioned());
        assert_eq!(p.ctx(hi).unwrap().budget.sm, 7);
        assert_eq!(p.ctx(lo).unwrap().budget.sm, 3);
        assert_eq!(
            p.create(SmWqBudget {
                sm: 1,
                wq: 1
            })
            .unwrap_err(),
            GreenCtxError::Overcommit
        );
    }

    #[test]
    fn overcommit_refused_before_bind() {
        let mut p = SoftGreenPool::new();
        p.create(SmWqBudget {
            sm: 8,
            wq: 8,
        })
        .unwrap();
        assert_eq!(
            p.create(SmWqBudget::split_30()).unwrap_err(),
            GreenCtxError::Overcommit
        );
    }

    #[test]
    fn memcpy_interference_partitioned_beats_unpartitioned() {
        let r = run_greenctx_demo();
        assert!(r.split_ok);
        assert!(r.interference_ok, "70% ctx must beat 50/50 share");
        assert!(r.not_mig, "residual HBM tax: not MIG");
        assert!(r.migrate_ok);
        assert!(r.all_ok());
        assert!(r.part70_bw > r.unpart_bw);
        assert!(r.unpart_bw > r.part30_bw);
        assert!(r.part70_bw < r.solo_bw);
        // 70% slice takes less interference than the 50/50 share.
        // Residual tax keeps both above zero — not MIG / BAR firewall.
        assert!(r.part70_interference < r.unpart_interference);
        assert!(r.unpart_interference < r.part30_interference);
        assert!(r.part70_interference > 0);
        assert!(r.unpart_interference > 0);
    }

    #[test]
    fn migrate_to_yield_keeps_sid() {
        let mut p = SoftGreenPool::new();
        let (hi, lo) = p.split_70_30().unwrap();
        p.bind(lo, 0).unwrap();
        let sid = StreamId::from_raw(0x11_22_33_44);
        let out = p.migrate_to_yield(0, hi, sid).unwrap();
        assert_eq!(out, sid);
        assert_eq!(p.ctx_for_queue(0), Some(hi));
        assert_eq!(
            p.migrate_to_yield(1, hi, sid).unwrap_err(),
            GreenCtxError::Unbound
        );
        p.bind(lo, 1).unwrap();
        assert_eq!(
            p.migrate_to_yield(1, hi, sid).unwrap_err(),
            GreenCtxError::Busy
        );
    }

    #[test]
    fn solo_bw_is_1000_milli() {
        let mut p = SoftGreenPool::new();
        let r = p.memcpy_solo(DEMO_MEMCPY_BYTES).unwrap();
        assert_eq!(r.sm, SOFT_SM_POOL);
        assert_eq!(r.n_co, 1);
        assert_eq!(r.cycles, DEMO_MEMCPY_BYTES);
        assert_eq!(r.bw_milli, 1000);
        assert_eq!(r.interference_milli(r), 0);
    }

    #[test]
    fn not_mig_and_not_bar_firewall() {
        // Residual tax is the honesty latch: partitioned BW < solo.
        let r = run_greenctx_demo();
        assert!(r.not_mig);
        assert!(r.part70_bw < r.solo_bw);
        let tax = SHARED_BW_TAX_MILLI;
        assert!(tax > 0, "zero tax would look like MIG isolation");
    }
}
