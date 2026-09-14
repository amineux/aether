//! Host red-team diligence clip — scripted stdout a buyer can grep.
//!
//! Reuses the same refuse paths the kernel self-check already runs
//! (`run_blast_demo`, `run_blast_hops_demo`, `run_bank_color_demo`,
//! `run_qos_credits_demo`, `run_firewall_demo`, `run_softsfi_demo`,
//! `run_softnoi_demo`, `run_sva_demo`). This crate does not invent a
//! new isolation mechanism.
//!
//! Each case prints `[redteam] attack=… result=refused`. SoftNoI
//! fabric-class and SoftSFI `ATOMIC_ADD` print one grep-able line
//! each from the same clips. SoftSFI heap/alloc prints
//! `[softsfi] heap=refused` (named `Unmodeled`, not a bump allocator).
//! Blast hops is `PartitionProfile::admit_hops` → `BlastRadius`. Bank
//! color is `admit_wave` → `ColorError::ForeignBank` (not a rehash of
//! CrossCut / hops). QoS credits is `Timeline::submit` →
//! `CreditExhausted` when `in_flight >= qos.credits` (not EventRing
//! theater, not a second charge API). The closer names what this
//! is **not**: confidential GPU, HW MIG, hardware SMMU (Soft SMMU is
//! software).
//!
//! Run: `make red-team` or `cargo run -p aether-redteam`.

use aether_core::blast::run_blast_demo;
use aether_core::noi::run_softnoi_demo;
use aether_core::color::run_bank_color_demo;
use aether_core::partition::{run_blast_hops_demo, run_qos_credits_demo};
use aether_core::softsfi::run_softsfi_demo;
use aether_core::sva::run_sva_demo;
use aether_drivers::run_firewall_demo;

/// Grep-able proof lines. CI matches these exactly.
const LINE_CROSSCUT: &str = "[redteam] attack=wrong-sid-crosscut result=refused";
const LINE_FIREWALL: &str = "[redteam] attack=softcmdfirewall result=refused";
const LINE_SFI: &str = "[redteam] attack=softsfi-oob result=refused";
const LINE_NOI: &str = "[redteam] attack=softnoi-is result=refused";
const LINE_PASID: &str = "[redteam] attack=pasid-stale result=refused";
const LINE_BLAST_HOPS: &str = "[redteam] attack=blast-hops result=refused";
const LINE_BANK_COLOR: &str = "[redteam] attack=bank-color result=refused";
const LINE_QOS_CREDITS: &str = "[redteam] attack=qos-credits result=refused";
const LINE_CLASS: &str = "[redteam] fabric-class admit/refuse";
const LINE_ATOMIC: &str = "[redteam] ATOMIC_ADD accept/reject";
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
    bank_color: bool,
    qos_credits: bool,
    class: bool,
    atomic: bool,
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
            && self.bank_color
            && self.qos_credits
            && self.class
            && self.atomic
            && self.heap
    }
}

/// Call the in-tree clips. No new SID / SFI / NoI / SMMU policy.
fn run_redteam() -> RedTeamReport {
    let blast = run_blast_demo();
    let hops = run_blast_hops_demo();
    let color = run_bank_color_demo();
    let qos = run_qos_credits_demo();
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
        // admit_wave: same-color Compute OK; foreign bank → ForeignBank; Exchange OK.
        // Existing path only — not CrossCut / hops.
        bank_color: color.all_ok(),
        // Timeline::submit: in-budget OK; in_flight >= credits → CreditExhausted;
        // complete/timeout frees credit and admit resumes. Existing fence meter.
        qos_credits: qos.all_ok(),
        // Fabric-class tag: Gradient admits; second Curl refuses (ring).
        class: noi.class_grad_admit && noi.class_curl_refuse,
        // SID-proved toy fetch-add: in-bounds accept, foreign span Oob.
        atomic: sfi.atomic_ok,
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
    emit(r.bank_color, LINE_BANK_COLOR);
    emit(r.qos_credits, LINE_QOS_CREDITS);
    emit_tagged(r.class, LINE_CLASS);
    emit_tagged(r.atomic, LINE_ATOMIC);
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
        assert!(r.bank_color, "admit_wave foreign bank → ForeignBank");
        assert!(r.qos_credits, "Timeline::submit over credits → CreditExhausted");
        assert!(r.class, "fabric-class Gradient admit / Curl refuse");
        assert!(r.atomic, "ATOMIC_ADD accept/reject");
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
        assert_eq!(LINE_BANK_COLOR, "[redteam] attack=bank-color result=refused");
        assert_eq!(LINE_QOS_CREDITS, "[redteam] attack=qos-credits result=refused");
        assert_eq!(LINE_CLASS, "[redteam] fabric-class admit/refuse");
        assert_eq!(LINE_ATOMIC, "[redteam] ATOMIC_ADD accept/reject");
        assert_eq!(LINE_HEAP, "[softsfi] heap=refused");
        for line in [
            LINE_CROSSCUT,
            LINE_FIREWALL,
            LINE_SFI,
            LINE_NOI,
            LINE_PASID,
            LINE_BLAST_HOPS,
            LINE_BANK_COLOR,
            LINE_QOS_CREDITS,
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
