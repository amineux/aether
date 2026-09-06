#!/usr/bin/env python3
"""Pack named files into an AETHFS01 raw disk image (virtio-blk → ramfs).

Layout matches `core/src/bootfs.rs` (little-endian):

    0x00  magic     8  b"AETHFS01"
    0x08  nfiles    4
    0x0C  flags     4  0
    0x10  entries[n]
            name    16  NUL-padded
            offset  4
            size    4
    then payloads; image padded to 512 bytes.
"""

from __future__ import annotations

import argparse
import pathlib
import struct
import sys

MAGIC = b"AETHFS01"
HDR = 16
ENT = 24
NAME = 16
SECTOR = 512
MAX_FILES = 8


def pack(files: list[tuple[str, bytes]]) -> bytes:
    if not files:
        raise SystemExit("need at least one file")
    if len(files) > MAX_FILES:
        raise SystemExit(f"too many files (max {MAX_FILES})")
    dir_end = HDR + len(files) * ENT
    cursor = dir_end
    entries = []
    for name, data in files:
        if not name.startswith("/") or len(name) < 2 or len(name) > NAME:
            raise SystemExit(f"bad name {name!r}")
        entries.append((name, cursor, data))
        cursor += len(data)
    padded = (cursor + SECTOR - 1) & ~(SECTOR - 1)
    out = bytearray(padded)
    out[0:8] = MAGIC
    struct.pack_into("<I", out, 8, len(files))
    struct.pack_into("<I", out, 12, 0)
    for i, (name, off, data) in enumerate(entries):
        e = HDR + i * ENT
        raw = name.encode("ascii")
        out[e : e + len(raw)] = raw
        struct.pack_into("<I", out, e + 16, off)
        struct.pack_into("<I", out, e + 20, len(data))
        out[off : off + len(data)] = data
    return bytes(out)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", required=True, help="raw image path")
    ap.add_argument(
        "files",
        nargs="+",
        help="name=path pairs (e.g. /init=build/init.elf)",
    )
    args = ap.parse_args()
    packed: list[tuple[str, bytes]] = []
    for item in args.files:
        if "=" not in item:
            raise SystemExit(f"expected name=path, got {item!r}")
        name, path = item.split("=", 1)
        data = pathlib.Path(path).read_bytes()
        if not data:
            raise SystemExit(f"{path} is empty")
        packed.append((name, data))
    img = pack(packed)
    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(img)
    print(f"bootfs {out} {len(img)} bytes files={len(packed)}", file=sys.stderr)


if __name__ == "__main__":
    main()
