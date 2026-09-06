#!/usr/bin/env python3
"""Append a trampoline trailer that names .rela.dyn inside kernel.bin.

The x86_64 kernel is a static-PIE. objcopy -O binary keeps .rela.dyn in
the blob; this script writes a 16-byte footer so the 64-bit trampoline
can walk Elf64_Rela without parsing the ELF header:

    magic (0xAE7E4E1C), count, rela_off, entsize (24)

rela_off is the objcopy offset (LMA - 0x400000). Only R_X86_64_RELATIVE
is accepted — anything else is a hard error so we do not ship a table
the trampoline would skip.
"""

from __future__ import annotations

import struct
import sys

KERNEL_VMA = 0xFFFFFFFF80000000
KERNEL_LMA = 0x400000
SHT_RELA = 4
R_X86_64_RELATIVE = 8
PIE_RELOC_MAGIC = 0xAE7E4E1C
RELA64_SIZE = 24


def pack(elf_path: str, bin_path: str) -> int:
    elf = open(elf_path, "rb").read()
    if elf[:4] != b"\x7fELF" or elf[4] != 2:
        raise SystemExit("expected ELF64")
    e_shoff = struct.unpack_from("<Q", elf, 40)[0]
    e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHH", elf, 58)
    shstr_off = struct.unpack_from("<Q", elf, e_shoff + e_shstrndx * e_shentsize + 24)[0]

    rela = None
    for i in range(e_shnum):
        base = e_shoff + i * e_shentsize
        name_off, sh_type, _flags, sh_addr, sh_offset, sh_size, _l, _i, _a, sh_entsize = (
            struct.unpack_from("<IIQQQQIIQQ", elf, base)
        )
        end = elf.index(b"\0", shstr_off + name_off)
        name = elf[shstr_off + name_off : end].decode()
        if name == ".rela.dyn" and sh_type == SHT_RELA:
            rela = (sh_addr, sh_offset, sh_size, sh_entsize)
            break
    if rela is None:
        raise SystemExit("no .rela.dyn — kernel was not linked PIC")
    sh_addr, sh_offset, sh_size, sh_entsize = rela
    if sh_entsize != RELA64_SIZE or sh_size % RELA64_SIZE != 0:
        raise SystemExit(f"bad .rela.dyn entsize={sh_entsize} size={sh_size}")
    count = sh_size // RELA64_SIZE
    if count == 0:
        raise SystemExit(".rela.dyn is empty")

    blob = open(bin_path, "rb").read()
    rela_off = sh_addr - KERNEL_VMA - KERNEL_LMA
    if rela_off < 0 or rela_off + sh_size > len(blob):
        raise SystemExit(
            f".rela.dyn not in kernel.bin (off={rela_off:#x} size={sh_size:#x} bin={len(blob):#x})"
        )
    if blob[rela_off : rela_off + sh_size] != elf[sh_offset : sh_offset + sh_size]:
        raise SystemExit("kernel.bin .rela.dyn does not match the ELF")

    for j in range(count):
        off, info, addend = struct.unpack_from("<QQq", elf, sh_offset + j * RELA64_SIZE)
        kind = info & 0xFFFFFFFF
        if kind != R_X86_64_RELATIVE:
            raise SystemExit(f"unsupported reloc type {kind} at {off:#x}")
        site = off - KERNEL_VMA - KERNEL_LMA
        if site < 0 or site + 8 > len(blob):
            raise SystemExit(f"reloc site {off:#x} outside kernel.bin")
        uadd = addend & 0xFFFFFFFFFFFFFFFF
        if uadd < KERNEL_VMA:
            raise SystemExit(f"addend {uadd:#x} is not a HH VA")

    trailer = struct.pack("<IIII", PIE_RELOC_MAGIC, count, rela_off, RELA64_SIZE)
    open(bin_path, "wb").write(blob + trailer)
    print(
        f"kernel.bin {len(blob) + len(trailer)} bytes "
        f"(.rela.dyn n={count} off={rela_off:#x})"
    )
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <kernel.elf> <kernel.bin>", file=sys.stderr)
        sys.exit(2)
    sys.exit(pack(sys.argv[1], sys.argv[2]))
