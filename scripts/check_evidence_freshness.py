#!/usr/bin/env python3
"""Fail when investor-facing docs cite a commit that is stale or not on main.

Gated files: docs/SCOREBOARD.md and docs/business/*.md. A cited commit is any
7-40 character lowercase hex token that is not an elided hash (``abc1234…``).

Allowed-lag rule (see docs/business/INVESTOR_EVIDENCE.md):

1. Every cited commit must exist and be an ancestor of the evidence commit
   (a typo, a branch-only SHA or a rebased-away SHA fails).
2. Every cited commit must be at most MAX_LAG first-parent commits behind the
   evidence commit. On main one first-parent commit is one merged PR, so a
   docs-only merge moves the counter by one; it never fails on its own.
3. A GitHub Actions run link (``/actions/runs/<id>``) must sit on a line that
   also cites a commit, so rule 1 and 2 cover it without an API call.

The evidence commit is the ``commit_sha`` recorded by
``scripts/collect_evidence.py`` (``--manifest``), or HEAD when no manifest is
given: CI's host-evidence job records the same SHA the workflow checks out.
A line containing ``evidence-lag: historical`` is exempt (for commits cited
as history, not as the measured state).

CI runs this as the ``evidence-freshness`` check inside
``scripts/collect_evidence.py`` (host-evidence job). That checkout is shallow,
so a shallow clone is unshallowed first (public fetch, no credentials).
"""
import argparse
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MAX_LAG = 20
GATED = ["docs/SCOREBOARD.md", "docs/business/*.md"]
HEX = re.compile(r"(?<![0-9A-Za-z_])([0-9a-f]{7,40})(?![0-9A-Za-z_…])")
RUN_LINK = re.compile(r"/actions/runs/\d+")
EXEMPT = "evidence-lag: historical"


def git(*args):
    result = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
    return result.returncode, result.stdout.strip()


def citations(root=ROOT):
    """Yield (path, line_no, line, [sha tokens]) for gated docs."""
    for pattern in GATED:
        for path in sorted(root.glob(pattern)):
            rel = path.relative_to(root).as_posix()
            for no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if EXEMPT in line:
                    continue
                # A token needs a digit and a letter, so plain words and plain
                # numbers are never mistaken for hashes.
                shas = [t for t in HEX.findall(line)
                        if re.search(r"\d", t) and re.search(r"[a-f]", t)]
                if shas or RUN_LINK.search(line):
                    yield rel, no, line, shas


def first_parent_lag(sha, evidence):
    """First-parent commits from `sha` to `evidence`, or None if not an ancestor."""
    code, full = git("rev-parse", "--verify", "--quiet", f"{sha}^{{commit}}")
    if code != 0:
        return None, "not a commit in this clone"
    code, _ = git("merge-base", "--is-ancestor", full, evidence)
    if code != 0:
        return None, f"not an ancestor of evidence commit {evidence[:7]}"
    # Count first-parent steps until we reach a commit that contains `sha`.
    code, out = git("rev-list", "--first-parent", f"{full}..{evidence}")
    if code != 0:
        return None, "rev-list failed"
    behind = [c for c in out.splitlines() if c]
    lag = 0
    for commit in behind:
        code, _ = git("merge-base", "--is-ancestor", full, f"{commit}^")
        if code != 0:
            break
        lag += 1
    return lag, None


def ensure_history():
    """CI checks out one commit; the lag rule needs the history behind it."""
    code, shallow = git("rev-parse", "--is-shallow-repository")
    if code == 0 and shallow == "true":
        code, _ = git("fetch", "--quiet", "--unshallow", "origin")
        if code != 0:
            raise SystemExit("[evidence-freshness] shallow clone and unshallow fetch failed")


def evidence_commit(manifest):
    if manifest:
        data = json.loads(pathlib.Path(manifest).read_text(encoding="utf-8"))
        sha = data.get("commit_sha", "")
        if not re.fullmatch(r"[0-9a-f]{40}", sha):
            raise SystemExit(f"{manifest}: no 40-hex commit_sha recorded")
        return sha
    code, sha = git("rev-parse", "HEAD")
    if code != 0:
        raise SystemExit("git rev-parse HEAD failed")
    return sha


def check(evidence, max_lag=MAX_LAG, root=ROOT):
    errors, seen = [], 0
    for rel, no, line, shas in citations(root):
        if RUN_LINK.search(line) and not shas:
            errors.append(f"{rel}:{no}: CI run link without a commit SHA on the same line")
        for sha in shas:
            seen += 1
            lag, why = first_parent_lag(sha, evidence)
            if lag is None:
                errors.append(f"{rel}:{no}: {sha}: {why}")
            elif lag > max_lag:
                errors.append(f"{rel}:{no}: {sha} is {lag} merges behind {evidence[:7]} "
                              f"(allowed lag {max_lag}); re-measure and refresh it")
            else:
                print(f"[evidence-freshness] {rel}:{no} {sha} lag={lag}")
    return seen, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", help="manifest.json from collect_evidence.py")
    parser.add_argument("--max-lag", type=int, default=MAX_LAG)
    parser.add_argument("--self-test", action="store_true",
                        help="run scripts/test_evidence_freshness.py first")
    args = parser.parse_args()
    if args.self_test:
        result = subprocess.run([sys.executable, "-m", "unittest", "-q",
                                 "test_evidence_freshness"], cwd=ROOT / "scripts")
        if result.returncode != 0:
            return result.returncode
    ensure_history()
    evidence = evidence_commit(args.manifest)
    seen, errors = check(evidence, args.max_lag)
    for error in errors:
        print(f"[evidence-freshness] FAIL {error}", file=sys.stderr)
    print(f"[evidence-freshness] evidence={evidence[:7]} cited={seen} "
          f"max_lag={args.max_lag} stale={len(errors)}")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
