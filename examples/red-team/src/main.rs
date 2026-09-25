//! Host red-team diligence clip — scripted stdout a buyer can grep.
//!
//! Reuses the same refuse paths the kernel self-check already runs
//! (`run_blast_demo`, `run_blast_hops_demo`, `run_blast_nodes_demo`,
//! `run_bank_color_demo`, `run_uncolored_compute_demo`,
//! `run_foreign_tenant_color_demo`, `run_qos_credits_demo`, `run_fence_not_ready_demo`, `run_outside_slice_demo`, `run_typed_window_sid_demo`,
//! `run_silent_remote_demo`, `run_hbm_bw_demo`, `run_xqueue_sid_override_demo`,
//! `run_set_sid_unbound_demo`, `run_submit_sid_demo`, `run_sid_budget_demo`, `run_stage2_fault_demo`, `run_softnoi_exhausted_demo`, `run_hodge_harmonic_tree_demo`, `run_hodge_curl_tree_demo`, `run_hodge_quota_demo`, `run_firewall_demo`, `run_firewall_ident_pa_demo`, `run_greenctx_overcommit_demo`, `run_greenctx_unbound_demo`, `run_greenctx_exhausted_demo`, `run_greenctx_busy_demo`, `run_smmu_overlap_demo`, `run_smmu_not_mapped_demo`, `run_smmu_wrong_stream_demo`, `run_smmu_stream_abort_demo`, `run_softcct_incorrect_elision_demo`, `run_softcct_credit_exhausted_demo`,
//! `run_softsfi_demo`, `run_softnoi_demo`, `run_sva_demo`).
//! This crate does not invent a new isolation mechanism.
//!
//! Each case prints `[redteam] attack=… result=refused`. SoftNoI
//! fabric-class and SoftSFI `ATOMIC_ADD` print one grep-able line
//! each from the same clips. SoftSFI tensor, heap/alloc, unknown, and
//! unknown-base print `[softsfi] tensor=refused`, `[softsfi] heap=refused`,
//! `[softsfi] unknown=refused`, and `[softsfi] unknown-base=refused`
//! (tensor/heap/unknown named `Unmodeled`; unknown-base is load/store
//! with no proved base window → `SfiError::UnknownBase`; heap is not a
//! bump allocator; unknown is bad opcode / illegal width; lines stay
//! separate).
//! Blast hops is `PartitionProfile::admit_hops` → `BlastRadius`. Blast
//! nodes is `PartitionProfile::admit_nodes` → `BlastRadius` (not a hops
//! rehash — hops stays `attack=blast-hops`). Bank color is `admit_wave`
//! → `ColorError::ForeignBank` (not a rehash of CrossCut / hops).
//! Uncolored compute is `admit_wave(..., color=None)` →
//! `ColorError::Uncolored` (Exchange with color still OK; not a
//! ForeignBank / bank-color rehash). Foreign-tenant color is
//! `admit_wave` → `ColorError::ForeignTenant` (Exchange still OK; not a
//! ForeignBank / bank-color or Uncolored / uncolored-compute rehash).
//! QoS credits is `Timeline::submit` →
//! `CreditExhausted` when `in_flight >= qos.credits` (not EventRing
//! theater, not a second charge API). Fence-not-ready is
//! `Timeline::wait` → `FenceNotReady` on issued-but-not-retired
//! (not CreditExhausted / qos-credits; timeout-frees-credit stays
//! inside the qos demo). Outside-slice is
//! `PartitionProfile::admit_chiplet` → `OutsideSlice` (not hops / qos /
//! CrossCut / bank-color). Silent-remote is `map_place` / `map_fabric` →
//! `SpaceError::SilentRemoteLoad` (`MEM_FULL` never implies `UNIFIED`;
//! not CXL productization / BAR0 / SoftNPU). Typed-window-sid is
//! `IommuMap::map_window_sid` → `MapError::WrongStream` (exploration
//! TypedWindow stub; not CXL.mem silicon / BAR0; CrossTenant sibling on
//! foreign pin). Soft HBM BW is `SoftHbmBwMeter::charge` → `QosExceeded`
//! when `used + mbps > qos.bw_mbps` on an HBM `TypedWindow` (not silicon
//! BW / FLOPs / charge_credits). XQueue SID override is Soft-CP
//! `stamp_queue_sid` second SID → `HalError::Busy` (pending sticky SID;
//! not BAR0 / SoftNPU). SET_SID unbound is Soft-CP `set_sid` / submit
//! without Bound SID → `HalError::Fault` (SID-at-submit `StreamAbort`
//! foundation; not xqueue-sid-override / PASID). Submit-sid is Soft-SMMU `resolve_submit` without SET_SID → `MapError::SubmitSid` (walk still OK; not set-sid-unbound StreamAbort / Soft-CP Fault, not SidBudget). Sid-budget is Soft-SMMU `bind_stream` over `SID_BUDGET_PER_TENANT` → `MapError::SidBudget` (peer tenant still has budget; not set-sid-unbound / SubmitSid / xqueue Busy). Stage2-fault is Soft-SMMU `unbind_stage2` then nested walk → `MapError::Stage2Fault` (S1 remains / SID still Bound; not PASID stale / SubmitSid / StreamAbort). SoftNoI-exhausted is `admit` past `MAX_NOI_TENANTS` → `NoiError::Exhausted` (not softnoi-is OverBudget / fabric-class RingExhausted). Hodge harmonic-tree is
//! `OperatorKernelHandle::bind(Tree, Harmonic)` → `HodgeError::HarmonicTreeReduce`
//! (not SoftNoI fabric-class Curl ring). Hodge curl-tree is
//! `OperatorKernelHandle::bind(Tree, Curl)` → `HodgeError::CurlOnTree`
//! (sibling of HarmonicTreeReduce; not SoftNoI fabric-class). Hodge-quota is
//! `HodgeQuota::empty().admit(...)` → `HodgeError::QuotaExceeded` (generous admit
//! succeeds; not HarmonicTreeReduce / CurlOnTree / ClassNotAuthorized / CapTable). Firewall identity guest PA is SoftCmdFirewall
//! `admit_packed` with `iova < SOFT_SMMU_IOVA_BASE` → `HalError::Fault`
//! (addr-cap; **not** mutation-during-validate — `softcmdfirewall` stays
//! separate; not confidential GPU). Greenctx-overcommit is SoftGreenPool
//! `create` past SM/WQ pool → `GreenCtxError::Overcommit` (not diligence
//! `run_greenctx_demo` 70/30 sell; not HW MIG / BAR0 / SoftNPU). Greenctx-unbound is SoftGreenPool
//! `migrate_to_yield` on a queue with no bound ctx → `GreenCtxError::Unbound` (not set-sid-unbound Soft-CP
//! Fault; not greenctx-overcommit SM/WQ ceiling; not HW MIG / BAR0 / SoftNPU). Greenctx-exhausted is SoftGreenPool
//! `create` past `MAX_GREEN_CTX` slots → `GreenCtxError::Exhausted` (not greenctx-overcommit SM/WQ ceiling;
//! not SoftNoI Exhausted; not HW MIG / BAR0 / SoftNPU). Greenctx-busy is SoftGreenPool
//! `migrate_to_yield` when dest is bound to another queue → `GreenCtxError::Busy` (not greenctx-unbound;
//! not greenctx-overcommit / exhausted; not xqueue-sid-override Soft-CP Busy; not HW MIG / BAR0 / SoftNPU). Smmu-overlap is Soft-SMMU
//! `map` same-SID overlapping guest PA → `MapError::Overlap` (disjoint pin admits; not CrossTenant /
//! WrongStream / Stage2Fault / SubmitSid / SidBudget). The closer names what this is **not**:
//! confidential GPU, HW MIG, hardware SMMU (Soft SMMU is software).
//!
//! Run: `make red-team` or `cargo run -p aether-redteam`.

use aether_core::blast::run_blast_demo;
use aether_core::chipsync::{run_softcct_credit_exhausted_demo, run_softcct_incorrect_elision_demo};
use aether_core::iommu::{run_smmu_not_mapped_demo, run_smmu_overlap_demo, run_smmu_stream_abort_demo, run_smmu_wrong_stream_demo, run_stage2_fault_demo};
use aether_core::sid::{run_sid_budget_demo, run_submit_sid_demo};
use aether_core::fence::run_fence_not_ready_demo;
use aether_core::noi::{run_softnoi_demo, run_softnoi_exhausted_demo};
use aether_core::color::{run_bank_color_demo, run_foreign_tenant_color_demo, run_uncolored_compute_demo};
use aether_core::partition::{
    run_blast_hops_demo, run_blast_nodes_demo, run_hbm_bw_demo, run_outside_slice_demo,
    run_qos_credits_demo,
};
use aether_core::space::run_silent_remote_demo;
use aether_core::softsfi::run_softsfi_demo;
use aether_core::sva::run_sva_demo;
use aether_core::window::run_typed_window_sid_demo;
use aether_core::opkernel::{run_hodge_curl_tree_demo, run_hodge_harmonic_tree_demo};
use aether_core::hodge::run_hodge_quota_demo;
use aether_core::greenctx::{run_greenctx_busy_demo, run_greenctx_exhausted_demo, run_greenctx_overcommit_demo, run_greenctx_unbound_demo};
use aether_drivers::{
    run_firewall_demo, run_firewall_ident_pa_demo, run_set_sid_unbound_demo,
    run_xqueue_sid_override_demo,
};

/// Grep-able proof lines. CI matches these exactly.
const LINE_CROSSCUT: &str = "[redteam] attack=wrong-sid-crosscut result=refused";
const LINE_FIREWALL: &str = "[redteam] attack=softcmdfirewall result=refused";
const LINE_SFI: &str = "[redteam] attack=softsfi-oob result=refused";
const LINE_NOI: &str = "[redteam] attack=softnoi-is result=refused";
const LINE_PASID: &str = "[redteam] attack=pasid-stale result=refused";
const LINE_BLAST_HOPS: &str = "[redteam] attack=blast-hops result=refused";
const LINE_BLAST_NODES: &str = "[redteam] attack=blast-nodes result=refused";
const LINE_BANK_COLOR: &str = "[redteam] attack=bank-color result=refused";
const LINE_UNCOLORED: &str = "[redteam] attack=uncolored-compute result=refused";
const LINE_FOREIGN_TENANT: &str = "[redteam] attack=foreign-tenant-color result=refused";
const LINE_QOS_CREDITS: &str = "[redteam] attack=qos-credits result=refused";
const LINE_FENCE_NOT_READY: &str = "[redteam] attack=fence-not-ready result=refused";
const LINE_OUTSIDE_SLICE: &str = "[redteam] attack=outside-slice result=refused";
const LINE_SILENT_REMOTE: &str = "[redteam] attack=silent-remote result=refused";
const LINE_TYPED_WINDOW_SID: &str = "[redteam] attack=typed-window-sid result=refused";
const LINE_HBM_BW: &str = "[redteam] attack=hbm-bw result=refused";
const LINE_XQUEUE_SID_OVERRIDE: &str = "[redteam] attack=xqueue-sid-override result=refused";
const LINE_SET_SID_UNBOUND: &str = "[redteam] attack=set-sid-unbound result=refused";
const LINE_SUBMIT_SID: &str = "[redteam] attack=submit-sid result=refused";
const LINE_SID_BUDGET: &str = "[redteam] attack=sid-budget result=refused";
const LINE_STAGE2_FAULT: &str = "[redteam] attack=stage2-fault result=refused";
const LINE_SOFTNOI_EXHAUSTED: &str = "[redteam] attack=softnoi-exhausted result=refused";
const LINE_HODGE_HARMONIC_TREE: &str = "[redteam] attack=hodge-harmonic-tree result=refused";
const LINE_HODGE_CURL_TREE: &str = "[redteam] attack=hodge-curl-tree result=refused";
const LINE_HODGE_QUOTA: &str = "[redteam] attack=hodge-quota result=refused";
const LINE_FIREWALL_IDENT_PA: &str = "[redteam] attack=firewall-ident-pa result=refused";
const LINE_GREENCTX_OVERCOMMIT: &str = "[redteam] attack=greenctx-overcommit result=refused";
const LINE_GREENCTX_UNBOUND: &str = "[redteam] attack=greenctx-unbound result=refused";
const LINE_GREENCTX_EXHAUSTED: &str = "[redteam] attack=greenctx-exhausted result=refused";
const LINE_GREENCTX_BUSY: &str = "[redteam] attack=greenctx-busy result=refused";
const LINE_SMMU_OVERLAP: &str = "[redteam] attack=smmu-overlap result=refused";
const LINE_SMMU_NOT_MAPPED: &str = "[redteam] attack=smmu-not-mapped result=refused";
const LINE_SMMU_WRONG_STREAM: &str = "[redteam] attack=smmu-wrong-stream result=refused";
const LINE_SMMU_STREAM_ABORT: &str = "[redteam] attack=smmu-stream-abort result=refused";
const LINE_SOFTCCT_INCORRECT_ELISION: &str = "[redteam] attack=softcct-incorrect-elision result=refused";
const LINE_SOFTCCT_CREDIT_EXHAUSTED: &str = "[redteam] attack=softcct-credit-exhausted result=refused";
const LINE_CLASS: &str = "[redteam] fabric-class admit/refuse";
const LINE_ATOMIC: &str = "[redteam] ATOMIC_ADD accept/reject";
const LINE_TENSOR: &str = "[softsfi] tensor=refused";
const LINE_HEAP: &str = "[softsfi] heap=refused";
const LINE_UNKNOWN: &str = "[softsfi] unknown=refused";
const LINE_UNKNOWN_BASE: &str = "[softsfi] unknown-base=refused";
const LINE_NOT: &str =
    "[redteam] what this is not: confidential GPU; not HW MIG; Soft SMMU is software";
const LINE_SEALED: &str = "[redteam] sealed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RedTeamReport {
    crosscut: bool,
    firewall: bool,
    sfi: bool,
    noi: bool,
    pasid: bool,
    blast_hops: bool,
    blast_nodes: bool,
    bank_color: bool,
    uncolored_compute: bool,
    foreign_tenant_color: bool,
    qos_credits: bool,
    fence_not_ready: bool,
    outside_slice: bool,
    silent_remote: bool,
    typed_window_sid: bool,
    hbm_bw: bool,
    xqueue_sid_override: bool,
    set_sid_unbound: bool,
    submit_sid: bool,
    sid_budget: bool,
    stage2_fault: bool,
    softnoi_exhausted: bool,
    hodge_harmonic_tree: bool,
    hodge_curl_tree: bool,
    hodge_quota: bool,
    firewall_ident_pa: bool,
    greenctx_overcommit: bool,
    greenctx_unbound: bool,
    greenctx_exhausted: bool,
    greenctx_busy: bool,
    smmu_overlap: bool,
    smmu_not_mapped: bool,
    smmu_wrong_stream: bool,
    smmu_stream_abort: bool,
    softcct_incorrect_elision: bool,
    softcct_credit_exhausted: bool,
    class: bool,
    atomic: bool,
    tensor: bool,
    heap: bool,
    unknown: bool,
    unknown_base: bool,
}

impl RedTeamReport {
    fn all_ok(&self) -> bool {
        self.crosscut
            && self.firewall
            && self.sfi
            && self.noi
            && self.pasid
            && self.blast_hops
            && self.blast_nodes
            && self.bank_color
            && self.uncolored_compute
            && self.foreign_tenant_color
            && self.qos_credits
            && self.fence_not_ready
            && self.outside_slice
            && self.silent_remote
            && self.typed_window_sid
            && self.hbm_bw
            && self.xqueue_sid_override
            && self.set_sid_unbound
            && self.submit_sid
            && self.sid_budget
            && self.stage2_fault
            && self.softnoi_exhausted
            && self.hodge_harmonic_tree
            && self.hodge_curl_tree
            && self.hodge_quota
            && self.firewall_ident_pa
            && self.greenctx_overcommit
            && self.greenctx_unbound
            && self.greenctx_exhausted
            && self.greenctx_busy
            && self.smmu_overlap
            && self.smmu_not_mapped
            && self.smmu_wrong_stream
            && self.smmu_stream_abort
            && self.softcct_incorrect_elision
            && self.softcct_credit_exhausted
            && self.class
            && self.atomic
            && self.tensor
            && self.heap
            && self.unknown
            && self.unknown_base
    }
}

/// Call the in-tree clips. No new SID / SFI / NoI / SMMU policy.
fn run_redteam() -> RedTeamReport {
    let blast = run_blast_demo();
    let hops = run_blast_hops_demo();
    let nodes = run_blast_nodes_demo();
    let color = run_bank_color_demo();
    let uncolored = run_uncolored_compute_demo();
    let foreign_tenant = run_foreign_tenant_color_demo();
    let qos = run_qos_credits_demo();
    let fence_nr = run_fence_not_ready_demo();
    let outside = run_outside_slice_demo();
    let silent = run_silent_remote_demo();
    let typed_win = run_typed_window_sid_demo();
    let hbm = run_hbm_bw_demo();
    let xqueue_sid = run_xqueue_sid_override_demo();
    let set_sid_unbound = run_set_sid_unbound_demo();
    let submit_sid = run_submit_sid_demo();
    let sid_budget = run_sid_budget_demo();
    let stage2_fault = run_stage2_fault_demo();
    let softnoi_exhausted = run_softnoi_exhausted_demo();
    let hodge_ht = run_hodge_harmonic_tree_demo();
    let hodge_ct = run_hodge_curl_tree_demo();
    let hodge_quota = run_hodge_quota_demo();
    let firewall = run_firewall_demo();
    let firewall_ident = run_firewall_ident_pa_demo();
    let greenctx_over = run_greenctx_overcommit_demo();
    let greenctx_unbound = run_greenctx_unbound_demo();
    let greenctx_exhausted = run_greenctx_exhausted_demo();
    let greenctx_busy = run_greenctx_busy_demo();
    let smmu_overlap = run_smmu_overlap_demo();
    let smmu_not_mapped = run_smmu_not_mapped_demo();
    let smmu_wrong_stream = run_smmu_wrong_stream_demo();
    let smmu_stream_abort = run_smmu_stream_abort_demo();
    let softcct_incorrect_elision = run_softcct_incorrect_elision_demo();
    let softcct_credit_exhausted = run_softcct_credit_exhausted_demo();
    let sfi = run_softsfi_demo();
    let noi = run_softnoi_demo();
    let sva = run_sva_demo();

    RedTeamReport {
        // SpectralCut CrossCut + Soft SMMU wrong-SID DMA abort.
        crosscut: blast.cut_ok && blast.smmu_ok,
        // Copy-then-validate: rewrite during the validate window does
        // not sneak (Host1x hole is closed). In-place path still sneaks
        // so we are not faking the race.
        firewall: firewall.hold_with && firewall.sneak_without,
        // Toy Soft-CP load of a foreign SID window is Oob.
        sfi: sfi.oob_reject && sfi.no_cross_read,
        // Heavy concurrent demand projects IS > 1.5; second tenant refused.
        noi: noi.heavy_refuse && noi.heavy_is_over,
        // Honest unmap drops the SSID TLB; skipped invalidate is a stale
        // hit until the ATC is flushed (then the walk faults).
        pasid: sva.unmap_inv && sva.stale_fault,
        // PartitionProfile::admit_hops: two slices OK; over max_hops → BlastRadius.
        // Not a rehash of CrossCut / wrong-SID (those stay on LINE_CROSSCUT).
        blast_hops: hops.all_ok(),
        // PartitionProfile::admit_nodes: two slices OK; over max_nodes → BlastRadius.
        // Not a hops rehash — hops stays on LINE_BLAST_HOPS.
        blast_nodes: nodes.all_ok(),
        // admit_wave: same-color Compute OK; foreign bank → ForeignBank; Exchange OK.
        // Existing path only — not CrossCut / hops.
        bank_color: color.all_ok(),
        // admit_wave(..., color=None): Compute → Uncolored; Exchange with color OK.
        // Existing path only — not ForeignBank / bank-color (that stays on LINE_BANK_COLOR).
        uncolored_compute: uncolored.all_ok(),
        // admit_wave: same-tenant Compute OK; foreign tenant → ForeignTenant; Exchange OK.
        // Existing path only — not ForeignBank / bank-color or Uncolored / uncolored-compute.
        foreign_tenant_color: foreign_tenant.all_ok(),
        // Timeline::submit: in-budget OK; in_flight >= credits → CreditExhausted;
        // complete/timeout frees credit and admit resumes. Existing fence meter.
        qos_credits: qos.all_ok(),
        // Timeline::wait: issued-but-not-retired → FenceNotReady; complete then wait OK.
        // Existing path — not CreditExhausted / qos-credits; timeout-frees stays in qos demo.
        fence_not_ready: fence_nr.all_ok(),
        // PartitionProfile::admit_chiplet: own chiplet OK; foreign → OutsideSlice.
        // Existing path only — not hops / qos / CrossCut / bank-color.
        outside_slice: outside.all_ok(),
        // map_place / map_fabric: local OK; remote → SilentRemoteLoad.
        // MEM_FULL never implies UNIFIED. Not CXL productization / BAR0 / SoftNPU.
        silent_remote: silent.all_ok(),
        // TypedWindow map_window_sid: match OK; mismatch → WrongStream;
        // foreign pin → CrossTenant. Exploration stub — not CXL.mem / BAR0.
        typed_window_sid: typed_win.all_ok(),
        // SoftHbmBwMeter::charge: in-budget OK; used+mbps > bw_mbps → QosExceeded;
        // release frees; non-HBM → Unbound. Software meter vs QosBudget.bw_mbps.
        hbm_bw: hbm.all_ok(),
        // Soft-CP stamp_queue_sid: same SID while pending OK; foreign → Busy.
        // Existing XQueue sticky-SID path — not BAR0 / SoftNPU / CXL.
        xqueue_sid_override: xqueue_sid.all_ok(),
        // Soft-CP set_sid / submit without Bound → Fault (StreamAbort foundation).
        // SID-at-submit path — not xqueue-sid-override / PASID / BAR0.
        set_sid_unbound: set_sid_unbound.all_ok(),
        // Soft-SMMU resolve_submit without SET_SID → SubmitSid; walk still OK.
        // Not set-sid-unbound StreamAbort / Soft-CP Fault, not SidBudget.
        submit_sid: submit_sid.all_ok(),
        // Soft-SMMU bind_stream over SID_BUDGET_PER_TENANT → SidBudget.
        // Peer tenant still has budget — not set-sid-unbound / SubmitSid / xqueue Busy.
        sid_budget: sid_budget.all_ok(),
        // Soft-SMMU unbind_stage2 then nested walk → Stage2Fault (S1 remains).
        // Not PASID stale / SubmitSid / StreamAbort / set-sid-unbound.
        stage2_fault: stage2_fault.all_ok(),
        // SoftNoI admit past MAX_NOI_TENANTS → Exhausted.
        // Not softnoi-is OverBudget / fabric-class RingExhausted.
        softnoi_exhausted: softnoi_exhausted.all_ok(),
        // OperatorKernelHandle::bind(Tree, Harmonic) → HarmonicTreeReduce.
        // Existing Hodge / opkernel path — not SoftNoI fabric-class Curl ring.
        hodge_harmonic_tree: hodge_ht.all_ok(),
        // OperatorKernelHandle::bind(Tree, Curl) → CurlOnTree.
        // Sibling of HarmonicTreeReduce — not SoftNoI fabric-class Curl ring.
        hodge_curl_tree: hodge_ct.all_ok(),
        // HodgeQuota::empty().admit → QuotaExceeded; generous admits.
        // Not HarmonicTreeReduce / CurlOnTree / ClassNotAuthorized / CapTable.
        hodge_quota: hodge_quota.all_ok(),
        // SoftCmdFirewall admit_packed: Soft-SMMU IOVA OK; identity guest PA → Fault.
        // Addr-cap path — not mutation-during-validate (softcmdfirewall stays separate).
        firewall_ident_pa: firewall_ident.all_ok(),
        // SoftGreenPool::create past SM/WQ pool → Overcommit.
        // Not diligence run_greenctx_demo 70/30 sell; not HW MIG / BAR0 / SoftNPU.
        greenctx_overcommit: greenctx_over.all_ok(),
        // SoftGreenPool::migrate_to_yield unbound queue → Unbound.
        // Not set-sid-unbound Soft-CP Fault; not greenctx-overcommit; not HW MIG.
        greenctx_unbound: greenctx_unbound.all_ok(),
        // SoftGreenPool::create past MAX_GREEN_CTX slots → Exhausted.
        // Not greenctx-overcommit SM/WQ; not SoftNoI Exhausted; not HW MIG.
        greenctx_exhausted: greenctx_exhausted.all_ok(),
        // SoftGreenPool::migrate_to_yield dest bound elsewhere → Busy.
        // Not greenctx-unbound; not overcommit/exhausted; not xqueue Soft-CP Busy; not HW MIG.
        greenctx_busy: greenctx_busy.all_ok(),
        // Soft-SMMU map same-SID overlapping guest PA → Overlap.
        // Not CrossTenant / WrongStream / Stage2Fault / SubmitSid / SidBudget.
        smmu_overlap: smmu_overlap.all_ok(),
        // Soft-SMMU Bound walk hole → NotMapped.
        // Not WrongStream / Stage2Fault / StreamAbort / SubmitSid / Overlap.
        smmu_not_mapped: smmu_not_mapped.all_ok(),
        // Soft-SMMU resolve_submit armed≠packet → WrongStream.
        // Not SubmitSid / Stage2Fault / NotMapped / Overlap / StreamAbort.
        smmu_wrong_stream: smmu_wrong_stream.all_ok(),
        // Soft-SMMU unbound walk → StreamAbort; after unbind aborts again.
        // Not set-sid-unbound Soft-CP Fault / SubmitSid / NotMapped / WrongStream.
        smmu_stream_abort: smmu_stream_abort.all_ok(),
        // SoftCCT incorrect_elide dual-proof fold → refused.
        // Diligence banner sibling; not UCIe / qos-credits / softcct-credit-exhausted.
        softcct_incorrect_elision: softcct_incorrect_elision.all_ok(),
        // SoftCCT record past MAX_CCT_ENTRIES → CreditExhausted.
        // Not Timeline qos-credits / incorrect-elision / UCIe.
        softcct_credit_exhausted: softcct_credit_exhausted.all_ok(),
        // Fabric-class tag: Gradient admits; second Curl refuses (ring).
        class: noi.class_grad_admit && noi.class_curl_refuse,
        // SID-proved toy fetch-add: in-bounds accept, foreign span Oob.
        atomic: sfi.atomic_ok,
        // Named SoftOp::Tensor refuse (`SfiError::Unmodeled`). Not a modeled TMA.
        tensor: sfi.tensor_reject,
        // Named heap/alloc refuse (`SfiError::Unmodeled`). Not a bump allocator.
        heap: sfi.heap_reject,
        // Bad opcode / illegal width → Unmodeled. Tensor/heap lines stay separate.
        unknown: sfi.unknown_reject,
        // Load/store no base window → UnknownBase. Tensor/heap/unknown stay separate.
        unknown_base: sfi.unknown_base_reject,
    }
}

fn emit(ok: bool, line: &str) {
    if ok {
        println!("{line}");
    } else {
        println!("{}", line.replace("result=refused", "result=LEAKED"));
    }
}

fn emit_tagged(ok: bool, line: &str) {
    if ok {
        println!("{line}");
    } else {
        println!("{line} FAIL");
    }
}

fn print_clip(r: &RedTeamReport) {
    println!("Aether red-team diligence clip (host).");
    println!("Same refuse paths as the kernel self-check — not a new isolator.");
    println!();
    emit(r.crosscut, LINE_CROSSCUT);
    emit(r.firewall, LINE_FIREWALL);
    emit(r.sfi, LINE_SFI);
    emit(r.noi, LINE_NOI);
    emit(r.pasid, LINE_PASID);
    emit(r.blast_hops, LINE_BLAST_HOPS);
    emit(r.blast_nodes, LINE_BLAST_NODES);
    emit(r.bank_color, LINE_BANK_COLOR);
    emit(r.uncolored_compute, LINE_UNCOLORED);
    emit(r.foreign_tenant_color, LINE_FOREIGN_TENANT);
    emit(r.qos_credits, LINE_QOS_CREDITS);
    emit(r.fence_not_ready, LINE_FENCE_NOT_READY);
    emit(r.outside_slice, LINE_OUTSIDE_SLICE);
    emit(r.silent_remote, LINE_SILENT_REMOTE);
    emit(r.typed_window_sid, LINE_TYPED_WINDOW_SID);
    emit(r.hbm_bw, LINE_HBM_BW);
    emit(r.xqueue_sid_override, LINE_XQUEUE_SID_OVERRIDE);
    emit(r.set_sid_unbound, LINE_SET_SID_UNBOUND);
    emit(r.submit_sid, LINE_SUBMIT_SID);
    emit(r.sid_budget, LINE_SID_BUDGET);
    emit(r.stage2_fault, LINE_STAGE2_FAULT);
    emit(r.softnoi_exhausted, LINE_SOFTNOI_EXHAUSTED);
    emit(r.hodge_harmonic_tree, LINE_HODGE_HARMONIC_TREE);
    emit(r.hodge_curl_tree, LINE_HODGE_CURL_TREE);
    emit(r.hodge_quota, LINE_HODGE_QUOTA);
    emit(r.firewall_ident_pa, LINE_FIREWALL_IDENT_PA);
    emit(r.greenctx_overcommit, LINE_GREENCTX_OVERCOMMIT);
    emit(r.greenctx_unbound, LINE_GREENCTX_UNBOUND);
    emit(r.greenctx_exhausted, LINE_GREENCTX_EXHAUSTED);
    emit(r.greenctx_busy, LINE_GREENCTX_BUSY);
    emit(r.smmu_overlap, LINE_SMMU_OVERLAP);
    emit(r.smmu_not_mapped, LINE_SMMU_NOT_MAPPED);
    emit(r.smmu_wrong_stream, LINE_SMMU_WRONG_STREAM);
    emit(r.smmu_stream_abort, LINE_SMMU_STREAM_ABORT);
    emit(r.softcct_incorrect_elision, LINE_SOFTCCT_INCORRECT_ELISION);
    emit(r.softcct_credit_exhausted, LINE_SOFTCCT_CREDIT_EXHAUSTED);
    emit_tagged(r.class, LINE_CLASS);
    emit_tagged(r.atomic, LINE_ATOMIC);
    emit_tagged(r.tensor, LINE_TENSOR);
    emit_tagged(r.heap, LINE_HEAP);
    emit_tagged(r.unknown, LINE_UNKNOWN);
    emit_tagged(r.unknown_base, LINE_UNKNOWN_BASE);
    println!();
    println!("{LINE_NOT}");
    if r.all_ok() {
        println!("{LINE_SEALED}");
    } else {
        println!("[redteam] FAIL");
    }
}

fn main() {
    let r = run_redteam();
    print_clip(&r);
    if !r.all_ok() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_attacks_are_refused() {
        let r = run_redteam();
        assert!(r.crosscut, "CrossCut + wrong-SID DMA");
        assert!(r.firewall, "SoftCmdFirewall mutate-during-validate");
        assert!(r.sfi, "SoftSFI OOB load");
        assert!(r.noi, "SoftNoI-IS overload admit");
        assert!(r.pasid, "PASID stale translate after unmap");
        assert!(r.blast_hops, "admit_hops over max_hops → BlastRadius");
        assert!(r.blast_nodes, "admit_nodes over max_nodes → BlastRadius");
        assert!(r.bank_color, "admit_wave foreign bank → ForeignBank");
        assert!(r.uncolored_compute, "admit_wave color=None → Uncolored");
        assert!(r.foreign_tenant_color, "admit_wave foreign tenant → ForeignTenant");
        assert!(r.qos_credits, "Timeline::submit over credits → CreditExhausted");
        assert!(r.fence_not_ready, "Timeline::wait before retire → FenceNotReady");
        assert!(r.outside_slice, "admit_chiplet foreign chiplet → OutsideSlice");
        assert!(r.silent_remote, "map_place remote → SilentRemoteLoad");
        assert!(r.typed_window_sid, "map_window_sid mismatch → WrongStream");
        assert!(r.hbm_bw, "SoftHbmBwMeter over bw_mbps → QosExceeded");
        assert!(
            r.xqueue_sid_override,
            "stamp_queue_sid second SID → HalError::Busy"
        );
        assert!(
            r.set_sid_unbound,
            "set_sid / submit unbound → HalError::Fault"
        );
        assert!(
            r.submit_sid,
            "resolve_submit without SET_SID → SubmitSid"
        );
        assert!(
            r.sid_budget,
            "bind_stream over SID_BUDGET_PER_TENANT → SidBudget"
        );
        assert!(
            r.stage2_fault,
            "unbind_stage2 then walk → Stage2Fault"
        );
        assert!(
            r.softnoi_exhausted,
            "admit past MAX_NOI_TENANTS → Exhausted"
        );
        assert!(
            r.hodge_harmonic_tree,
            "bind(Tree, Harmonic) → HarmonicTreeReduce"
        );
        assert!(
            r.hodge_curl_tree,
            "bind(Tree, Curl) → CurlOnTree"
        );
        assert!(
            r.hodge_quota,
            "HodgeQuota::empty().admit → QuotaExceeded"
        );
        assert!(
            r.firewall_ident_pa,
            "SoftCmdFirewall identity guest PA → HalError::Fault"
        );
        assert!(
            r.greenctx_overcommit,
            "SoftGreenPool::create past pool → Overcommit"
        );
        assert!(
            r.greenctx_unbound,
            "SoftGreenPool::migrate_to_yield unbound → Unbound"
        );
        assert!(
            r.greenctx_exhausted,
            "SoftGreenPool::create past MAX_GREEN_CTX → Exhausted"
        );
        assert!(
            r.greenctx_busy,
            "SoftGreenPool::migrate_to_yield dest bound elsewhere → Busy"
        );
        assert!(
            r.smmu_overlap,
            "Soft-SMMU map same-SID overlapping guest PA → Overlap"
        );
        assert!(
            r.smmu_not_mapped,
            "Soft-SMMU Bound walk hole → NotMapped"
        );
        assert!(
            r.smmu_wrong_stream,
            "Soft-SMMU resolve_submit armed≠packet → WrongStream"
        );
        assert!(
            r.smmu_stream_abort,
            "Soft-SMMU unbound walk → StreamAbort"
        );
        assert!(
            r.softcct_incorrect_elision,
            "SoftCCT incorrect_elide dual-proof → refused"
        );
        assert!(
            r.softcct_credit_exhausted,
            "SoftCCT record past MAX_CCT_ENTRIES → CreditExhausted"
        );
        assert!(r.class, "fabric-class Gradient admit / Curl refuse");
        assert!(r.atomic, "ATOMIC_ADD accept/reject");
        assert!(r.tensor, "SoftSFI tensor named refuse");
        assert!(r.heap, "SoftSFI heap/alloc named refuse");
        assert!(r.unknown, "SoftSFI unknown opcode/illegal width refuse");
        assert!(r.unknown_base, "SoftSFI load/store no base window → UnknownBase");
        assert!(r.all_ok());
    }

    #[test]
    fn proof_line_literals_match_ci_greps() {
        assert!(LINE_CROSSCUT.contains("attack=wrong-sid-crosscut"));
        assert!(LINE_FIREWALL.contains("attack=softcmdfirewall"));
        assert!(LINE_SFI.contains("attack=softsfi-oob"));
        assert!(LINE_NOI.contains("attack=softnoi-is"));
        assert!(LINE_PASID.contains("attack=pasid-stale"));
        assert_eq!(LINE_BLAST_HOPS, "[redteam] attack=blast-hops result=refused");
        assert_eq!(LINE_BLAST_NODES, "[redteam] attack=blast-nodes result=refused");
        assert_eq!(LINE_BANK_COLOR, "[redteam] attack=bank-color result=refused");
        assert_eq!(LINE_UNCOLORED, "[redteam] attack=uncolored-compute result=refused");
        assert_eq!(LINE_FOREIGN_TENANT, "[redteam] attack=foreign-tenant-color result=refused");
        assert_eq!(LINE_QOS_CREDITS, "[redteam] attack=qos-credits result=refused");
        assert_eq!(LINE_FENCE_NOT_READY, "[redteam] attack=fence-not-ready result=refused");
        assert_eq!(LINE_OUTSIDE_SLICE, "[redteam] attack=outside-slice result=refused");
        assert_eq!(LINE_SILENT_REMOTE, "[redteam] attack=silent-remote result=refused");
        assert_eq!(LINE_TYPED_WINDOW_SID, "[redteam] attack=typed-window-sid result=refused");
        assert_eq!(LINE_HBM_BW, "[redteam] attack=hbm-bw result=refused");
        assert_eq!(
            LINE_XQUEUE_SID_OVERRIDE,
            "[redteam] attack=xqueue-sid-override result=refused"
        );
        assert_eq!(
            LINE_SET_SID_UNBOUND,
            "[redteam] attack=set-sid-unbound result=refused"
        );
        assert_eq!(
            LINE_SUBMIT_SID,
            "[redteam] attack=submit-sid result=refused"
        );
        assert_eq!(
            LINE_SID_BUDGET,
            "[redteam] attack=sid-budget result=refused"
        );
        assert_eq!(
            LINE_STAGE2_FAULT,
            "[redteam] attack=stage2-fault result=refused"
        );
        assert_eq!(
            LINE_SOFTNOI_EXHAUSTED,
            "[redteam] attack=softnoi-exhausted result=refused"
        );
        assert_eq!(
            LINE_HODGE_HARMONIC_TREE,
            "[redteam] attack=hodge-harmonic-tree result=refused"
        );
        assert_eq!(
            LINE_HODGE_CURL_TREE,
            "[redteam] attack=hodge-curl-tree result=refused"
        );
        assert_eq!(
            LINE_HODGE_QUOTA,
            "[redteam] attack=hodge-quota result=refused"
        );
        assert_eq!(
            LINE_FIREWALL_IDENT_PA,
            "[redteam] attack=firewall-ident-pa result=refused"
        );
        assert_eq!(
            LINE_GREENCTX_OVERCOMMIT,
            "[redteam] attack=greenctx-overcommit result=refused"
        );
        assert_eq!(
            LINE_GREENCTX_UNBOUND,
            "[redteam] attack=greenctx-unbound result=refused"
        );
        assert_eq!(
            LINE_GREENCTX_EXHAUSTED,
            "[redteam] attack=greenctx-exhausted result=refused"
        );
        assert_eq!(
            LINE_GREENCTX_BUSY,
            "[redteam] attack=greenctx-busy result=refused"
        );
        assert_eq!(
            LINE_SMMU_OVERLAP,
            "[redteam] attack=smmu-overlap result=refused"
        );
        assert_eq!(
            LINE_SMMU_NOT_MAPPED,
            "[redteam] attack=smmu-not-mapped result=refused"
        );
        assert_eq!(
            LINE_SMMU_WRONG_STREAM,
            "[redteam] attack=smmu-wrong-stream result=refused"
        );
        assert_eq!(
            LINE_SMMU_STREAM_ABORT,
            "[redteam] attack=smmu-stream-abort result=refused"
        );
        assert_eq!(
            LINE_SOFTCCT_INCORRECT_ELISION,
            "[redteam] attack=softcct-incorrect-elision result=refused"
        );
        assert_eq!(
            LINE_SOFTCCT_CREDIT_EXHAUSTED,
            "[redteam] attack=softcct-credit-exhausted result=refused"
        );
        assert_eq!(LINE_CLASS, "[redteam] fabric-class admit/refuse");
        assert_eq!(LINE_ATOMIC, "[redteam] ATOMIC_ADD accept/reject");
        assert_eq!(LINE_TENSOR, "[softsfi] tensor=refused");
        assert_eq!(LINE_HEAP, "[softsfi] heap=refused");
        assert_eq!(LINE_UNKNOWN, "[softsfi] unknown=refused");
        assert_eq!(LINE_UNKNOWN_BASE, "[softsfi] unknown-base=refused");
        for line in [
            LINE_CROSSCUT,
            LINE_FIREWALL,
            LINE_SFI,
            LINE_NOI,
            LINE_PASID,
            LINE_BLAST_HOPS,
            LINE_BLAST_NODES,
            LINE_BANK_COLOR,
            LINE_UNCOLORED,
            LINE_FOREIGN_TENANT,
            LINE_QOS_CREDITS,
            LINE_FENCE_NOT_READY,
            LINE_OUTSIDE_SLICE,
            LINE_SILENT_REMOTE,
            LINE_TYPED_WINDOW_SID,
            LINE_HBM_BW,
            LINE_XQUEUE_SID_OVERRIDE,
            LINE_SET_SID_UNBOUND,
            LINE_HODGE_HARMONIC_TREE,
            LINE_HODGE_CURL_TREE,
            LINE_HODGE_QUOTA,
            LINE_FIREWALL_IDENT_PA,
            LINE_GREENCTX_OVERCOMMIT,
            LINE_GREENCTX_UNBOUND,
            LINE_GREENCTX_EXHAUSTED,
            LINE_GREENCTX_BUSY,
            LINE_SMMU_OVERLAP,
            LINE_SMMU_NOT_MAPPED,
            LINE_SMMU_WRONG_STREAM,
            LINE_SMMU_STREAM_ABORT,
            LINE_SOFTCCT_INCORRECT_ELISION,
            LINE_SOFTCCT_CREDIT_EXHAUSTED,
        ] {
            assert!(
                line.starts_with("[redteam] attack=") && line.ends_with(" result=refused"),
                "{line}"
            );
        }
        assert!(LINE_NOT.contains("confidential GPU"));
        assert!(LINE_NOT.contains("not HW MIG"));
        assert!(LINE_NOT.contains("Soft SMMU is software"));
    }
}
