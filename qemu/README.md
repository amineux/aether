# Path A: QEMU `aether-accel` device

Optional custom QEMU device that exposes the **frozen** virtqueue BAR
from [docs/ACCEL.md](../docs/ACCEL.md). SoftNPU (I32 Nop / MatMul / Wave;
Add / Relu additive on the same engine) sits behind the doorbell and
DMA-reads tensor addresses from
`AccelJobWire`.

**Path B is still the canonical demo.** `make qemu` uses stock QEMU and
the in-kernel `AccelMmio` BAR. Soft SMMU, identity islands, mmap, and
KPTI are unchanged. Do not invent a second BAR.

This is **not** an upstream virtio device.

## Why this is path A, not path B

Path B (`SoftNpuDevice`) is the in-kernel BAR array: the kernel CPU
takes the avail ring and SoftNPU loads guest PA on SoftNPU ssid 0.
Path A is this PCI BAR device as the **DMA initiator**. The host
contract proves the job wire carries Soft-SMMU IOVAs (not identity
GPAs), DMA walks path-A `stream_id` (ssid 4), and a wrong SID aborts.
A kernel PCI bind to BAR0 is **not** required for that proof and is
not landed. Soft SMMU stays software. This device is not a QEMU IOMMU.

## What CI covers

| Command | What it proves |
| --- | --- |
| `make accel-test` | Host unit test of the device model (BAR offsets, doorbell, I32 DMA, used-ring IRQ, Soft-SMMU IOVA + wrong-SID abort). **This is what CI runs.** |
| `cargo test -p aether-drivers --lib path_a` | Same IOVA / bind / wrong-SID contract on the real `IommuMap` (`PathABar`). Workspace `cargo test` covers it. |
| `make qemu-accel` | `accel-test` + `path_a` tests. If `QEMU_ACCEL` is unset, it stops there and prints how to build QEMU. |
| `make qemu-accel` with `QEMU_ACCEL` | Attaches `-device aether-accel` to the stock guest. Guest SoftNPU stays path B. Fails if the binary does not know the device. |

CI does **not** rebuild QEMU. A full softmmu build is optional and heavy.

## Host test (always)

```bash
make accel-test
make qemu-accel   # same, plus the Rust PathABar contract
```

Uses only `gcc` for `accel-test`. No QEMU headers. The test owns a fake
guest RAM, writes an `AccelJobWire` into the BAR, kicks the doorbell,
and checks `C = A @ B` for the same 2×2 I32 the path-B golden uses.
A second clip pins through a software IOVA window (`0x1_0000_0000`,
ssid 4), proves the wire is not identity, and refuses DMA on SoftNPU
ssid 0.

## Build the softmmu device (optional)

Need a QEMU 8.2+ or 9.x tree (Ubuntu 24.04's `qemu-8.2` is the
reference). From this directory:

```bash
export QEMU_SRC=/path/to/qemu
./install-into-qemu.sh
cd "$QEMU_SRC"
./configure --target-list=x86_64-softmmu
ninja -C build
export QEMU_ACCEL="$QEMU_SRC/build/qemu-system-x86_64"
```

`install-into-qemu.sh` copies `aether_accel.h`, `aether_accel.c`, and
`aether_accel_pci.c` into `$QEMU_SRC/hw/misc/` and appends the
`meson.snippet` line if it is missing.

Then, from the Aether tree:

```bash
make qemu-accel
```

QEMU flags added when `QEMU_ACCEL` is set:

```text
-device aether-accel
```

PCI vendor `0xAE7E`, device `0xACC1`, 1 KiB memory BAR0. The guest
does not bind a kernel driver to that BAR — SoftNPU submit still
goes through the in-kernel window so identity teardown / Soft SMMU /
KPTI stay on the path-B code you already boot. The device is present
and the model is what a guest driver would DMA. IOVA / wrong-SID is
the host contract above, not a QEMU rebuild.

A later kernel cut can `pci_dma` the same offsets without changing
fabric or caps.

## BAR (frozen; same as path B)

```text
0x00  magic       0xAE7EACC1
0x04  version     1
0x08  status      ACK | DRIVER | DRIVER_OK | FAILED
0x0C  qsize       8
0x10  doorbell    write 1 = kick (device clears after service)
0x14  used_idx    device-updated
0x18  irq_status  bit0 = used-ring IRQ
0x1C  irq_ack     driver write 1 to ack
0x20  avail_idx   driver-updated
0x80  avail[8]    AccelJobWire (88 bytes)
0x340 used[8]     { job_seq, status, cycles }
```

F16 / F32 jobs complete with status `-1` on this device. Software IEEE
stays the in-kernel SoftNPU (path B). Host tests put Soft-SMMU IOVAs
in the job wire and translate under DMA. The QEMU `-device` still
`pci_dma`s the published address — not a QEMU IOMMU.
