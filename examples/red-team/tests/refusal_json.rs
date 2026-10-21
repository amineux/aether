//! `make red-team-json` gate: the JSON refusal catalogue is generated from the
//! same text clip `make red-team` prints, has exactly 99 entries, and matches
//! the clip's `[redteam] attack=` lines one for one, in order. Every
//! `Enum::Variant` it names must exist in the source tree. Measurement only:
//! no attack is added and the text clip is not changed.

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_aether-redteam");

fn clip() -> String {
    let out = Command::new(BIN).output().expect("spawn aether-redteam");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

fn json_of(text: &str) -> (bool, String, String) {
    let mut child = Command::new(BIN)
        .arg("--json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aether-redteam --json");
    child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// Value of `"key": "..."` in a one-line JSON object (handles \" and \\).
fn field(entry: &str, key: &str) -> String {
    let start = entry.find(&format!("\"{key}\": \"")).unwrap_or_else(|| panic!("no {key} in {entry}"))
        + key.len()
        + 5;
    let mut out = String::new();
    let mut chars = entry[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return out,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => break,
            },
            c => out.push(c),
        }
    }
    panic!("unterminated {key} in {entry}");
}

#[test]
fn json_has_99_entries_matching_the_clip_line_for_line() {
    let text = clip();
    let (ok, json, stderr) = json_of(&text);
    assert!(ok, "--json failed: {stderr}");
    assert!(stderr.contains("[red-team-json] entries=99 matches_clip=true"), "{stderr}");
    assert!(json.contains("\"count\": 99,"));

    let clip_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("[redteam] attack=")).collect();
    let entries: Vec<&str> = json.lines().filter(|l| l.trim_start().starts_with("{\"attack\":")).collect();
    assert_eq!(clip_lines.len(), 99, "text clip attack lines");
    assert_eq!(entries.len(), 99, "JSON entries");
    for (i, (line, entry)) in clip_lines.iter().zip(&entries).enumerate() {
        assert_eq!(&field(entry, "line"), line, "entry {} line", i + 1);
        let attack = line["[redteam] attack=".len()..].split(' ').next().unwrap();
        assert_eq!(field(entry, "attack"), attack, "entry {} attack", i + 1);
        assert_eq!(field(entry, "result"), "refused", "entry {} result", i + 1);
        for k in ["class", "api", "error"] {
            assert!(!field(entry, k).is_empty(), "entry {} empty {k}", i + 1);
        }
    }
}

#[test]
fn json_refuses_a_clip_that_drifted_from_the_catalogue() {
    let text = clip();
    let dropped: String = text
        .lines()
        .filter(|l| !l.starts_with("[redteam] attack=hodge-quota "))
        .map(|l| format!("{l}\n"))
        .collect();
    let (ok, _, stderr) = json_of(&dropped);
    assert!(!ok && stderr.contains("FAIL"), "missing attack must fail: {stderr}");

    let renamed = text.replace("attack=smmu-overlap ", "attack=smmu-overlapx ");
    let (ok, _, stderr) = json_of(&renamed);
    assert!(!ok && stderr.contains("FAIL"), "renamed attack must fail: {stderr}");
}

#[test]
fn every_named_error_variant_exists_in_source() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut src = String::new();
    for dir in ["core/src", "drivers/src", "hal/src", "kernel/src"] {
        let mut stack = vec![root.join(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    src.push_str(&std::fs::read_to_string(&p).unwrap());
                    src.push('\n');
                }
            }
        }
    }
    let tsv = std::fs::read_to_string(root.join("examples/red-team/refusal-catalogue.tsv")).unwrap();
    let mut checked = 0;
    for row in tsv.lines().filter(|l| !l.trim().is_empty()) {
        let error = row.split('\t').nth(3).expect("4 fields");
        for tok in error.split(|c: char| !(c.is_ascii_alphanumeric() || c == ':' || c == '_')) {
            let Some((ty, var)) = tok.split_once("::") else { continue };
            if !ty.ends_with("Error") || var.is_empty() {
                continue;
            }
            let decl = format!("pub enum {ty} {{");
            let at = src.find(&decl).unwrap_or_else(|| panic!("{ty} not declared (row: {row})"));
            let body = &src[at..at + src[at..].find("\n}").unwrap()];
            assert!(
                body.lines().any(|l| {
                    let t = l.trim_start();
                    t.starts_with(var) && t[var.len()..].starts_with([',', '(', ' ', '{'])
                }),
                "{ty}::{var} not a variant (row: {row})"
            );
            checked += 1;
        }
    }
    assert!(checked >= 99, "only {checked} variants checked");
}
