//! Scoreboard gate: the structural numbers in `docs/SCOREBOARD.md` must match
//! the live sources they are measured from.
//!
//! The doc carries a block of `scoreboard key=value` lines. Each key below is
//! recomputed from the tree; a mismatch (or a missing key) fails the test, so
//! the doc cannot drift from the code it describes. It also refuses phrasing
//! the project does not claim.
//!
//! Only stable, source-countable values are gated here. Run-time numbers
//! (host test count, red-team refused lines, error-variant coverage) are
//! recorded in the doc with the exact command that reproduces them; their own
//! gates are `cargo test`, `examples/red-team/tests/determinism.rs`, and
//! `core/tests/error_coverage.rs`.
//!
//! Print the live values: `cargo test -p aether-core --test scoreboard -- --nocapture`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core/ has a parent")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// `scoreboard key=value ...` lines from the doc.
fn doc_values(doc: &str) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    for line in doc.lines() {
        let Some(rest) = line.trim().strip_prefix("scoreboard ") else {
            continue;
        };
        for kv in rest.split_whitespace() {
            let (k, v) = kv
                .split_once('=')
                .unwrap_or_else(|| panic!("bad scoreboard pair {kv:?}"));
            let v: u64 = v
                .parse()
                .unwrap_or_else(|_| panic!("scoreboard {k}: {v:?} is not a number"));
            assert!(
                m.insert(k.to_string(), v).is_none(),
                "duplicate scoreboard key {k}"
            );
        }
    }
    m
}

/// Body of `pub trait AccelDevice { ... }` (closing brace at column 0).
fn accel_trait(src: &str) -> Vec<&str> {
    let start = src
        .lines()
        .position(|l| l.starts_with("pub trait AccelDevice"))
        .expect("pub trait AccelDevice in hal/src/lib.rs");
    src.lines()
        .skip(start + 1)
        .take_while(|l| !l.starts_with('}'))
        .collect()
}

fn live_values(root: &Path) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();

    // Unsafe budget: per-file lines must sum to the header total.
    let budget = read(root, "unsafe-budget.txt");
    let (mut files, mut total) = (0u64, 0u64);
    for l in budget
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
    {
        let n: u64 = l
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .expect("budget count");
        files += 1;
        total += n;
    }
    let header = budget
        .lines()
        .find(|l| l.starts_with("# files="))
        .expect("unsafe-budget.txt header");
    assert_eq!(
        header,
        format!("# files={files} total={total}"),
        "unsafe-budget.txt header vs body"
    );
    m.insert("unsafe_files".into(), files);
    m.insert("unsafe_total".into(), total);

    // Syscall numbers: `pub const SYS_*: u64 = N;` in core/src/sysnr.rs.
    let sysnr = read(root, "core/src/sysnr.rs");
    let syscalls = sysnr
        .lines()
        .filter(|l| l.starts_with("pub const SYS_") && l.contains(": u64 = "))
        .count();
    m.insert("syscalls".into(), syscalls as u64);

    // Frozen host packet size.
    let ireecp = read(root, "drivers/src/ireecp.rs");
    let size = ireecp
        .lines()
        .find_map(|l| l.strip_prefix("pub const IREE_HAL_CMD_SIZE: usize = "))
        .and_then(|v| v.trim_end_matches(';').trim().parse::<u64>().ok())
        .expect("IREE_HAL_CMD_SIZE in drivers/src/ireecp.rs");
    m.insert("iree_hal_cmd_bytes".into(), size);

    // AccelDevice: every method, and the ones with no default body.
    let hal = read(root, "hal/src/lib.rs");
    let body = accel_trait(&hal);
    let methods = body.iter().filter(|l| l.starts_with("    fn ")).count();
    let required = body
        .iter()
        .filter(|l| l.starts_with("    fn ") && l.trim_end().ends_with(';'))
        .count();
    m.insert("accel_device_methods".into(), methods as u64);
    m.insert("accel_device_required".into(), required as u64);

    // Kani harnesses (bounded model checking; not run in CI).
    let mut kani = 0u64;
    let mut names: Vec<_> = fs::read_dir(root.join("proofs/src"))
        .expect("proofs/src")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    names.sort();
    for p in names {
        kani += fs::read_to_string(&p)
            .unwrap()
            .matches("#[kani::proof]")
            .count() as u64;
    }
    m.insert("kani_harnesses".into(), kani);

    // ISAs booted in CI: one `make qemu*-ci` family per architecture.
    let ci = read(root, ".github/workflows/ci.yml");
    let isas = [
        "run: make qemu-ci",
        "run: make qemu-riscv-ci",
        "run: make qemu-aarch64-ci",
    ]
    .iter()
    .filter(|needle| ci.contains(*needle))
    .count();
    m.insert("ci_isas".into(), isas as u64);

    m
}

#[test]
fn scoreboard_numbers_match_live_sources() {
    let root = repo_root();
    let doc = read(&root, "docs/SCOREBOARD.md");
    let want = doc_values(&doc);
    let live = live_values(&root);
    for (k, v) in &live {
        println!("[scoreboard] {k}={v} doc={:?}", want.get(k));
    }
    let mut bad = Vec::new();
    for (k, v) in &live {
        match want.get(k) {
            Some(d) if d == v => {}
            Some(d) => bad.push(format!("{k}: doc says {d}, tree says {v}")),
            None => bad.push(format!(
                "{k}: missing from the doc's scoreboard block (tree says {v})"
            )),
        }
    }
    for k in want.keys().filter(|k| !live.contains_key(*k)) {
        bad.push(format!(
            "{k}: in the doc but not gated by core/tests/scoreboard.rs"
        ));
    }
    assert!(
        bad.is_empty(),
        "docs/SCOREBOARD.md is stale:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn scoreboard_avoids_unclaimed_phrasing() {
    let doc = read(&repo_root(), "docs/SCOREBOARD.md").to_lowercase();
    let banned = [
        "1000x",
        "formally verified",
        "zero-trust",
        "zero trust",
        "flops",
        "tape-out",
        "tapeout",
    ];
    let hits: Vec<_> = banned.iter().filter(|b| doc.contains(*b)).collect();
    assert!(
        hits.is_empty(),
        "docs/SCOREBOARD.md uses phrasing the project does not claim: {hits:?}"
    );
}
