#!/usr/bin/env python3
"""One-command reproduce (`make verify`): re-run the checks behind the scoreboard.

Runs, from the repo root, with the locked lockfile:
  1. host tests            cargo test --workspace --locked
  2. diligence clip        golden lines from examples/diligence-demo/expected.txt
  3. red-team clip         stdout sha256 (twice, must match) + result=refused count
  4. error-coverage matrix variants / named / allowlisted

Prints one `[verify]` line per check and compares the run-time numbers with
the `verify key=value` line in docs/SCOREBOARD.md. Exit 1 on any failure or
mismatch. Host software models only: no QEMU, no hardware.
"""
import hashlib
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCOREBOARD = ROOT / "docs/SCOREBOARD.md"
REDTEAM = ["cargo", "run", "-q", "--locked", "-p", "aether-redteam"]
DILIGENCE = ["cargo", "run", "-q", "--locked", "-p", "aether-diligence-demo",
             "--bin", "diligence-demo"]
COVERAGE = ["cargo", "test", "-q", "--locked", "-p", "aether-core",
            "--test", "error_coverage", "--", "--nocapture"]
HOST_TESTS = ["cargo", "test", "--workspace", "--locked"]


def run(command):
    print(f"[verify] $ {' '.join(command)}", flush=True)
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True,
                            encoding="utf-8", errors="replace")
    return result.returncode, result.stdout, result.stderr


def expected():
    """`verify key=value ...` from docs/SCOREBOARD.md (one line)."""
    lines = [l.strip() for l in SCOREBOARD.read_text(encoding="utf-8").splitlines()
             if l.strip().startswith("verify ")]
    if len(lines) != 1:
        raise SystemExit(f"[verify] FAIL docs/SCOREBOARD.md needs exactly one 'verify' line, found {len(lines)}")
    return dict(kv.split("=", 1) for kv in lines[0].split()[1:])


def host_tests(measured, failures):
    code, out, err = run(HOST_TESTS)
    passed = failed = 0
    for m in re.finditer(r"^test result: \w+\. (\d+) passed; (\d+) failed", out + err, re.M):
        passed += int(m.group(1))
        failed += int(m.group(2))
    measured["host_tests_passed"] = str(passed)
    print(f"[verify] host-tests passed={passed} failed={failed} exit={code}")
    if code != 0 or failed:
        failures.append("host tests failed")


def diligence(failures):
    code, out, _ = run(DILIGENCE)
    needles = (ROOT / "examples/diligence-demo/expected.txt").read_text(encoding="utf-8")
    missing = [l for l in needles.splitlines()
               if l.strip() and not l.startswith("#") and l not in out]
    print(f"[verify] diligence lines={len(out.splitlines())} missing_golden={len(missing)} exit={code}")
    if code != 0 or missing:
        failures.append("diligence clip: " + ("; ".join(missing[:3]) or f"exit {code}"))


def red_team(measured, failures):
    outs = []
    for _ in range(2):
        code, out, _ = run(REDTEAM)
        if code != 0:
            failures.append(f"red-team exit {code}")
        outs.append(out)
    sha = hashlib.sha256(outs[0].encode("utf-8")).hexdigest()
    refused = sum("result=refused" in l for l in outs[0].splitlines())
    deterministic = outs[0] == outs[1]
    measured["refused"] = str(refused)
    measured["redteam_sha256"] = sha
    print(f"[verify] red-team sha256={sha} refused={refused} deterministic={str(deterministic).lower()}")
    if not deterministic:
        failures.append("red-team stdout differs between two runs")


def coverage(measured, failures):
    code, out, err = run(COVERAGE)
    m = re.search(r"\[error-coverage\] variants=(\d+) named=(\d+) allowlisted=(\d+)", out + err)
    if code != 0 or not m:
        failures.append(f"error-coverage matrix failed (exit {code})")
        return
    measured.update(variants=m.group(1), named=m.group(2), allowlisted=m.group(3))
    print(f"[verify] error-coverage variants={m.group(1)} named={m.group(2)} allowlisted={m.group(3)}")


def main():
    want = expected()
    measured, failures = {}, []
    host_tests(measured, failures)
    diligence(failures)
    red_team(measured, failures)
    coverage(measured, failures)
    for key, value in want.items():
        got = measured.get(key)
        if got != value:
            failures.append(f"SCOREBOARD says {key}={value}, this run measured {key}={got}")
    summary = " ".join(f"{k}={measured.get(k)}" for k in want)
    for failure in failures:
        print(f"[verify] FAIL {failure}", file=sys.stderr)
    print(f"[verify] {summary} scoreboard_match={str(not failures).lower()}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
