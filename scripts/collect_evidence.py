#!/usr/bin/env python3
"""Capture dated host evidence; preserve failures and never claim hardware proof."""
import argparse
import datetime
import json
import hashlib
import pathlib
import platform
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
COMMANDS = [
    ("host-tests", ["cargo", "test", "--workspace", "--locked"]),
    ("diligence", ["cargo", "run", "--locked", "--quiet", "-p", "aether-diligence-demo", "--bin", "diligence-demo"]),
    ("red-team", ["make", "red-team"]),
    ("kv-fabric", ["cargo", "run", "--locked", "--quiet", "-p", "aether-kv-fabric"]),
    ("partner-hello", ["cargo", "run", "--locked", "--quiet", "-p", "aether-partner-hello"]),
    ("design-win-standin", ["cargo", "run", "--locked", "--quiet", "-p", "aether-design-win-check", "--", "docs/design-win/iree-hal-standin.toml"]),
]


def run(command, timeout):
    try:
        result = subprocess.run(command, cwd=ROOT, capture_output=True,
                                text=True, encoding="utf-8", errors="replace",
                                timeout=timeout)
        return result.returncode, result.stdout, result.stderr
    except (OSError, subprocess.TimeoutExpired) as exc:
        return 1, "", str(exc)


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
        "schema_version": 2,
        "captured_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(),
        "python": sys.version,
        "scope": "Host software models only; no hardware, QEMU or customer validation",
        "metadata": {}, "checks": [],
    }
    for label, command in [("commit", ["git", "rev-parse", "HEAD"]),
                           ("working_tree", ["git", "status", "--porcelain"]),
                           ("rustc", ["rustc", "-Vv"]),
                           ("cargo", ["cargo", "--version"])]:
        code, stdout, stderr = run(command, args.timeout)
        metadata["metadata"][label] = dict(exit_code=code, stdout=stdout, stderr=stderr)
    (output / "Cargo.lock").write_bytes((ROOT / "Cargo.lock").read_bytes())
    metadata["lockfile_sha256"] = hashlib.sha256((output / "Cargo.lock").read_bytes()).hexdigest()
    failed = any(item["exit_code"] != 0 for item in metadata["metadata"].values())
    # Write an initial manifest so an interrupted run cannot look complete.
    metadata["complete"] = False
    manifest = output / "manifest.json"
    manifest.write_text(json.dumps(metadata, indent=2), encoding="utf-8")
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
        manifest.write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    code, stdout, stderr = run(["git", "status", "--porcelain"], args.timeout)
    metadata["metadata"]["working_tree_end"] = dict(exit_code=code, stdout=stdout, stderr=stderr)
    failed |= code != 0
    metadata["complete"] = True
    metadata["passed"] = not failed
    manifest.write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    print(f"Evidence: {output}; passed={not failed}")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
