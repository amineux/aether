//! Error-coverage gate (isolation matrix).
//!
//! Every variant of every `*Error` enum in the kernel-side crates
//! (`core/src`, `hal/src`, `drivers/src`, `kernel/src`) must either be
//! **named in evidence** or appear in `error_coverage_allowlist.txt` with a
//! reason. A stale allowlist entry (variant now named, or gone) also fails,
//! so the list cannot rot.
//!
//! "Named in evidence" means the literal `Enum::Variant` appears in:
//! - a `#[cfg(test)]` module of those crates,
//! - a host-run demo body (`fn run_*_demo` / `fn run_*`), which the red-team
//!   and diligence clips execute and CI greps, or
//! - any `.rs` file under `examples/`, `host/`, or a crate's `tests/`.
//!
//! This is a source scan, not a proof that a refuse path is reachable. It
//! exists because `ArenaError::ArenaLimit` was constructed but swallowed for
//! months (`alloc_slot().ok()?`) and no test named it.
//!
//! Print the matrix: `cargo test -p aether-core --test error_coverage -- --nocapture`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const SCAN_DIRS: &[&str] = &["core/src", "hal/src", "drivers/src", "kernel/src"];
const EVIDENCE_DIRS: &[&str] = &["examples", "host", "core/tests", "drivers/tests", "hal/tests"];

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
            if name != "target" && !name.starts_with('.') {
                rs_files(&p, out);
            }
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

/// Blank out comments and string / char literal contents (same length, so
/// byte offsets stay valid) so braces and `//` inside literals do not
/// confuse the brace matcher.
fn strip_line_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut out[from..to.min(b.len())] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = src[i..].find('\n').map(|j| i + j).unwrap_or(b.len());
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = src[i + 2..].find("*/").map(|j| i + 2 + j + 2).unwrap_or(b.len());
                blank(&mut out, i, end);
                i = end;
            }
            b'r' if (b.get(i + 1) == Some(&b'"') || b.get(i + 1) == Some(&b'#'))
                && (i == 0 || !is_ident(b[i - 1])) =>
            {
                // raw string r"..." / r#"..."#
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
                let end = src[j + 1..].find(&close).map(|k| j + 1 + k + close.len()).unwrap_or(b.len());
                blank(&mut out, j + 1, end - close.len());
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
                // char literal ('x', '\n', '\'') vs lifetime ('a)
                if b.get(i + 1) == Some(&b'\\') {
                    let end = src[i + 2..].find('\'').map(|k| i + 2 + k).unwrap_or(b.len());
                    blank(&mut out, i + 1, end);
                    i = end + 1;
                } else if b.get(i + 2) == Some(&b'\'') {
                    blank(&mut out, i + 1, i + 2);
                    i += 3;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Byte index of the `}` closing the block whose `{` is at `open`.
fn match_brace(s: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, &c) in s.iter().enumerate().skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    s.len()
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `(enum, variant)` pairs declared in `src`.
fn error_enums(src: &str) -> Vec<(String, Vec<String>)> {
    let s = strip_line_comments(src);
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(off) = s[at..].find("enum ") {
        let start = at + off;
        at = start + 5;
        if start > 0 && is_ident(b[start - 1]) {
            continue;
        }
        let name: String = s[at..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !name.ends_with("Error") {
            continue;
        }
        let Some(rel) = s[at..].find('{') else { break };
        let open = at + rel;
        let close = match_brace(b, open);
        let body = &s[open + 1..close];
        // Top-level variants: identifiers at nesting depth 0 that start a
        // comma-separated item (skip attributes and payload types).
        let mut variants = Vec::new();
        let mut depth = 0i32;
        let mut item_start = true;
        let bb = body.as_bytes();
        let mut i = 0;
        while i < bb.len() {
            let c = bb[i];
            match c {
                b'(' | b'{' | b'[' | b'<' => depth += 1,
                b')' | b'}' | b']' | b'>' => depth -= 1,
                b',' if depth == 0 => item_start = true,
                b'#' if depth == 0 => {
                    // attribute: skip to matching ']'
                    if let Some(j) = body[i..].find(']') {
                        i += j;
                    }
                }
                _ if depth == 0 && item_start && (c.is_ascii_uppercase()) => {
                    let v: String = body[i..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    i += v.len();
                    variants.push(v);
                    item_start = false;
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
        out.push((name, variants));
        at = close;
    }
    out
}

/// Regions of `src` that count as evidence: `#[cfg(test)]` items and
/// `fn run_*` bodies.
fn evidence_regions(src: &str) -> String {
    let s = strip_line_comments(src);
    let b = s.as_bytes();
    let mut out = String::new();
    for marker in ["#[cfg(test)]", "fn run_"] {
        let mut at = 0;
        while let Some(off) = s[at..].find(marker) {
            let start = at + off;
            let Some(rel) = s[start..].find('{') else { break };
            let open = start + rel;
            // `#[cfg(test)] use ...;` has no block before the `;`.
            if marker.starts_with('#') {
                if let Some(semi) = s[start..open].find(';') {
                    at = start + semi + 1;
                    continue;
                }
            }
            let close = match_brace(b, open);
            out.push_str(&s[open..close.min(s.len())]);
            out.push('\n');
            at = close.min(s.len());
        }
    }
    out
}

fn named(text: &str, en: &str, v: &str) -> bool {
    let pat = format!("{en}::{v}");
    let b = text.as_bytes();
    let mut at = 0;
    while let Some(off) = text[at..].find(&pat) {
        let i = at + off;
        let end = i + pat.len();
        let before_ok = i == 0 || !is_ident(b[i - 1]);
        let after_ok = end >= b.len() || !is_ident(b[end]);
        if before_ok && after_ok {
            return true;
        }
        at = end;
    }
    false
}

fn load_allowlist(root: &Path) -> BTreeMap<String, String> {
    let path = root.join("core/tests/error_coverage_allowlist.txt");
    let text = fs::read_to_string(&path).expect("allowlist present");
    let mut m = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, reason) = line
            .split_once('|')
            .unwrap_or_else(|| panic!("allowlist line {}: want `Enum::Variant | reason`", n + 1));
        let (key, reason) = (key.trim().to_string(), reason.trim().to_string());
        assert!(reason.len() >= 10, "allowlist line {}: give a real reason", n + 1);
        assert!(m.insert(key.clone(), reason).is_none(), "duplicate allowlist entry {key}");
    }
    m
}

struct Scan {
    /// `Enum::Variant` → (declaring file, named in evidence?)
    variants: BTreeMap<String, (String, bool)>,
}

fn scan() -> Scan {
    let root = repo_root();
    let mut decl_files = Vec::new();
    for d in SCAN_DIRS {
        rs_files(&root.join(d), &mut decl_files);
    }
    let mut evidence = String::new();
    for f in &decl_files {
        let src = fs::read_to_string(f).unwrap();
        evidence.push_str(&evidence_regions(&src));
    }
    let mut ev_files = Vec::new();
    for d in EVIDENCE_DIRS {
        rs_files(&root.join(d), &mut ev_files);
    }
    let me = root.join("core/tests/error_coverage.rs");
    for f in ev_files.iter().filter(|f| **f != me) {
        evidence.push_str(&strip_line_comments(&fs::read_to_string(f).unwrap()));
        evidence.push('\n');
    }
    let mut variants = BTreeMap::new();
    for f in &decl_files {
        let src = fs::read_to_string(f).unwrap();
        let rel = f.strip_prefix(&root).unwrap().display().to_string();
        for (en, vs) in error_enums(&src) {
            for v in vs {
                let hit = named(&evidence, &en, &v);
                variants.insert(format!("{en}::{v}"), (rel.clone(), hit));
            }
        }
    }
    Scan { variants }
}

#[test]
fn parser_sees_known_enums() {
    let s = scan();
    for key in [
        "ArenaError::NoSpace",
        "MapError::WrongStream",
        "OpKernelError::ClassMismatch",
        "OpKernelError::Hodge",
        "FabricError::QueueFull",
        "SysError::Fault",
    ] {
        assert!(s.variants.contains_key(key), "scanner missed {key}");
    }
    assert!(s.variants.len() >= 100, "scanned only {} variants", s.variants.len());
}

#[test]
fn every_error_variant_is_named_or_allowlisted() {
    let s = scan();
    let allow = load_allowlist(&repo_root());
    let mut missing = Vec::new();
    let mut stale = Vec::new();
    for (key, (file, hit)) in &s.variants {
        match (hit, allow.contains_key(key)) {
            (false, false) => missing.push(format!("{key}  ({file})")),
            (true, true) => stale.push(format!("{key}  (now named in evidence; drop it)")),
            _ => {}
        }
    }
    let known: BTreeSet<&String> = s.variants.keys().collect();
    for key in allow.keys() {
        if !known.contains(key) {
            stale.push(format!("{key}  (no such variant any more)"));
        }
    }
    let named = s.variants.values().filter(|(_, h)| *h).count();
    println!(
        "[error-coverage] variants={} named={} allowlisted={}",
        s.variants.len(),
        named,
        allow.len()
    );
    for (key, (file, hit)) in &s.variants {
        let state = if *hit { "named" } else { "allowlisted" };
        println!("[error-coverage] {state:<11} {key}  {file}");
    }
    assert!(
        missing.is_empty(),
        "error variants with no test / demo / red-team naming them and no allowlist reason:\n  {}\n\
         Add a test that asserts the variant by name, or an allowlist line \
         `Enum::Variant | reason` in core/tests/error_coverage_allowlist.txt.",
        missing.join("\n  ")
    );
    assert!(
        stale.is_empty(),
        "stale allowlist entries in core/tests/error_coverage_allowlist.txt:\n  {}",
        stale.join("\n  ")
    );
}
