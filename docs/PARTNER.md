# Partner hello (clone and run, no QEMU)

A silicon or compiler team can clone this tree and run a frozen IREE
HAL dispatch on the host. There is **no QEMU rebuild**. Path B (stock
`make qemu`, in-kernel SoftNPU BAR) stays the canonical guest demo.

```bash
make partner-hello
# or: cargo run -p aether-partner-hello
# or: cargo test -p aether-partner-hello
```

This is **not** a signed vendor, a PJRT plugin, hardware SMMU, FLOPs,
tape-out, or an NVIDIA partnership. `PartnerNpuStub` is a leftover
no-op sketch, not this path.

## What you get

| Surface | Honest reading |
| --- | --- |
| Frozen packet | 96-byte little-endian [`IreeHalCmd`](../drivers/src/ireecp.rs). Magic `0xAE7E1EE1`. Offsets in [ACCEL.md](ACCEL.md) are **frozen** — do not change them here. |
| Backend | `IreeShapedCp` (`backend = 4`). Reserved: 0 SoftNPU, 1 virtqueue SoftNPU, 2 `PartnerNpuStub`, 3 Soft-CP. Pick a **new** id for a real chip. |
| Executable | Opaque `isa_blob_id` `0x0001EE00` (`IREE_REF_EXECUTABLE`). Any other id is refused. The kernel does not parse IREE VM bytecode. |
| Soft SMMU SID | Packet `stream_id` packs `ssid = 2` (`IREE_SSID`). Software STE→CD→Stage-1/2 walk. IOVA is not identity. Not a hardware SMMU. |
| PJRT nouns | `host/aether-pjrt` maps Client / Device / Buffer / Executable / Event onto that packet and submits through `IreeShapedCp`. Not `GetPjRtApi`. |
| Research marker | `AccelInfo.vendor = 0xAE7E` is an Aether marker, **not** a silicon vendor ID. Do not invent one in this tree. |

The example prints every packet field (ACCEL.md offsets), a 2×2 I32
matmul result `[19, 22, 43, 50]`, a refused `0xDEAD` executable, and
the one-screen `AccelDevice` plug-in (same steps as below).

## How to plug a backend

```text
1. PCI / MMIO / NoC probe. Fill AccelInfo { backend: YOUR_ID, vendor: YOUR_CHIP, … }.
   Do not reuse 0–4. Do not invent a vendor ID in this repository.
2. Implement aether_hal::AccelDevice { probe, submit, poll, map }.
   IreeShapedCp is the partner-shaped IREE HAL packet (this example).
   SoftCommandProcessor is the Aether-native CpCmd path.
3. map(): bind_stream + pin from a Memory+MAP cap walk. Soft SMMU
   (`IommuMap`) is a software table. A hardware SMMU is still required
   on silicon; a real device can DMA past this walk.
4. submit(): pack AccelJobDesc into the chip packet. IreeShapedCp uses
   the frozen 96-byte IreeHalCmd on a single mailbox. Doorbell. Do not
   execute in the syscall / submit path.
5. IRQ: AccelDevice::poll, retire the fence through Timeline::complete
   / retire_into. The timeline is a software model.
```

Compilers own the ISA blob. Aether admits the job against a partition,
a SpectralCut, a bank color, and a fence. It does not fuse a graph.

Walkthrough: [ACCEL.md](ACCEL.md), [HOST.md](HOST.md), [ABI.md](ABI.md),
[DILIGENCE.md](DILIGENCE.md). Code: `examples/partner-hello`,
`host/aether-pjrt`, `drivers/src/ireecp.rs`.

## Non-claims

We will not claim:

- A signed silicon vendor, a design win, or a partnership with NVIDIA
  or any ASIC / compiler house
- That `aether-pjrt` is an OpenXLA PJRT plugin (`GetPjRtApi`) or an
  in-tree IREE HAL driver
- That Soft SMMU / `IommuMap` is a hardware SMMU
- FLOPs, benchmarks, or a tape-out checklist
- That Path A (`qemu/aether-accel`) is required to try this packet —
  this example is host-only; Path B stays what stock `make qemu` runs
- That `PartnerNpuStub` is this path

`IreeHalCmd` offsets stay frozen. Changing an offset is a dual
`ireecp.rs` + [ACCEL.md](ACCEL.md) + host pack/unpack update, not a
hello-world edit.
