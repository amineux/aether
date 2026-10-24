#!/usr/bin/env python3
"""Run every runnable row of docs/business/PROVE_IT.md and grep its expected line.

The table is the source of truth: each row's `make <target>` (or cargo command)
is mapped to the same recipe the Makefile runs, so this works without GNU make.
Rows without a command (blocked) are reported and skipped. QEMU and Kani are
never run here.
"""
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
DOC = ROOT / "docs/business/PROVE_IT.md"
KIT = "examples/isolation-kit/Cargo.toml"

# make target -> shell recipe (mirrors the Makefile; build/ is gitignored).
RECIPES = {
    "make eval-run": "cargo run --locked --quiet -p aether-eval-run -- --json build/eval-run.json",
    "make verify": "python3 scripts/verify.py",
    "make red-team": "cargo run --locked --release --quiet -p aether-redteam",
    "make red-team-json": (
        "mkdir -p build && cargo run --locked --release --quiet -p aether-redteam > build/redteam.log && "
        "cargo run --locked --release --quiet -p aether-redteam -- --json < build/redteam.log > build/redteam.json"),
    "make noninterference": (
        "cargo run --locked --release -p aether-redteam --quiet --example noninterference -- 512 16"),
    "make isolation-matrix": (
        f"cargo test --locked --manifest-path {KIT} --release --quiet && "
        f"cargo run --locked --manifest-path {KIT} --release --quiet -- aether-soft"),
    "make evidence-pack": "python3 scripts/evidence_pack.py",
}


def rows(text):
    out = []
    for line in text.splitlines():
        if not line.startswith("| ") or line.startswith("| Claim") or line.startswith("|---"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) != 4:
            continue
        cmd = re.fullmatch(r"`([^`]+)`", cells[1])
        exp = re.fullmatch(r"`([^`]+)`", cells[2])
        out.append((cells[0], cmd.group(1) if cmd else None, exp.group(1) if exp else None))
    return out


def main():
    table = rows(DOC.read_text(encoding="utf-8"))
    if len(table) < 8:
        print(f"[prove-it] table has {len(table)} rows, expected 8", file=sys.stderr)
        return 1
    failures, ran, skipped = [], 0, 0
    for claim, cmd, expected in table:
        if cmd is None or expected is None:
            skipped += 1
            print(f"[prove-it] skip (blocked) claim={claim!r}")
            continue
        recipe = RECIPES.get(cmd, cmd if cmd.startswith("cargo ") else None)
        if recipe is None:
            failures.append(f"no recipe for {cmd!r}")
            continue
        res = subprocess.run(recipe, shell=True, cwd=ROOT, capture_output=True, text=True,
                             encoding="utf-8", errors="replace")
        output = res.stdout + res.stderr
        ok = res.returncode == 0 and re.search(expected, output, re.M) is not None
        ran += 1
        print(f"[prove-it] {'ok  ' if ok else 'FAIL'} {cmd}")
        if not ok:
            failures.append(f"{cmd}: exit={res.returncode} expected={expected!r}\n{output[-2000:]}")
    print(f"[prove-it] rows={len(table)} ran={ran} skipped={skipped} failed={len(failures)}")
    for f in failures:
        print(f, file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
