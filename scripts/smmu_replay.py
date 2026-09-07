#!/usr/bin/env python3
"""Replay helper for the Soft SMMU bring-up kit.

Prints docs/bringup/smmu_replay.jsonl (map / translate / abort-until-bound /
wrong-stream / ATS invalidate). Does not reimplement IommuMap: the host test
`smmu_bringup::replay_jsonl` executes the same file. Optional `--run-tests`
feeds that sequence into cargo.

Software tables only. Not a hardware SMMU. Not a Soft-SMMU redo.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "docs" / "bringup" / "smmu_replay.jsonl"
GOLDEN = ROOT / "docs" / "bringup" / "smmu_dump.golden.json"
DUMP_PY = ROOT / "scripts" / "smmu_dump.py"

ALLOWED_OPS = {
    "note",
    "capture",
    "bind_stream",
    "bind_nested",
    "map",
    "translate",
    "walk",
    "resolve_ats",
    "invalidate",
    "dump",
}

REQUIRED_MARKERS = (
    "capture",
    "map",
    "translate",
    "walk",
    "invalidate",
    "StreamAbort",
    "WrongStream",
    "NoMemoryCap",
    "Ats",
    "bind_nested",
)


def load_jsonl(path: pathlib.Path) -> list[dict]:
    steps = []
    for i, raw in enumerate(path.read_text().splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError as e:
            raise SystemExit(f"{path}:{i}: {e}") from e
        if not isinstance(obj, dict) or "op" not in obj:
            raise SystemExit(f"{path}:{i}: expected a JSON object with op")
        if obj["op"] not in ALLOWED_OPS:
            raise SystemExit(f"{path}:{i}: unknown op {obj['op']!r}")
        steps.append(obj)
    return steps


def fmt_steps(steps: list[dict]) -> str:
    lines = [
        "Soft SMMU bring-up replay (software tables; host test executes this file)",
        "STE → CD → S1/S2 + ATS invalidate; AccelDevice::map / Soft-CP SID bind",
        "",
    ]
    for obj in steps:
        step = obj.get("step", "?")
        op = obj["op"]
        if op == "note":
            lines.append(f"{step:>3}  note  {obj.get('text', '')}")
            continue
        bits = [f"{step:>3}  {op}"]
        for key in (
            "sid",
            "guest_pa",
            "addr",
            "iova",
            "cmd",
            "with_cap",
            "expect",
            "expect_pa",
            "expect_config",
            "expect_dropped",
        ):
            if key in obj:
                bits.append(f"{key}={obj[key]}")
        lines.append("  ".join(bits))
    return "\n".join(lines)


def check_markers(text: str) -> None:
    missing = [m for m in REQUIRED_MARKERS if m not in text]
    if missing:
        raise SystemExit(f"{SCRIPT}: missing kit markers {missing}")


def run_tests() -> int:
    cmds = [
        ["cargo", "test", "-p", "aether-core", "--lib", "smmu_bringup"],
        ["cargo", "test", "-p", "aether-drivers", "--lib", "fakecp", "--", "bringup"],
    ]
    for cmd in cmds:
        print("+", " ".join(cmd), flush=True)
        r = subprocess.run(cmd, cwd=ROOT)
        if r.returncode != 0:
            return r.returncode
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--script",
        default=str(SCRIPT),
        help="JSONL replay script",
    )
    ap.add_argument(
        "--check",
        action="store_true",
        help="validate JSONL + golden dump; do not print the walk",
    )
    ap.add_argument(
        "--run-tests",
        action="store_true",
        help="after checks, run the host-test replay (feeds cargo test)",
    )
    args = ap.parse_args()
    path = pathlib.Path(args.script)
    steps = load_jsonl(path)
    text = path.read_text()
    check_markers(text)
    dump_check = subprocess.run(
        [sys.executable, str(DUMP_PY), str(GOLDEN), "--check"],
        cwd=ROOT,
    )
    if dump_check.returncode != 0:
        return dump_check.returncode
    ops = [s["op"] for s in steps if s["op"] != "note"]
    if args.check:
        print(
            f"ok  {path.name}  steps={len(steps)} ops={ops}  golden={GOLDEN.name}"
        )
    else:
        sys.stdout.write(fmt_steps(steps) + "\n")
        print(f"\n{len(steps)} JSONL lines. Host test: cargo test -p aether-core smmu_bringup")
    if args.run_tests:
        return run_tests()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
