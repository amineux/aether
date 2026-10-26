#!/usr/bin/env python3
"""Leave-behind evidence pack: one dated, deterministic tarball of host clips.

`make evidence-pack` runs the red-team, noninterference, isolation-matrix and
diligence-demo clips on the current checkout and bundles their stdout with the
commit SHA, the toolchain versions and the "does not offer" block from
docs/business/DISCOVERY.md into
`build/evidence-pack/aether-evidence-<YYYY-MM-DD>-<sha12>.tar.gz`.

Only the file name carries the date. The archive is built deterministically
(sorted members, fixed mtime from the commit, uid/gid 0, no user names, gzip
header mtime 0), so two runs on the same commit and toolchain produce the same
bytes. Host software clips only: no customer validation, no hardware, no
throughput numbers.
"""
import argparse
import datetime
import gzip
import hashlib
import io
import pathlib
import re
import subprocess
import sys
import tarfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
DISCOVERY = ROOT / "docs" / "business" / "DISCOVERY.md"
NI_SEEDS = "512"
NI_OPS_PER_GAP = "16"

# (file in the pack, command, regexes that must all match stdout)
CLIPS = [
    ("redteam.log",
     ["cargo", "run", "--locked", "--release", "--quiet", "-p", "aether-redteam"],
     [r"^\[redteam\] sealed$"]),
    ("noninterference.log",
     ["cargo", "run", "--locked", "--release", "--quiet", "-p", "aether-redteam",
      "--example", "noninterference", "--", NI_SEEDS, NI_OPS_PER_GAP],
     [r"^\[noninterference\] worlds=2 seeds=" + NI_SEEDS + r" ops=\d+ divergences=0$"]),
    ("isolation-matrix.log",
     ["cargo", "run", "--locked", "--manifest-path", "examples/isolation-kit/Cargo.toml",
      "--release", "--quiet"],
     [r"^\[isolation-kit\] backend=aether-soft .* accepted=0 .* result=conformant$",
      r"^\[isolation-kit\] backend=weak-sample-example-only .* result=NONCONFORMANT$"]),
    ("diligence-demo.log",
     ["cargo", "run", "--locked", "--quiet", "-p", "aether-diligence-demo",
      "--bin", "diligence-demo"],
     []),
]

README = """# Aether evidence pack

Commit: {sha}
Commit date (UTC): {commit_date}

This archive is a leave-behind from a technical conversation. Every file was
produced on the commit above by host software models (`make evidence-pack`).
It is not hardware isolation, not a benchmark, not certification and not a
customer or partner result.

| File | What it is | Re-run on any checkout of the commit |
| --- | --- | --- |
| redteam.log | Named-attack refusal clip ({refused} `result=refused` lines) | `make red-team` |
| noninterference.log | Two-world noninterference replay and negative controls | `make noninterference` |
| isolation-matrix.log | 15-class isolation-kit matrix (reference + weak sample) | `make isolation-matrix` |
| diligence-demo.log | Host Path B diligence clip | `make diligence-demo` |
| COMMIT.txt | Commit SHA and whether the tree had local changes | `git rev-parse HEAD` |
| TOOLCHAIN.txt | rustc / cargo / python versions used for this pack | `rustc -vV` |
| NOT_PROVEN.md | What this evidence does not show | docs/business/DISCOVERY.md |
| SHA256SUMS | sha256 of every other file in the pack | `sha256sum -c SHA256SUMS` |

Only the archive file name carries a date. Two runs on the same commit and
toolchain give byte-identical archives.
"""


def sh(cmd):
    return subprocess.run(cmd, cwd=ROOT, check=True, capture_output=True,
                          text=True).stdout


def not_offered_block(text):
    """Return DISCOVERY's 'What it does **not** offer' paragraph verbatim."""
    m = re.search(r"^What it does \*\*not\*\* offer:.*?(?=\n\s*\n|\Z)", text,
                  re.M | re.S)
    if not m:
        raise SystemExit("evidence-pack: DISCOVERY.md has no 'does not offer' block")
    return m.group(0).strip()


def not_proven_md(block):
    return (
        "# Not proven by this pack\n\n"
        "From docs/business/DISCOVERY.md (verbatim):\n\n"
        f"> {block.replace(chr(10), chr(10) + '> ')}\n\n"
        "Also not shown: customer or partner validation, silicon results,\n"
        "hardware SMMU behaviour, or any performance number. Everything here is\n"
        "a host software model run on the commit in COMMIT.txt.\n"
    )


def deterministic_tar_gz(files, mtime):
    """Pack {name: bytes} into gzip bytes with sorted, owner-free members."""
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        for name in sorted(files):
            data = files[name]
            info = tarfile.TarInfo(name=f"aether-evidence/{name}")
            info.size = len(data)
            info.mtime = mtime
            info.mode = 0o644
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            tar.addfile(info, io.BytesIO(data))
    out = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=out, mtime=0,
                       compresslevel=9) as gz:
        gz.write(raw.getvalue())
    return out.getvalue()


def run_clip(name, cmd, needles):
    proc = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    out = proc.stdout
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"evidence-pack: {name} exited {proc.returncode}")
    for needle in needles:
        if not re.search(needle, out, re.M):
            raise SystemExit(f"evidence-pack: {name} missing line /{needle}/")
    return out


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out-dir", default="build/evidence-pack")
    ap.add_argument("--date", help="YYYY-MM-DD for the file name (default: today, UTC)")
    args = ap.parse_args(argv)

    date = args.date or datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    datetime.date.fromisoformat(date)
    sha = sh(["git", "rev-parse", "HEAD"]).strip()
    commit_ts = int(sh(["git", "show", "-s", "--format=%ct", "HEAD"]).strip())
    commit_date = datetime.datetime.fromtimestamp(commit_ts, datetime.timezone.utc)
    dirty = bool(sh(["git", "status", "--porcelain", "--untracked-files=no"]).strip())

    files = {}
    for name, cmd, needles in CLIPS:
        files[name] = run_clip(name, cmd, needles).encode()
    refused = files["redteam.log"].decode().count("result=refused")

    files["COMMIT.txt"] = f"sha={sha}\ndirty={'true' if dirty else 'false'}\n".encode()
    toolchain = sh(["rustc", "-vV"]) + sh(["cargo", "-V"])
    toolchain += f"python {sys.version.split()[0]}\n"
    files["TOOLCHAIN.txt"] = toolchain.encode()
    files["NOT_PROVEN.md"] = not_proven_md(
        not_offered_block(DISCOVERY.read_text(encoding="utf-8"))).encode()
    files["README.md"] = README.format(
        sha=sha, commit_date=commit_date.strftime("%Y-%m-%d %H:%M:%S"),
        refused=refused).encode()
    files["SHA256SUMS"] = "".join(
        f"{hashlib.sha256(files[n]).hexdigest()}  {n}\n" for n in sorted(files)).encode()

    blob = deterministic_tar_gz(files, commit_ts)
    out_dir = ROOT / args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / f"aether-evidence-{date}-{sha[:12]}.tar.gz"
    path.write_bytes(blob)

    print(f"[evidence-pack] tarball={path.relative_to(ROOT)} "
          f"sha256={hashlib.sha256(blob).hexdigest()} dirty={'true' if dirty else 'false'}")
    print(f"[evidence-pack] sha={sha} files={len(files)} not_proven=listed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
