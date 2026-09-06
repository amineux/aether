# Diligence pack (research prototype)

This is what a silicon OS team would receive before spending bring-up
time on Aether. It is **not** a partnership announcement, a tape-out
checklist, or a benchmark brief. The public site (`site/`) is the same
leave-behind — not a vendor pitch. See [ROADMAP.md](ROADMAP.md) for the
active track the site must match.

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
| OperatorKernelHandle (collective × Hodge) | Implemented, host-tested | `core/src/opkernel.rs` |
| SparsifiedCollective (milli threshold) | Implemented, host-tested | `core/src/sparsify.rs` |
| Accel HAL + SoftNPU + virtqueue MMIO | Implemented (in-kernel BAR) | `hal/`, `drivers/`, `core/src/accel.rs` |
| SoftCommandProcessor (`backend = 3`) | Software CP: `CpCmd` + Soft SMMU SID + IRQ/fence | `drivers/src/fakecp.rs` |
| Fence / timeline | Software CP-shaped seq / wait / complete (not silicon) | `core/src/fence.rs` |
| Partner sketch `PartnerNpuStub` | No-op `AccelDevice` (not a CP path) | `drivers/src/partner.rs` |
| PJRT/IREE-shaped host nouns | Types only; no graph IR | `core/src/abi.rs`, `docs/ABI.md` |
| x86_64 QEMU + ring-3 `/init` | Working vertical slice | `boot/x86_64/`, `user/init/`, `make qemu` |
| Per-task PML4 + SMEP/SMAP | Documented x86 subset (CR3 + USER-local 2 MiB) | `kernel/src/mm/paging.rs`, `core/src/aspace.rs` |
| RISC-V virt boot | Thin S-mode port | `boot/riscv64/`, `make qemu-riscv` |
| aarch64 virt boot | Thin EL1 port (no EL0) | `boot/aarch64/`, `make qemu-aarch64` |
| Multiboot mmap → frames | Documented x86 subset (clip 16 MiB, cap 128 MiB); HAL fallback | `core/src/mmap.rs`, `kernel/src/mm/` |

The portable specification is `aether-core`. Host tests execute the same
`run_boot_demo()` the kernels print (caps, fabric, map, color, cut), plus
the Multiboot mmap parser. The RISC-V and aarch64 ports did not change
`aether-hal` or the syscall / AccelDevice ABI.

## What is stubbed

See [ROADMAP.md](ROADMAP.md) for the full STUB table. The diligence-relevant
gaps:

| Gap | Honest reading |
| --- | --- |
| Hardware SMMU | Soft SMMU is software only (chiplet SIDs + capture/bind); a real device can still DMA past it |
| Custom QEMU virtio-accel | In-kernel BAR + SoftNPU; stock QEMU is enough to demo |
| RISC-V is thin | kmain + UART + Sv39 + `aether_core` self-check. No ring-3, no PLIC virtio |
| aarch64 is thin | kmain + PL011 + TTBR + GICv2/CNTV + `aether_core` self-check. No EL0, no virtio |
| Fiedler is integer power iteration | Cut construction for n≤8 still enumerates |
| SMP is a QEMU smoke | INIT-SIPI + `gs` + two-hart steal on `-smp 2`; APs are kernel-only |
| No higher-half / KPTI | Per-task PML4 clones the identity 4 GiB; kernel can still name every PA |
| No FDT mmap | RISC-V / aarch64 print an explicit Multiboot-missing fallback; they do not invent a map |
| No CXL.mem | `MemorySpace::CxlRegion` is a typed place, not a window |
| Cap CDT / revoke | **Landed** (small parent/child + `revoke_in`). Not a seL4 CNode. No user syscall. Kernel World is still one shared table |
| Hardware fence / timeline | **Landed** as a software model (seq / wait / complete + credits). Timeout is software. QEMU IRQ is still software. Not a silicon fence |

x86_64 **does** have ring-3 `/init` + `syscall`/`sysret` and cap checks on
send/recv/map/accel. That is not stubbed on x86; it is stubbed on RISC-V
and aarch64.

## How a silicon team plugs `AccelDevice`

```text
1. PCI / MMIO / NoC probe. Fill AccelInfo { backend: 3 (or your id),
   vendor, ... }. Do not reuse 0 (SoftNPU), 1 (virtqueue SoftNPU),
   or 2 (PartnerNpuStub).
2. Implement aether_hal::AccelDevice { probe, submit, poll, map }.
   SoftCommandProcessor is the in-tree worked example.
3. map(): bind_stream + pin from a Memory cap walk. Refuse anything
   that did not come from the cap table. Refuse a silent remote
   (place, local) — aether_hal::map_fabric already does.
   IommuMap is the Soft-SMMU table (per-stream IOVA; abort until Bound).
   A hardware SMMU is still required on silicon; do not treat this as one.
4. submit(): pack AccelJobDesc into the chip's command packet. Soft-CP
   uses the 64-byte CpCmd in [ACCEL.md](ACCEL.md) with a packed StreamId.
   Doorbell. Do not execute in the syscall.
5. IRQ: AccelDevice::poll, retire the job's fence seq through
   `Timeline::complete` / `retire_into`, fabric REPLY to
   job.completion_ep. The timeline is a software model.
```

Do **not** map all of HBM into the NPU. The arena + cap + color is the point.

The compiler / runtime (IREE, XLA/PJRT, a vendor stack) owns the ISA
blob (`abi::Executable`). Aether admits the job against a partition,
a SpectralCut, a bank color, and a fence. It does not fuse a graph.

Walkthrough: [ACCEL.md](ACCEL.md), [ABI.md](ABI.md). Start from
`SoftCommandProcessor`. `PartnerNpuStub` is a leftover no-op sketch,
not a partnership and not this path.

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
11. Revoke of a parent empties derived children in that table;
    `revoke_in` empties GRANT-children in named tables. Unrelated caps live.

Not enforced in hardware yet: SMMU stream IDs, RISC-V ring-3, aarch64
EL0, measured boot. Revoke descendants is host-tested (`revoke` /
`revoke_in`); there is no `SYS_REVOKE` and no kernel-global CNode walk.
On x86, isolation is “cap tables + ring-3 + per-task USER leaves +
SMEP/SMAP + Soft SMMU.” Soft SMMU is a software table a real device can
ignore. The kernel identity map still lets a forged kernel pointer name
a physical address. On RISC-V / aarch64 it is still “the cap tables do
the right thing.”

## CI status

| Job | Command | Intent |
| --- | --- | --- |
| Host tests | `cargo test --workspace` | Caps, fabric, arenas, color, map, sched, SoftNPU, Laplacian, ELF, mmap, opkernel, sparsify |
| x86_64 boot | `make qemu-ci` | Ring-3 `/init` + virtqueue demo; greps Multiboot mmap + SMEP/SMAP + aspace isolate |
| x86_64 SMP smoke | `make qemu-smp-ci` | `-smp 2`; greps AP online + work-steal + SoftNPU banner |
| RISC-V boot | `make qemu-riscv-ci` | OpenSBI S-mode + self-check banner; greps mmap fallback |
| aarch64 boot | `make qemu-aarch64-ci` | QEMU virt EL1 + self-check banner; greps mmap fallback |

x86_64 is the supported path. RISC-V and aarch64 CI grep the fabric
success banner. They are bring-up tests, not second-architecture
products.

## Non-claims

We will not claim:

- Partnerships with NVIDIA, any ASIC house, or any compiler project
- Benchmarks vs Linux / seL4 / CUDA / any NPU SDK
- seL4-level formal proofs
- A CUDA-style unified virtual address space
- Cache coherence across chiplets
- Wafer-scale marketing; tile SRAM is the honest first place
- Readiness for tape-out or safety certification
- That the RISC-V or aarch64 port is a full userspace kernel
- That `AffinityLaplacian` is a production eigensolver
- That `IommuMap` / Soft SMMU is a hardware SMMU
- That `SoftCommandProcessor` is a silicon driver
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
   the host, on x86_64 QEMU (then ring-3 `/init`), and on RISC-V /
   aarch64 virt.
   Porting was a trampoline + UART + timer + page tables. The fabric
   does not encode x86.

The pitch is not “replace CUDA.” It is: your compiler keeps scheduling
FLOPs; Aether schedules partitions, fences, colors, and who is allowed
to name a tile.

If that contract matches the chip, start at `aether_hal::AccelDevice`
and tell us which opcode / dtype / route fields the command processor
already has.
