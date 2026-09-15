//! Soft-CP host for SparsifiedCollective / `decide_header` at XQueue submit.
//!
//! Lives **beside** [`crate::fakecp`] so SoftNoI / SoftGreenCtx can keep
//! editing the CP. Soft-CP already carries [`FlowClass`] on
//! [`AccelJobDesc`]; this path consults
//! [`aether_core::sparsify::decide_header`] **before** enqueue.
//!
//! - Hodge refuse still wins (Tree+Harmonic → `HalError::BadArg`), even
//!   below threshold. That is **not** a Drop and **not** a red-team
//!   `result=refused` line.
//! - Below-threshold Harmonic → **DROP**: no XQueue push, no submit_seq
//!   charge. Host needle: `[softcp] sparsify DROP`.
//! - At/above threshold Harmonic, and Gradient/Curl → KEEP → ordinary
//!   [`SoftCommandProcessor::submit_xqueue`].
//!
//! Default [`SoftCommandProcessor::submit_xqueue`] stays ungated (SID /
//! XQueue / SoftNoI unchanged). Not a QEMU `[sparsify]` re-grep, not an
//! eigensolve, not a `CpCmd` layout change.

use aether_core::accel::{AccelJobDesc, DmaView};
use aether_core::hodge::{FlowClass, HodgeError};
use aether_core::opkernel::CollectiveKind;
use aether_core::sparsify::{decide_header, SparsifyAction, DEFAULT_THRESHOLD_MILLI};
use aether_core::types::{ChipletId, PhysAddr};
use aether_hal::HalError;

use crate::fakecp::SoftCommandProcessor;
use crate::noi::two_tenant_nop;

fn map_hodge_error(e: HodgeError) -> HalError {
    match e {
        HodgeError::HarmonicTreeReduce | HodgeError::CurlOnTree | HodgeError::ClassNotAuthorized => {
            HalError::BadArg
        }
        HodgeError::QuotaExceeded => HalError::Busy,
    }
}

impl<M: DmaView> SoftCommandProcessor<M> {
    /// XQueue submit gated by sparsify `decide_header`.
    ///
    /// Uses [`AccelJobDesc::flow`] plus an explicit collective topology
    /// (needed so Tree+Harmonic can still refuse) and milli energy.
    ///
    /// Returns:
    /// - `Ok(None)` — **DROP**: authorized no-op; queue untouched, no seq.
    /// - `Ok(Some(seq))` — **KEEP**: enqueued via [`Self::submit_xqueue`].
    /// - `Err(HalError::BadArg)` — Hodge refuse (not a Drop).
    pub fn submit_xqueue_sparsify(
        &mut self,
        queue: u16,
        job: &AccelJobDesc,
        topology: CollectiveKind,
        energy_milli: u32,
    ) -> Result<Option<u32>, HalError> {
        self.submit_xqueue_sparsify_threshold(
            queue,
            job,
            topology,
            energy_milli,
            DEFAULT_THRESHOLD_MILLI,
        )
    }

    /// Same as [`Self::submit_xqueue_sparsify`] with an explicit threshold.
    pub fn submit_xqueue_sparsify_threshold(
        &mut self,
        queue: u16,
        job: &AccelJobDesc,
        topology: CollectiveKind,
        energy_milli: u32,
        threshold_milli: u32,
    ) -> Result<Option<u32>, HalError> {
        match decide_header(topology, job.flow, energy_milli, threshold_milli)
            .map_err(map_hodge_error)?
        {
            SparsifyAction::Drop => Ok(None),
            SparsifyAction::Keep => self.submit_xqueue(queue, job).map(Some),
        }
    }
}

/// Host-tested Soft-CP sparsify clip. Prints `[softcp] sparsify DROP` on
/// the below-threshold Harmonic path (distinct from qemu `[sparsify]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftcpSparsifyReport {
    pub drop_ok: bool,
    pub keep_ok: bool,
    pub refuse_ok: bool,
    pub gradient_ok: bool,
}

impl SoftcpSparsifyReport {
    pub const fn all_ok(self) -> bool {
        self.drop_ok && self.keep_ok && self.refuse_ok && self.gradient_ok
    }
}

/// Tag a Nop with an explicit FlowClass (software enum on the descriptor).
pub fn flow_nop(tenant: u32, chiplet: u8, tile: u16, flow: FlowClass) -> AccelJobDesc {
    two_tenant_nop(tenant, chiplet, tile).with_flow(flow)
}

/// Soft-CP host clip: DROP / KEEP / Hodge-refuse / Gradient pass-through.
///
/// Uses real [`SoftCommandProcessor::submit_xqueue_sparsify`] — not a
/// re-grep of the kernel `[sparsify]` serial line.
pub fn run_softcp_sparsify_demo() -> SoftcpSparsifyReport {
    use crate::fakecp::{SoftCommandProcessor, CP_SSID};
    use aether_core::accel::SliceMem;
    use aether_core::iommu::StreamId;
    use aether_core::types::TileId;

    let mut backing = [0u8; 16];
    let mem = SliceMem {
        base: PhysAddr(0),
        bytes: &mut backing,
    };
    let mut d = SoftCommandProcessor::new(mem);
    let sid = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
    d.create_xqueue(0, sid, 0).unwrap();

    // Below-threshold Torus+Harmonic → DROP (no enqueue).
    let harm_drop = flow_nop(1, 0, 2, FlowClass::Harmonic);
    let drop_out = d
        .submit_xqueue_sparsify_threshold(0, &harm_drop, CollectiveKind::Torus, 499, 500)
        .expect("drop path is Ok(None), not Err");
    let drop_ok = drop_out.is_none() && d.xqueue(0).unwrap().is_empty();

    // At-threshold Torus+Harmonic → KEEP (enqueue).
    let harm_keep = flow_nop(1, 0, 2, FlowClass::Harmonic);
    let keep_out = d
        .submit_xqueue_sparsify_threshold(0, &harm_keep, CollectiveKind::Torus, 500, 500)
        .expect("keep enqueues");
    let keep_ok = keep_out == Some(1) && d.xqueue(0).unwrap().pending() == 1;
    // Drain so later submits see an empty queue depth check.
    let _ = d.service();

    // Tree+Harmonic even below threshold → Hodge refuse (not Drop).
    let illegal = flow_nop(1, 0, 2, FlowClass::Harmonic);
    let refuse = d.submit_xqueue_sparsify_threshold(
        0,
        &illegal,
        CollectiveKind::Tree,
        1,
        500,
    );
    let refuse_ok = refuse == Err(HalError::BadArg) && d.xqueue(0).unwrap().is_empty();

    // Gradient energy ignored → KEEP.
    let grad = flow_nop(1, 0, 2, FlowClass::Gradient);
    let g_out = d
        .submit_xqueue_sparsify(0, &grad, CollectiveKind::Tree, 0)
        .expect("gradient keep");
    let gradient_ok = g_out.is_some() && !d.xqueue(0).unwrap().is_empty();

    SoftcpSparsifyReport {
        drop_ok,
        keep_ok,
        refuse_ok,
        gradient_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakecp::{SoftCommandProcessor, CP_SSID};
    use aether_core::accel::AccelOp;
    use aether_core::accel::SliceMem;
    use aether_core::iommu::StreamId;
    use aether_core::types::TileId;

    fn one_queue(d: &mut SoftCommandProcessor<SliceMem<'_>>) {
        let sid = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        d.create_xqueue(0, sid, 0).unwrap();
    }

    #[test]
    fn softcp_below_threshold_harmonic_drops_without_enqueue() {
        let r = run_softcp_sparsify_demo();
        assert!(r.drop_ok, "below-threshold Harmonic must DROP without enqueue: {r:?}");
        // Soft-CP/host needle — distinct from qemu `[sparsify] …`.
        println!("[softcp] sparsify DROP");
        assert!(r.all_ok(), "{r:?}");
    }

    #[test]
    fn softcp_sparsify_drop_keeps_submit_seq_uncharged() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        one_queue(&mut d);
        let job = flow_nop(1, 0, 2, FlowClass::Harmonic);
        assert_eq!(
            d.submit_xqueue_sparsify_threshold(0, &job, CollectiveKind::Torus, 10, 100)
                .unwrap(),
            None
        );
        assert!(d.xqueue(0).unwrap().is_empty());
        // Ungated path still works after a Drop (seq advances only here).
        let seq = d.submit_xqueue(0, &job).unwrap();
        assert_eq!(seq, 1);
        assert_eq!(d.xqueue(0).unwrap().pending(), 1);
    }

    #[test]
    fn softcp_harmonic_tree_refuse_is_not_drop() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        one_queue(&mut d);
        let job = flow_nop(1, 0, 2, FlowClass::Harmonic);
        assert_eq!(
            d.submit_xqueue_sparsify(0, &job, CollectiveKind::Tree, 1)
                .unwrap_err(),
            HalError::BadArg
        );
        assert!(d.xqueue(0).unwrap().is_empty());
    }

    #[test]
    fn default_submit_xqueue_still_ignores_sparsify() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        one_queue(&mut d);
        // Harmonic Nop with zero energy would Drop on the sparsify path;
        // ungated submit still enqueues (SoftNoI-style opt-in).
        let job = flow_nop(1, 0, 2, FlowClass::Harmonic);
        d.submit_xqueue(0, &job).unwrap();
        assert!(!d.xqueue(0).unwrap().is_empty());
    }

    #[test]
    fn nop_job_is_accel_nop() {
        let j = flow_nop(1, 0, 2, FlowClass::Curl);
        assert_eq!(j.op, AccelOp::Nop);
        assert_eq!(j.flow, FlowClass::Curl);
    }
}
