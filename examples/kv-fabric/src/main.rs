//! `make kv-fabric` / `cargo run -p aether-kv-fabric`
//!
//! Prints the disaggregated-inference leave-behind. Toy bytes. The
//! ratio and the refuse paths are the result. Not NVLink, not CUDA,
//! not MIG, not a measured latency.

use std::fmt::Write as _;

use aether_core::{run_kv_fabric_demo, KvReport, DEMO_LAYERS, DEMO_TOKENS};

fn render(r: &KvReport, out: &mut String) {
    let _ = writeln!(
        out,
        "[kv] handoff read-only seq=1 layers=0..{DEMO_LAYERS} tokens=0..{DEMO_TOKENS}"
    );
    let _ = writeln!(
        out,
        "[kv] fabric-bytes={} copy-bytes={} weights-stay={}",
        r.fabric_bytes, r.copy_bytes, r.weight_bytes
    );
    let _ = writeln!(
        out,
        "[kv] attack=forge result={}",
        refused(r.forge_refused)
    );
    let _ = writeln!(
        out,
        "[kv] attack=write result={}",
        refused(r.read_only)
    );
    let _ = writeln!(
        out,
        "[kv] attack=regrant result={}",
        refused(r.regrant_refused)
    );
    let _ = writeln!(
        out,
        "[kv] attack=weights result={}",
        refused(r.weights_stay)
    );
    let _ = writeln!(out, "[kv] attack=oob result={}", refused(r.oob_refused));
    let _ = writeln!(
        out,
        "[kv] attack=wrong-sid result={}",
        refused(r.wrong_sid)
    );
    let _ = writeln!(
        out,
        "[kv] attack=revoke result={}",
        refused(r.revoke_ok)
    );
    let _ = writeln!(
        out,
        "[kv] deadline-miss seq=1 neighbor={}",
        if r.deadline_isolated && r.neighbor_lives {
            "live"
        } else {
            "FAIL"
        }
    );
    let _ = writeln!(
        out,
        "[kv] dma decode-writable={}",
        if r.decode_readonly_dma { "false" } else { "FAIL" }
    );
    let _ = writeln!(out, "[kv] not-nvlink not-cuda not-mig soft-smmu=software");
}

fn refused(ok: bool) -> &'static str {
    if ok {
        "refused"
    } else {
        "FAIL"
    }
}

fn main() {
    let report = run_kv_fabric_demo();
    let mut out = String::new();
    render(&report, &mut out);
    print!("{out}");
    if !report.all_ok() {
        eprintln!("kv-fabric FAIL");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_needles() {
        let report = run_kv_fabric_demo();
        let mut out = String::new();
        render(&report, &mut out);
        assert!(report.all_ok());
        for needle in [
            "[kv] handoff read-only seq=1 layers=0..4 tokens=0..128",
            "[kv] fabric-bytes=32 copy-bytes=131072 weights-stay=8388608",
            "[kv] attack=forge result=refused",
            "[kv] attack=write result=refused",
            "[kv] attack=regrant result=refused",
            "[kv] attack=weights result=refused",
            "[kv] attack=oob result=refused",
            "[kv] attack=wrong-sid result=refused",
            "[kv] attack=revoke result=refused",
            "[kv] deadline-miss seq=1 neighbor=live",
            "[kv] dma decode-writable=false",
            "[kv] not-nvlink not-cuda not-mig soft-smmu=software",
        ] {
            assert!(out.contains(needle), "missing {needle}\n{out}");
        }
    }
}
