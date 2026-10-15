//! Determinism gate (gap-map slice 8): the `aether-redteam` binary must print
//! byte-identical stdout across runs, independent of working directory and of
//! common environment noise (TZ, locale, backtrace settings).
//!
//! This is measurement only: it adds no refusal and changes no output. It runs
//! under the ordinary `cargo test --workspace`, so any wall-clock, hash-seed,
//! pointer-address or thread-order leak into the red-team clip fails CI.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_aether-redteam");

fn run(dir: &std::path::Path, noisy_env: bool) -> Vec<u8> {
    let mut cmd = Command::new(BIN);
    cmd.current_dir(dir);
    if noisy_env {
        cmd.env("TZ", "Pacific/Kiritimati")
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("RUST_BACKTRACE", "full");
    } else {
        cmd.env_remove("TZ")
            .env_remove("LC_ALL")
            .env_remove("RUST_BACKTRACE");
    }
    let out = cmd.output().expect("spawn aether-redteam");
    assert!(
        out.status.success(),
        "aether-redteam exited {:?}; stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    let (sa, sb) = (String::from_utf8_lossy(a), String::from_utf8_lossy(b));
    for (i, (la, lb)) in sa.lines().zip(sb.lines()).enumerate() {
        if la != lb {
            return format!("line {}:\n  run1: {la}\n  run2: {lb}", i + 1);
        }
    }
    format!(
        "length differs: {} vs {} lines",
        sa.lines().count(),
        sb.lines().count()
    )
}

#[test]
fn redteam_stdout_is_byte_identical_across_runs() {
    let here = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tmp = std::env::temp_dir();
    let a = run(&here, false);
    let b = run(&tmp, true);
    let c = run(&here, true);
    assert!(!a.is_empty(), "aether-redteam printed nothing");
    assert!(
        a == b,
        "nondeterministic red-team output: {}",
        first_diff(&a, &b)
    );
    assert!(
        a == c,
        "nondeterministic red-team output: {}",
        first_diff(&a, &c)
    );
}

#[test]
fn redteam_output_is_sealed_and_has_refusals() {
    let here = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = String::from_utf8(run(&here, false)).expect("utf-8 stdout");
    assert_eq!(out.lines().last(), Some("[redteam] sealed"));
    let refused = out.lines().filter(|l| l.contains("result=refused")).count();
    // Floor, not an exact count: new named refusals only raise it.
    assert!(refused >= 69, "refused lines dropped to {refused}");
    assert!(
        !out.lines().any(|l| l.contains("result=accepted")),
        "a red-team attack was accepted"
    );
}

#[test]
fn diff_helper_reports_the_first_divergent_line() {
    // Negative control: the comparison actually detects a one-byte change.
    let a = b"[x] one\n[x] two\n".to_vec();
    let mut b = a.clone();
    b[12] = b'0';
    assert_ne!(a, b);
    assert!(first_diff(&a, &b).starts_with("line 2:"));
}
