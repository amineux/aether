//! Machine-readable view of the red-team clip (`make red-team-json`).
//!
//! `aether-redteam --json` reads the text clip that `make red-team` printed
//! (stdin) and turns every `[redteam] attack=… result=…` line into one JSON
//! entry: attack, class, API, error variant, result and the exact text line.
//! The attack names and results come from the clip itself; class / API /
//! error come from `refusal-catalogue.tsv`, which must list the same attacks in
//! the same order. Any mismatch (missing, extra or reordered attack) is an
//! error, so the JSON cannot drift from the clip. This adds no attack and
//! changes no line of the text clip.

/// Tab-separated: attack, class, API, error variant(s). One row per
/// `[redteam] attack=` line, in clip order.
const CATALOGUE_TSV: &str = include_str!("../refusal-catalogue.tsv");

const ATTACK_PREFIX: &str = "[redteam] attack=";

struct Row<'a> {
    attack: &'a str,
    class: &'a str,
    api: &'a str,
    error: &'a str,
}

fn rows() -> Result<Vec<Row<'static>>, String> {
    CATALOGUE_TSV
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| {
            let f: Vec<&str> = l.split('\t').collect();
            match f.as_slice() {
                [attack, class, api, error] => Ok(Row { attack, class, api, error }),
                _ => Err(format!("refusal-catalogue.tsv row {}: expected 4 tab-separated fields", i + 1)),
            }
        })
        .collect()
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Build the JSON document from the text clip. Errors if the clip's attack
/// lines and the catalogue differ in any name or position.
pub fn to_json(clip: &str) -> Result<(String, usize), String> {
    let rows = rows()?;
    let lines: Vec<&str> = clip.lines().filter(|l| l.starts_with(ATTACK_PREFIX)).collect();
    if lines.len() != rows.len() {
        return Err(format!(
            "clip has {} attack lines, catalogue has {} rows",
            lines.len(),
            rows.len()
        ));
    }
    let mut out = String::from("{\n  \"source\": \"make red-team (aether-redteam stdout)\",\n");
    out.push_str(&format!("  \"count\": {},\n  \"entries\": [\n", lines.len()));
    for (i, (line, row)) in lines.iter().zip(&rows).enumerate() {
        let rest = &line[ATTACK_PREFIX.len()..];
        let attack = rest.split(' ').next().unwrap_or("");
        if attack != row.attack {
            return Err(format!(
                "line {}: clip attack {attack:?} but catalogue row {:?}",
                i + 1,
                row.attack
            ));
        }
        let result = rest
            .split(' ')
            .find_map(|t| t.strip_prefix("result="))
            .ok_or_else(|| format!("line {}: no result= token", i + 1))?;
        out.push_str(&format!(
            "    {{\"attack\": \"{}\", \"class\": \"{}\", \"api\": \"{}\", \"error\": \"{}\", \"result\": \"{}\", \"line\": \"{}\"}}{}\n",
            esc(attack),
            esc(row.class),
            esc(row.api),
            esc(row.error),
            esc(result),
            esc(line),
            if i + 1 == lines.len() { "" } else { "," }
        ));
    }
    out.push_str("  ]\n}\n");
    Ok((out, lines.len()))
}

/// `aether-redteam --json`: clip on stdin, JSON on stdout, summary on stderr.
pub fn json_main() -> i32 {
    use std::io::Read;
    let mut clip = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut clip) {
        eprintln!("[red-team-json] cannot read clip from stdin: {e}");
        return 1;
    }
    match to_json(&clip) {
        Ok((json, n)) => {
            print!("{json}");
            eprintln!("[red-team-json] entries={n} matches_clip=true");
            0
        }
        Err(e) => {
            eprintln!("[red-team-json] FAIL: {e}");
            1
        }
    }
}
