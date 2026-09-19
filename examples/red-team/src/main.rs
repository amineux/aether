//! Host red-team diligence clip — scripted stdout a buyer can grep.
//!
//! Reuses the same refuse paths the kernel self-check already runs
//! (`run_blast_demo`, `run_blast_hops_demo`, `run_blast_nodes_demo`,
//! `run_bank_color_demo`, `run_uncolored_compute_demo`,
//! `run_qos_credits_demo`, `run_outside_slice_demo`, `run_typed_window_sid_demo`,
//! `run_silent_remote_demo`, `run_hbm_bw_demo`, `run_xqueue_sid_override_demo`,
//! `run_firewall_demo`, `run_softsfi_demo`, `run_softnoi_demo`, `run_sva_demo`).
//! This crate does not invent a new isolation mechanism.
//!
//! Each case prints `[redteam] attack=… result=refused`. SoftNoI
//! fabric-class and SoftSFI `ATOMIC_ADD` print one grep-able line
//! each from the same clips. SoftSFI tensor and heap/alloc print
//! `[softsfi] tensor=refused` and `[softsfi] heap=refused` (named
//! `Unmodeled`; heap is not a bump allocator).
//! Blast hops is `PartitionProfile::admit_hops` → `BlastRadius`. Blast
//! nodes is `PartitionProfile::admit_nodes` → `BlastRadius` (not a hops
//! rehash — hops stays `attack=blast-hops`). Bank color is `admit_wave`
//! → `ColorError::ForeignBank` (not a rehash of CrossCut / hops).
//! Uncolored compute is `admit_wave(..., color=None)` →
//! `ColorError::Uncolored` (Exchange with color still OK; not a
//! ForeignBank / bank-color rehash). QoS credits is `Timeline::submit` →
//! `CreditExhausted` when `in_flight >= qos.credits` (not EventRing
//! theater, not a second charge API). Outside-slice is
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
//! not BAR0 / SoftNPU). The closer names what this is **not**:
//! confidential GPU, HW MIG, hardware SMMU (Soft SMMU is software).
//!
//! Run: `make red-team` or `cargo run -p aether-redteam`.

use aether_core::blast::run_blast_demo;
use aether_core::noi::run_softnoi_demo;
use aether_core::color::{run_bank_color_demo, run_uncolored_compute_demo};
use aether_core::partition::{
    run_blast_hops_demo, run_blast_nodes_demo, run_hbm_bw_demo, run_outside_slice_demo,
    run_qos_credits_demo,
};
use aether_core::space::run_silent_remote_demo;
use aether_core::softsfi::run_softsfi_demo;
use aether_core::sva::run_sva_demo;
use aether_core::window::run_typed_window_sid_demo;
use aether_drivers::{run_firewall_demo, run_xqueue_sid_override_demo};

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
const LINE_QOS_CREDITS: &str = "[redteam] attack=qos-credits result=refused";
const LINE_OUTSIDE_SLICE: &str = "[redteam] attack=outside-slice result=refused";
const LINE_SILENT_REMOTE: &str = "[redteam] attack=silent-remote result=refused";
const LINE_TYPED_WINDOW_SID: &str = "[redteam] attack=typed-window-sid result=refused";
const LINE_HBM_BW: &str = "[redteam] attack=hbm-bw result=refused";
const LINE_XQUEUE_SID_OVERRIDE: &str = "[redteam] attack=xqueue-sid-override result=refused";
const LINE_CLASS: &str = "[redteam] fabric-class admit/refuse";
const LINE_ATOMIC: &str = "[redteam] ATOMIC_ADD accept/reject";
const LINE_TENSOR: &str = "[softsfi] tensor=refused";
const LINE_HEAP: &str = "[softsfi] heap=refused";
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
    qos_credits: bool,
    outside_slice: bool,
    silent_remote: bool,
    typed_window_sid: bool,
    hbm_bw: bool,
    xqueue_sid_override: bool,
    class: bool,
    atomic: bool,
    tensor: bool,
    heap: bool,
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
            && self.qos_credits
            && self.outside_slice
            && self.silent_remote
            && self.typed_window_sid
            && self.hbm_bw
            && self.xqueue_sid_override
            && self.class
            && self.atomic
            && self.tensor
            && self.heap
    }
}

/// Call the in-tree clips. No new SID / SFI / NoI / SMMU policy.
fn run_redteam() -> RedTeamReport {
    let blast = run_blast_demo();
    let hops = run_blast_hops_demo();
    let nodes = run_blast_nodes_demo();
    let color = run_bank_color_demo();
    let uncolored = run_uncolored_compute_demo();
    let qos = run_qos_credits_demo();
    let outside = run_outside_slice_demo();
    let silent = run_silent_remote_demo();
    let typed_win = run_typed_window_sid_demo();
    let hbm = run_hbm_bw_demo();
    let xqueue_sid = run_xqueue_sid_override_demo();
    let firewall = run_firewall_demo();
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
        // Timeline::submit: in-budget OK; in_flight >= credits → CreditExhausted;
        // complete/timeout frees credit and admit resumes. Existing fence meter.
        qos_credits: qos.all_ok(),
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
        // Fabric-class tag: Gradient admits; second Curl refuses (ring).
        class: noi.class_grad_admit && noi.class_curl_refuse,
        // SID-proved toy fetch-add: in-bounds accept, foreign span Oob.
        atomic: sfi.atomic_ok,
        // Named SoftOp::Tensor refuse (`SfiError::Unmodeled`). Not a modeled TMA.
        tensor: sfi.tensor_reject,
        // Named heap/alloc refuse (`SfiError::Unmodeled`). Not a bump allocator.
        heap: sfi.heap_reject,
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
    emit(r.qos_credits, LINE_QOS_CREDITS);
    emit(r.outside_slice, LINE_OUTSIDE_SLICE);
    emit(r.silent_remote, LINE_SILENT_REMOTE);
    emit(r.typed_window_sid, LINE_TYPED_WINDOW_SID);
    emit(r.hbm_bw, LINE_HBM_BW);
    emit(r.xqueue_sid_override, LINE_XQUEUE_SID_OVERRIDE);
    emit_tagged(r.class, LINE_CLASS);
    emit_tagged(r.atomic, LINE_ATOMIC);
    emit_tagged(r.tensor, LINE_TENSOR);
    emit_tagged(r.heap, LINE_HEAP);
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
        assert!(r.qos_credits, "Timeline::submit over credits → CreditExhausted");
        assert!(r.outside_slice, "admit_chiplet foreign chiplet → OutsideSlice");
        assert!(r.silent_remote, "map_place remote → SilentRemoteLoad");
        assert!(r.typed_window_sid, "map_window_sid mismatch → WrongStream");
        assert!(r.hbm_bw, "SoftHbmBwMeter over bw_mbps → QosExceeded");
        assert!(
            r.xqueue_sid_override,
            "stamp_queue_sid second SID → HalError::Busy"
        );
        assert!(r.class, "fabric-class Gradient admit / Curl refuse");
        assert!(r.atomic, "ATOMIC_ADD accept/reject");
        assert!(r.tensor, "SoftSFI tensor named refuse");
        assert!(r.heap, "SoftSFI heap/alloc named refuse");
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
        assert_eq!(LINE_QOS_CREDITS, "[redteam] attack=qos-credits result=refused");
        assert_eq!(LINE_OUTSIDE_SLICE, "[redteam] attack=outside-slice result=refused");
        assert_eq!(LINE_SILENT_REMOTE, "[redteam] attack=silent-remote result=refused");
        assert_eq!(LINE_TYPED_WINDOW_SID, "[redteam] attack=typed-window-sid result=refused");
        assert_eq!(LINE_HBM_BW, "[redteam] attack=hbm-bw result=refused");
        assert_eq!(
            LINE_XQUEUE_SID_OVERRIDE,
            "[redteam] attack=xqueue-sid-override result=refused"
        );
        assert_eq!(LINE_CLASS, "[redteam] fabric-class admit/refuse");
        assert_eq!(LINE_ATOMIC, "[redteam] ATOMIC_ADD accept/reject");
        assert_eq!(LINE_TENSOR, "[softsfi] tensor=refused");
        assert_eq!(LINE_HEAP, "[softsfi] heap=refused");
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
            LINE_QOS_CREDITS,
            LINE_OUTSIDE_SLICE,
            LINE_SILENT_REMOTE,
            LINE_TYPED_WINDOW_SID,
            LINE_HBM_BW,
            LINE_XQUEUE_SID_OVERRIDE,
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
