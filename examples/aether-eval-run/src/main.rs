//! Print a host evaluation report.
//!
//! JSON goes to stdout (and to `--json PATH` when set). The human summary
//! goes to stderr. Exit 0 only when metadata, the SoftGreenCtx milli
//! checks, and every named refusal pass. This is a software model.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use aether_eval_run::{evaluate, sha256_hex, EnvMeta};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut json_path: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--json" {
            i += 1;
            let Some(path) = args.get(i) else {
                eprintln!("aether-eval-run: --json needs a path");
                return ExitCode::from(2);
            };
            json_path = Some(PathBuf::from(path));
        } else {
            eprintln!("aether-eval-run: unknown argument {}", args[i]);
            return ExitCode::from(2);
        }
        i += 1;
    }

    let report = evaluate(&collect_env());
    let json = report.to_json();
    eprintln!("{}", report.summary());
    if let Some(path) = &json_path {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(err) = fs::create_dir_all(parent) {
                    eprintln!("aether-eval-run: mkdir {}: {err}", parent.display());
                    return ExitCode::from(2);
                }
            }
        }
        if let Err(err) = fs::write(path, &json) {
            eprintln!("aether-eval-run: write {}: {err}", path.display());
            return ExitCode::from(2);
        }
        eprintln!("[eval] wrote {}", path.display());
    }
    println!("{json}");
    if report.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn collect_env() -> EnvMeta {
    let root = repo_root();
    let git_commit = capture(&root, "git", &["rev-parse", "HEAD"]);
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| !out.stdout.is_empty() || !out.stderr.is_empty());
    let lock_path = root.join("Cargo.lock");
    let cargo_lock_sha256 = fs::read(&lock_path).ok().map(|bytes| sha256_hex(&bytes));
    EnvMeta {
        git_commit,
        git_dirty: dirty,
        rustc_version: capture(&root, "rustc", &["--version"]),
        cargo_version: capture(&root, "cargo", &["--version"]),
        cargo_lock_sha256,
    }
}

fn repo_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("Cargo.lock").is_file() {
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn capture(dir: &Path, cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
