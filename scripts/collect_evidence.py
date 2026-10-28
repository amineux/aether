#!/usr/bin/env python3
"""Capture dated host evidence; preserve failures and never claim hardware proof."""
import argparse
import datetime
import json
import hashlib
import pathlib
import os
import signal
import platform
import subprocess
import sys
import tempfile
import re

from evidence_io import atomic_write_text

ROOT = pathlib.Path(__file__).resolve().parents[1]
COMMANDS = [
    # Cheap (no cargo): SHAs cited in docs/business and docs/SCOREBOARD.md must be
    # on main and within the allowed lag; see docs/business/INVESTOR_EVIDENCE.md.
    ("evidence-freshness", [sys.executable, "scripts/check_evidence_freshness.py", "--self-test"]),
    ("host-tests", ["cargo", "test", "--workspace", "--locked"]),
    ("diligence", ["cargo", "run", "--locked", "--quiet", "-p", "aether-diligence-demo", "--bin", "diligence-demo"]),
    ("red-team", ["make", "red-team"]),
    ("kv-fabric", ["cargo", "run", "--locked", "--quiet", "-p", "aether-kv-fabric"]),
    ("partner-hello", ["cargo", "run", "--locked", "--quiet", "-p", "aether-partner-hello"]),
    # Every row of docs/business/PROVE_IT.md: run the command, grep its line.
    ("prove-it", [sys.executable, "scripts/prove_it.py"]),
    ("design-win-standin", ["cargo", "run", "--locked", "--quiet", "-p", "aether-design-win-check", "--", "docs/design-win/iree-hal-standin.toml"]),
    ("eval-run", ["cargo", "run", "--locked", "--quiet", "-p", "aether-eval-run"]),
]


def source_issues(metadata, commit_sha):
    """One source identity shared by collection, freshness checks and reports."""
    issues = []
    for key in ('commit', 'commit_end', 'working_tree', 'working_tree_end'):
        if metadata.get(key, {}).get('exit_code') != 0:
            issues.append(f'Missing or failed provenance: {key}.')
    sha = metadata.get('commit', {}).get('stdout', '').strip()
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        issues.append('Invalid source commit.')
    if metadata.get('commit_end', {}).get('stdout', '').strip() != sha:
        issues.append('Source commit changed during collection; recapture from a stable checkout.')
    if commit_sha != sha:
        issues.append('Freshness commit_sha does not match the measured source commit.')
    for key in ('working_tree', 'working_tree_end'):
        if metadata.get(key, {}).get('stdout', '').strip():
            issues.append(f'Source tree was modified ({key}); commit and recapture.')
    return issues


def run(command, timeout):
    # Give make/cargo and their children one lifetime. Killing only make leaves
    # cargo alive with inherited output pipes, which can hang communicate().
    try:
        process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True,
                                   encoding="utf-8", errors="replace",
                                   start_new_session=os.name == "posix")
    except OSError as exc:
        return 1, "", str(exc)

    def stop():
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except ProcessLookupError:
            pass

    try:
        stdout, stderr = process.communicate(timeout=timeout)
        return process.returncode, stdout, stderr
    except subprocess.TimeoutExpired:
        stop()
        stdout, stderr = process.communicate()
        return 124, stdout, stderr + f"\nEvidence command timed out after {timeout}s.\n"
    except BaseException:
        stop()
        process.communicate()
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path,
                        help="New directory; existing directories are rejected")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    output = args.output.resolve() if args.output else pathlib.Path(
        tempfile.mkdtemp(prefix="aether-evidence-"))
    if args.output:
        output.mkdir(parents=True, exist_ok=False)
    metadata = {
        "schema_version": 3,
        "captured_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(),
        "python": sys.version,
        "python_executable": sys.executable,
        "scope": "Host software models only; no hardware, QEMU or customer validation",
        "metadata": {}, "checks": [],
    }
    manifest = output / "manifest.json"
    metadata["complete"] = False
    atomic_write_text(manifest, json.dumps(metadata, indent=2))
    for label, command in [("commit", ["git", "rev-parse", "HEAD"]),
                           ("working_tree", ["git", "status", "--porcelain"]),
                           ("rustc", ["rustc", "-Vv"]),
                           ("cargo", ["cargo", "--version"])]:
        code, stdout, stderr = run(command, args.timeout)
        metadata["metadata"][label] = dict(exit_code=code, stdout=stdout, stderr=stderr)
    # The commit this bundle measured; scripts/check_evidence_freshness.py
    # compares the SHAs cited in docs/business and docs/SCOREBOARD.md to it.
    metadata["commit_sha"] = metadata["metadata"]["commit"]["stdout"].strip()
    (output / "Cargo.lock").write_bytes((ROOT / "Cargo.lock").read_bytes())
    metadata["lockfile_sha256"] = hashlib.sha256((output / "Cargo.lock").read_bytes()).hexdigest()
    failed = any(item["exit_code"] != 0 for item in metadata["metadata"].values())
    # Write an initial manifest so an interrupted run cannot look complete.
    metadata["complete"] = False
    atomic_write_text(manifest, json.dumps(metadata, indent=2))
    for label, command in COMMANDS:
        print(f"Running {label}", flush=True)
        code, stdout, stderr = run(command, args.timeout)
        (output / f"{label}.stdout.log").write_text(stdout, encoding="utf-8")
        (output / f"{label}.stderr.log").write_text(stderr, encoding="utf-8")
        missing = []
        if label == "diligence":
            needles = (ROOT / "examples/diligence-demo/expected.txt").read_text(encoding="utf-8")
            missing = [line for line in needles.splitlines()
                       if line.strip() and not line.startswith("#") and line not in stdout]
        passed = code == 0 and not missing
        failed |= not passed
        metadata["checks"].append(dict(name=label, command=command,
                                        exit_code=code, passed=passed,
                                        missing_expected_lines=missing,
                                        logs={stream: hashlib.sha256((output / f"{label}.{stream}.log").read_bytes()).hexdigest()
                                              for stream in ("stdout", "stderr")}))
        atomic_write_text(manifest, json.dumps(metadata, indent=2))
    code, stdout, stderr = run(["git", "status", "--porcelain"], args.timeout)
    metadata["metadata"]["working_tree_end"] = dict(exit_code=code, stdout=stdout, stderr=stderr)
    failed |= code != 0
    code, stdout, stderr = run(["git", "rev-parse", "HEAD"], args.timeout)
    metadata["metadata"]["commit_end"] = dict(exit_code=code, stdout=stdout, stderr=stderr)
    metadata["provenance_issues"] = source_issues(metadata["metadata"], metadata["commit_sha"])
    failed |= bool(metadata["provenance_issues"])
    metadata["complete"] = True
    metadata["passed"] = not failed
    atomic_write_text(manifest, json.dumps(metadata, indent=2))
    print(f"Evidence: {output}; passed={not failed}")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
