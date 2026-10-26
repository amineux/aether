//! Determinism gate (gap-map slice 8): the `diligence-demo` binary must print
//! byte-identical stdout across runs, independent of working directory and of
//! common environment noise, and every golden needle in `expected.txt` must
//! appear in that stdout.
//!
//! Measurement only: no output or behaviour change. Runs under the ordinary
//! `cargo test --workspace`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_diligence-demo");

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
    let out = cmd.output().expect("spawn diligence-demo");
    assert!(
        out.status.success(),
        "diligence-demo exited {:?}; stderr:\n{}",
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
fn diligence_stdout_is_byte_identical_across_runs() {
    let here = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tmp = std::env::temp_dir();
    let a = run(&here, false);
    let b = run(&tmp, true);
    let c = run(&here, true);
    assert!(!a.is_empty(), "diligence-demo printed nothing");
    assert!(
        a == b,
        "nondeterministic diligence output: {}",
        first_diff(&a, &b)
    );
    assert!(
        a == c,
        "nondeterministic diligence output: {}",
        first_diff(&a, &c)
    );
}

#[test]
fn binary_stdout_contains_every_golden_needle() {
    let tmp = std::env::temp_dir();
    let out = String::from_utf8(run(&tmp, false)).expect("utf-8 stdout");
    let needles = aether_diligence_demo::expected_needles();
    assert!(!needles.is_empty());
    for n in needles {
        assert!(out.contains(n), "binary stdout missing golden needle: {n}");
    }
}
