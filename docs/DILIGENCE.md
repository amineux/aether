# Diligence pack (research prototype)

This is what a silicon OS team would receive before spending bring-up
time on Aether. It is **not** a partnership announcement, a tape-out
checklist, or a benchmark brief.

## What ships in this tree

| Surface | Status | Where |
| --- | --- | --- |
| Capability fabric + isolation demo | Implemented, host-tested | `core/src/{caps,fabric,demo}.rs` |
| Tensor arenas, typed spaces, `(place, local)` | Implemented | `core/src/{arena,space}.rs` |
| Bank color (Compute refuse / Exchange ok) | Implemented, host-tested | `core/src/color.rs` |
| `IommuMap` Soft SMMU (per-stream, non-identity IOVA) | Implemented, host-tested | `core/src/iommu.rs` |
| Tile scheduler + SpectralCut refuse | Implemented (n≤8 enumerate) | `core/src/{sched,cut}.rs` |
| AffinityLaplacian `L = D − A` | Implemented (integer prototype) | `core/src/laplacian.rs` |
| Hodge flow-class quotas | Implemented | `core/src/hodge.rs` |
| Accel HAL + SoftNPU + virtqueue MMIO | Implemented (in-kernel BAR) | `hal/`, `drivers/`, `core/src/accel.rs` |
| Partner sketch `PartnerNpuStub` | No-op `AccelDevice` | `drivers/src/partner.rs` |
| PJRT/IREE-shaped host nouns | Types only; no graph IR | `core/src/abi.rs`, `docs/ABI.md` |
| x86_64 QEMU + ring-3 `/init` | Working vertical slice | `boot/x86_64/`, `user/init/`, `make qemu` |
| RISC-V virt boot | Thin S-mode port | `boot/riscv64/`, `make qemu-riscv` |

The portable specification is `aether-core`. Host tests execute the same
`run_boot_demo()` the kernels print (caps, fabric, map, color, cut). The
RISC-V port did not change `aether-core` or `aether-hal`.

## What is stubbed

See [ROADMAP.md](ROADMAP.md) for the full STUB table. The diligence-relevant
gaps:

| Gap | Honest reading |
| --- | --- |
| Hardware SMMU | Soft SMMU is software only (chiplet SIDs + capture/bind); a real device can still DMA past it |
| Custom QEMU virtio-accel | In-kernel BAR + SoftNPU; stock QEMU is enough to demo |
| RISC-V is thin | kmain + UART + Sv39 + `aether_core` self-check. No ring-3, no PLIC virtio |
| Fiedler is integer power iteration | Cut construction for n≤8 still enumerates |
| No SMP | Work-steal exists as a data structure |
| No CXL.mem | `MemorySpace::CxlRegion` is a typed place, not a window |
| Cap CDT / revoke | Descendants survive parent revoke |

x86_64 **does** have ring-3 `/init` + `syscall`/`sysret` and cap checks on
send/recv/map/accel. That is not stubbed on x86; it is stubbed on RISC-V.

## How a silicon team plugs `AccelDevice`

```text
1. PCI / MMIO / NoC probe. Fill AccelInfo { backend: 2, vendor, ... }.
2. Implement aether_hal::AccelDevice { probe, submit, poll, map }.
3. map(): program SMMU / stream IDs from a Memory cap walk. Refuse
   anything that did not come from the cap table. Refuse a silent
   remote (place, local) — aether_hal::map_fabric already does.
   IommuMap is the Soft-SMMU table (per-stream IOVA). A hardware SMMU
   is still required on silicon; do not treat this as one.
4. submit(): translate AccelJobDesc (op, MxNxK, strides, dtype, place,
   phase, partition, fence) into the chip's command packet. Doorbell.
   The in-tree virtqueue BAR is the shape to match.
5. IRQ: complete the fence, AccelDevice::poll, fabric REPLY to
   job.completion_ep.
```

Do **not** map all of HBM into the NPU. The arena + cap + color is the point.

The compiler / runtime (IREE, XLA/PJRT, a vendor stack) owns the ISA
blob (`abi::Executable`). Aether admits the job against a partition,
a SpectralCut, a bank color, and a fence. It does not fuse a graph.

Walkthrough: [ACCEL.md](ACCEL.md), [ABI.md](ABI.md). `PartnerNpuStub` is
a no-op sketch, not a partnership.

## Security invariants (what we will defend)

Implemented and host-tested ([SECURITY.md](SECURITY.md)):

1. No cross-tenant mint.
2. Monotonic derive (no right escalation).
3. GRANT required to transfer.
4. Kind + rights checked on use (including from ring-3 on x86).
5. Tenant B does not hold A's Memory / SpectralCut / Activity.
6. Cross-cut tile/bank placement is `CutError::CrossCut`.
7. Harmonic + `TREE_OFFLOAD` is refused (deadlock / homology).
8. `UNIFIED` is never implied by `MEM_FULL`.
9. `IommuMap` refuses a pin without Memory+MAP.
10. Compute waves with a foreign bank color are refused; Exchange may transfer.

Not enforced in hardware yet: SMMU stream IDs, RISC-V ring-3, revocation
broadcast, measured boot. On x86, isolation is “cap tables + ring-3 +
Soft SMMU.” Soft SMMU is a software table a real device can ignore.
On RISC-V it is still “the cap tables do the right thing.”

## CI status

| Job | Command | Intent |
| --- | --- | --- |
| Host tests | `cargo test --workspace` | Caps, fabric, arenas, color, map, sched, SoftNPU, Laplacian, ELF, preempt |
| x86_64 boot | `make qemu-ci` | Ring-3 `/init` + virtqueue demo; isa-debug-exit |
| RISC-V boot | `make qemu-riscv-ci` | OpenSBI S-mode + self-check banner on virt UART |

x86_64 is the supported path. RISC-V CI greps the fabric success
banner and is expected to be green on `qemu-system-riscv64` +
`riscv64gc-unknown-none-elf`. It is a bring-up test, not a
second-architecture product.

## Non-claims

We will not claim:

- Partnerships with NVIDIA, any ASIC house, or any compiler project
- Benchmarks vs Linux / seL4 / CUDA / any NPU SDK
- seL4-level formal proofs
- A CUDA-style unified virtual address space
- Cache coherence across chiplets
- Wafer-scale marketing; tile SRAM is the honest first place
- Readiness for tape-out or safety certification
- That the RISC-V port is a full ring-3 kernel
- That `AffinityLaplacian` is a production eigensolver
- That `IommuMap` is a hardware SMMU
- That `PartnerNpuStub` is a design win

## Design-win narrative

**Why Aether under a vendor compiler / runtime.**

An AI-package OS team already has a compiler. They do not want another
graph IR in the kernel. They want:

1. **A doorbell they can implement once.** `AccelDevice` is probe /
   submit / poll / map. The job descriptor is opcode + shape + typed
   places + fence. Their command processor already has those fields.
   The in-tree virtqueue BAR is the packet shape.
2. **Isolation that is not ioctl folklore.** Weights and KV caches are
   Memory caps with tenants and a bank color. Cross-tenant mint is a
   type error. Soft SMMU gives per-stream non-identity IOVA in software
   — policy and the software table are tested; a hardware SMMU is not
   programmed.
3. **Placement that names the package graph.** A SpectralCut is a
   capability. The Laplacian is a first-class `L = D − A`. Cross-die
   placement is refused because the cut said so, not because a hint
   was ignored.
4. **A kernel that stays out of FLOPs.** Named phases
   (`Compute | Exchange | Barrier`) are tags. Fusion stays in IREE /
   PJRT / the vendor stack. The host ABI is shaped like those runtimes
   on purpose.
5. **A HAL split that is real.** The same `aether_core` demo runs on
   the host, on x86_64 QEMU (then ring-3 `/init`), and on RISC-V virt.
   Porting was a trampoline + UART + timer + page tables. The fabric
   does not encode x86.

The pitch is not “replace CUDA.” It is: your compiler keeps scheduling
FLOPs; Aether schedules partitions, fences, colors, and who is allowed
to name a tile.

If that contract matches the chip, start at `aether_hal::AccelDevice`
and tell us which opcode / dtype / route fields the command processor
already has.
