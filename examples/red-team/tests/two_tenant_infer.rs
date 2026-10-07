//! Two-tenant inference isolation demo: golden summary, per-attack named
//! errors, CPU-exact outputs, determinism, and a negative control.

#[allow(dead_code)]
#[path = "../examples/two_tenant_infer/scenario.rs"]
mod scenario;

use scenario::{cpu_reference, run, summarize, Config, MODEL_A, MODEL_B};

const GOLDEN: &str = "[demo] tenants=2 attacker=1 attacks=9 refused=9 outputs_match_cpu=true deterministic=true unperturbed=true";

#[test]
fn summary_line_is_golden() {
    let s = summarize(Config::with_attacker());
    assert_eq!(s.line(), GOLDEN);
    assert!(s.all_ok());
}

#[test]
fn every_attack_returns_its_named_error() {
    let r = run(Config::with_attacker());
    let got: Vec<(&str, &str)> = r
        .attacks
        .iter()
        .map(|a| (a.name, a.got.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("cross-tenant-unmap", "MapError::CrossTenant"),
            ("bind-foreign-sid", "MapError::CrossTenant"),
            ("stamp-foreign-sid", "MapError::CrossTenant"),
            ("wrong-stream-submit", "MapError::WrongStream"),
            ("dma-read-foreign-weights", "MapError::WrongStream"),
            ("dma-write-foreign-activations", "MapError::WrongStream"),
            ("foreign-arena", "ArenaError::NotOwner"),
            ("user-copy-straddle", "user_pages_ok::Err(0x2c01000)"),
            ("opkernel-wrong-class", "OpKernelError::ClassMismatch"),
        ]
    );
    assert!(r.attacks.iter().all(|a| a.refused));
}

#[test]
fn outputs_match_hand_checked_cpu_reference() {
    // Hand-checked: A row 0 is h = [4,5,5,6] → y = [9,13] → z = [6,14].
    assert_eq!(cpu_reference(&MODEL_A), [6, 14, -2, 6]);
    assert_eq!(cpu_reference(&MODEL_B), [26, -2, -2, 5]);
    let r = run(Config::with_attacker());
    assert!(r.honest_jobs_ok);
    assert_eq!(r.outputs, r.cpu);
}

#[test]
fn byte_identical_across_runs_and_without_attacker() {
    let a = run(Config::with_attacker());
    let b = run(Config::with_attacker());
    let honest = run(Config::honest_only());
    assert_eq!(a.log, b.log);
    assert_eq!(a.output_bytes, b.output_bytes);
    assert_eq!(a.output_bytes, honest.output_bytes);
    assert!(honest.attacks.is_empty());
}

/// Negative control: if A's Memory+MAP cap leaked to C, `unmap_for`
/// accepts, A's mapping is gone, the harness reports the attack as not
/// refused, A's next layer fails, and the summary goes red.
#[test]
fn leaked_cap_control_goes_red() {
    let s = summarize(Config {
        attacker: true,
        leaked_cap: true,
    });
    assert_eq!(s.attacks, 9);
    assert!(!s.run.attacks[0].refused);
    assert_eq!(s.run.attacks[0].got, "accepted");
    // A's window is gone, so C's DMA at A's IOVA now misses as NotMapped,
    // not WrongStream: the harness counts only the expected named error.
    assert_eq!(s.run.attacks[4].got, "MapError::NotMapped");
    assert_eq!(s.run.attacks[5].got, "MapError::NotMapped");
    assert_eq!(s.refused, 6);
    assert!(!s.outputs_match_cpu);
    assert!(!s.unperturbed);
    assert!(!s.all_ok());
    assert!(s
        .run
        .log
        .iter()
        .any(|l| l.starts_with("[demo] tenant=A layer=2 op=Relu FAILED err=MapError::")));
}
