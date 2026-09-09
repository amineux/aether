//! Host red-team diligence clip — scripted stdout a buyer can grep.
//!
//! Reuses the same refuse paths the kernel self-check already runs
//! (`run_blast_demo`, `run_firewall_demo`, `run_softsfi_demo`,
//! `run_softnoi_demo`, `run_sva_demo`). This crate does not invent a
//! sixth isolation mechanism.
//!
//! Each case prints `[redteam] attack=… result=refused`. SoftNoI
//! fabric-class and SoftSFI `ATOMIC_ADD` print one grep-able line
//! each from the same clips. The closer names what this is **not**:
//! confidential GPU, HW MIG, hardware SMMU (Soft SMMU is software).
//!
//! Run: `make red-team` or `cargo run -p aether-redteam`.

use aether_core::blast::run_blast_demo;
use aether_core::noi::run_softnoi_demo;
use aether_core::softsfi::run_softsfi_demo;
use aether_core::sva::run_sva_demo;
use aether_drivers::run_firewall_demo;

/// Grep-able proof lines. CI matches these exactly.
const LINE_CROSSCUT: &str = "[redteam] attack=wrong-sid-crosscut result=refused";
const LINE_FIREWALL: &str = "[redteam] attack=softcmdfirewall result=refused";
const LINE_SFI: &str = "[redteam] attack=softsfi-oob result=refused";
const LINE_NOI: &str = "[redteam] attack=softnoi-is result=refused";
const LINE_PASID: &str = "[redteam] attack=pasid-stale result=refused";
const LINE_CLASS: &str = "[redteam] fabric-class admit/refuse";
const LINE_ATOMIC: &str = "[redteam] ATOMIC_ADD accept/reject";
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
    class: bool,
    atomic: bool,
}

impl RedTeamReport {
    fn all_ok(&self) -> bool {
        self.crosscut
            && self.firewall
            && self.sfi
            && self.noi
            && self.pasid
            && self.class
            && self.atomic
    }
}

/// Call the in-tree clips. No new SID / SFI / NoI / SMMU policy.
fn run_redteam() -> RedTeamReport {
    let blast = run_blast_demo();
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
        // Fabric-class tag: Gradient admits; second Curl refuses (ring).
        class: noi.class_grad_admit && noi.class_curl_refuse,
        // SID-proved toy fetch-add: in-bounds accept, foreign span Oob.
        atomic: sfi.atomic_ok,
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
    emit_tagged(r.class, LINE_CLASS);
    emit_tagged(r.atomic, LINE_ATOMIC);
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
        assert!(r.class, "fabric-class Gradient admit / Curl refuse");
        assert!(r.atomic, "ATOMIC_ADD accept/reject");
        assert!(r.all_ok());
    }

    #[test]
    fn proof_line_literals_match_ci_greps() {
        assert!(LINE_CROSSCUT.contains("attack=wrong-sid-crosscut"));
        assert!(LINE_FIREWALL.contains("attack=softcmdfirewall"));
        assert!(LINE_SFI.contains("attack=softsfi-oob"));
        assert!(LINE_NOI.contains("attack=softnoi-is"));
        assert!(LINE_PASID.contains("attack=pasid-stale"));
        assert_eq!(LINE_CLASS, "[redteam] fabric-class admit/refuse");
        assert_eq!(LINE_ATOMIC, "[redteam] ATOMIC_ADD accept/reject");
        for line in [LINE_CROSSCUT, LINE_FIREWALL, LINE_SFI, LINE_NOI, LINE_PASID] {
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
