//! Unsafe ratchet: trusted-code budget per file.
//!
//! Counts the `unsafe` keyword (blocks, fns, impls, traits, extern blocks)
//! in every `.rs` file of the repository, after blanking comments and string
//! / char literals, and compares the counts with `unsafe-budget.txt` at the
//! repo root. The gate fails when:
//!
//! - a file's count differs from its budget line (grew, or shrank without
//!   lowering the budget, so the ratchet only turns one way per edit),
//! - a file that is not in the budget gains `unsafe`, or
//! - a budget line names a file that no longer has `unsafe`.
//!
//! This measures how much code has to be trusted (an audit surface). It is a
//! source count, not a proof of anything about that code.
//!
//! Print the table: `cargo test -p aether-core --test unsafe_budget -- --nocapture`.
//! Rewrite the budget after an intended change:
//! `AETHER_UNSAFE_BUDGET_WRITE=1 cargo test -p aether-core --test unsafe_budget`,
//! then review the diff of `unsafe-budget.txt` like any other code change.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const BUDGET_FILE: &str = "unsafe-budget.txt";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core/ has a parent")
        .to_path_buf()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if name != "target" && name != "build" && !name.starts_with('.') {
                rs_files(&p, out);
            }
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Blank comments and string / char literal contents (same length) so the
/// word `unsafe` in prose or test strings is not counted.
fn strip(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut out[from.min(b.len())..to.min(b.len())] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = src[i..].find('\n').map(|j| i + j).unwrap_or(b.len());
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                // nested block comments
                let mut depth = 0usize;
                let mut j = i;
                while j < b.len() {
                    if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                        depth += 1;
                        j += 2;
                    } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                blank(&mut out, i, j);
                i = j;
            }
            b'r' if (b.get(i + 1) == Some(&b'"') || b.get(i + 1) == Some(&b'#'))
                && (i == 0 || !is_ident(b[i - 1]) || (b[i - 1] == b'b' && (i < 2 || !is_ident(b[i - 2])))) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close = format!("\"{}", "#".repeat(hashes));
                let end = src[j + 1..]
                    .find(&close)
                    .map(|k| j + 1 + k + close.len())
                    .unwrap_or(b.len());
                blank(&mut out, j + 1, end.saturating_sub(close.len()));
                i = end;
            }
            b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                blank(&mut out, i + 1, j);
                i = j + 1;
            }
            b'\'' => {
                if b.get(i + 1) == Some(&b'\\') {
                    let end = src[i + 2..].find('\'').map(|k| i + 2 + k).unwrap_or(b.len());
                    blank(&mut out, i + 1, end);
                    i = end + 1;
                } else if b.get(i + 2) == Some(&b'\'') {
                    blank(&mut out, i + 1, i + 2);
                    i += 3;
                } else {
                    i += 1; // lifetime or multi-byte char; nothing to blank
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Number of `unsafe` keyword tokens in already-stripped source.
fn count_unsafe(stripped: &str) -> usize {
    let b = stripped.as_bytes();
    let kw = b"unsafe";
    let mut n = 0;
    let mut i = 0;
    while i + kw.len() <= b.len() {
        if &b[i..i + kw.len()] == kw
            && (i == 0 || !is_ident(b[i - 1]))
            && b.get(i + kw.len()).map_or(true, |&c| !is_ident(c))
        {
            n += 1;
            i += kw.len();
        } else {
            i += 1;
        }
    }
    n
}

/// `// SAFETY:` comment lines in raw source (informational, not gated).
fn count_safety_comments(src: &str) -> usize {
    src.lines().filter(|l| l.trim_start().starts_with("// SAFETY:")).count()
}

struct Measured {
    counts: BTreeMap<String, usize>,
    safety: usize,
}

fn measure(root: &Path) -> Measured {
    let mut files = Vec::new();
    rs_files(root, &mut files);
    let mut counts = BTreeMap::new();
    let mut safety = 0;
    for f in files {
        let Ok(src) = fs::read_to_string(&f) else { continue };
        let n = count_unsafe(&strip(&src));
        if n > 0 {
            let rel = f
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            counts.insert(rel, n);
            safety += count_safety_comments(&src);
        }
    }
    Measured { counts, safety }
}

fn load_budget(root: &Path) -> BTreeMap<String, usize> {
    let text = fs::read_to_string(root.join(BUDGET_FILE))
        .unwrap_or_else(|e| panic!("{BUDGET_FILE} missing at repo root: {e}"));
    let mut m = BTreeMap::new();
    for (ln, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(n), Some(path), None) = (it.next(), it.next(), it.next()) else {
            panic!("{BUDGET_FILE}:{}: expected `<count> <path>`, got {line:?}", ln + 1);
        };
        let n: usize = n
            .parse()
            .unwrap_or_else(|_| panic!("{BUDGET_FILE}:{}: bad count {n:?}", ln + 1));
        assert!(
            m.insert(path.to_string(), n).is_none(),
            "{BUDGET_FILE}:{}: duplicate path {path}",
            ln + 1
        );
    }
    m
}

fn render_budget(counts: &BTreeMap<String, usize>) -> String {
    let total: usize = counts.values().sum();
    let mut s = String::new();
    s.push_str("# Unsafe ratchet budget: `unsafe` keyword count per .rs file.\n");
    s.push_str("# Checked by core/tests/unsafe_budget.rs (runs under `cargo test --workspace`).\n");
    s.push_str("# Comments and string literals are not counted. Any change in a count,\n");
    s.push_str("# or a new file with `unsafe`, must come with an edit to this file.\n");
    s.push_str("# Regenerate: AETHER_UNSAFE_BUDGET_WRITE=1 cargo test -p aether-core --test unsafe_budget\n");
    s.push_str(&format!("# files={} total={}\n", counts.len(), total));
    for (path, n) in counts {
        s.push_str(&format!("{n:>4} {path}\n"));
    }
    s
}

#[test]
fn counter_ignores_comments_strings_and_lint_names() {
    let src = r##"
        // unsafe in a comment
        /* unsafe /* nested unsafe */ still comment */
        #![forbid(unsafe_code)]
        #![deny(unsafe_op_in_unsafe_fn)]
        let s = "unsafe";
        let r = r#"unsafe "quoted" unsafe"#;
        let c = 'u';
        fn f<'a>(x: &'a u8) {}
        unsafe fn g() { unsafe { core::hint::unreachable_unchecked() } }
        unsafe impl Send for X {}
        let not_unsafe_ident = 1;
    "##;
    assert_eq!(count_unsafe(&strip(src)), 3);
}

#[test]
fn unsafe_budget_ratchet() {
    let root = repo_root();
    let m = measure(&root);
    let total: usize = m.counts.values().sum();

    if std::env::var_os("AETHER_UNSAFE_BUDGET_WRITE").is_some() {
        fs::write(root.join(BUDGET_FILE), render_budget(&m.counts)).expect("write budget");
        println!("[unsafe] wrote {BUDGET_FILE}: files={} total={total}", m.counts.len());
        return;
    }

    let budget = load_budget(&root);
    let budget_total: usize = budget.values().sum();
    let mut problems = Vec::new();
    for (path, &n) in &m.counts {
        match budget.get(path) {
            None => problems.push(format!("new file with unsafe: {path} count={n} (add `{n:>4} {path}` to {BUDGET_FILE})")),
            Some(&b) if n > b => problems.push(format!("grew: {path} count={n} budget={b}")),
            Some(&b) if n < b => problems.push(format!("shrank: {path} count={n} budget={b} (lower the budget line to {n})")),
            _ => {}
        }
    }
    for (path, &b) in &budget {
        if !m.counts.contains_key(path) {
            problems.push(format!("stale: {path} budget={b} but no unsafe found (delete the line)"));
        }
    }
    for (path, n) in &m.counts {
        println!("[unsafe] {n:>4} {path}");
    }
    let ok = problems.is_empty();
    println!(
        "[unsafe] files={} total={total} budget_total={budget_total} safety_comments={} budget_ok={ok}",
        m.counts.len(),
        m.safety
    );
    assert!(
        ok,
        "unsafe budget mismatch ({} problem(s)):\n  {}\nIf the change is intended, edit {BUDGET_FILE} (or rerun with AETHER_UNSAFE_BUDGET_WRITE=1) and justify it in the PR.",
        problems.len(),
        problems.join("\n  ")
    );
}
