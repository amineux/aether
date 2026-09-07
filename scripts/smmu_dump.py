#!/usr/bin/env python3
"""Pretty-print a Soft SMMU software-table dump (STE → CD → S1/S2 + ATC).

Reads docs/bringup/smmu_dump.golden.json by default. This is not a hardware
SMMU, not an SMMUv3 emulator, and not a reimplementation of IommuMap.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT = ROOT / "docs" / "bringup" / "smmu_dump.golden.json"


def load_dump(path: pathlib.Path) -> dict:
    data = json.loads(path.read_text())
    if data.get("kind") != "aether-soft-smmu-dump":
        raise SystemExit(f"{path}: expected kind aether-soft-smmu-dump")
    if not data.get("software_tables_only"):
        raise SystemExit(f"{path}: dump must set software_tables_only")
    if not data.get("not_hardware_smmu") or not data.get("not_smmuv3_emulator"):
        raise SystemExit(f"{path}: dump must keep the hardware non-claims")
    return data


def fmt_walk(dump: dict) -> str:
    lines = [
        "Soft SMMU dump (software tables only; not silicon)",
        f"  atc_hits={dump.get('atc_hits')}  atc_misses={dump.get('atc_misses')}",
    ]
    for ste in dump.get("stes") or []:
        lines.append(
            f"STE slot={ste.get('slot')} key={ste.get('key')} "
            f"state={ste.get('state')} config={ste.get('config')} "
            f"distinct_ipa={ste.get('distinct_ipa')} s2_vmid={ste.get('s2_vmid')}"
        )
        for cd in ste.get("cds") or []:
            s1 = cd.get("s1") or []
            lines.append(
                f"  CD slot={cd.get('cd_slot')} ssid={cd.get('ssid')} "
                f"valid={cd.get('valid')} asid={cd.get('asid')} s1_ptes={len(s1)}"
            )
            for pte in s1:
                lines.append(
                    f"    S1  {pte.get('va')} → IPA {pte.get('out')} "
                    f"len={pte.get('len')} wr={pte.get('writable')}"
                )
        for pte in ste.get("s2") or []:
            lines.append(
                f"  S2  IPA {pte.get('va')} → PA {pte.get('out')} "
                f"len={pte.get('len')} wr={pte.get('writable')}"
            )
    for r in dump.get("regions") or []:
        lines.append(
            f"pin  pa={r.get('guest_pa')} iova={r.get('iova')} "
            f"sid={r.get('stream_id')} len={r.get('len')}"
        )
    atc = dump.get("atc") or []
    if not atc:
        lines.append("ATC  (empty)")
    for line in atc:
        lines.append(
            f"ATC  sid={line.get('sid')} {line.get('iova')} → PA {line.get('pa')} "
            f"len={line.get('len')}"
        )
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "dump",
        nargs="?",
        default=str(DEFAULT),
        help="path to aether-soft-smmu-dump JSON",
    )
    ap.add_argument(
        "--check",
        action="store_true",
        help="validate non-claims + STE→CD→S1/S2 shape; no pretty-print",
    )
    args = ap.parse_args()
    path = pathlib.Path(args.dump)
    dump = load_dump(path)
    stes = dump.get("stes") or []
    if not stes:
        raise SystemExit(f"{path}: no STEs")
    nested = any(s.get("config") == "Nested" for s in stes)
    has_s1 = any((cd.get("s1") or []) for s in stes for cd in (s.get("cds") or []))
    has_s2 = any(s.get("s2") for s in stes)
    if not (nested and has_s1 and has_s2):
        raise SystemExit(f"{path}: expected Nested STE with S1 and S2 blocks")
    if args.check:
        print(f"ok  {path}  stes={len(stes)} nested={nested} s1={has_s1} s2={has_s2}")
        return 0
    sys.stdout.write(fmt_walk(dump) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
