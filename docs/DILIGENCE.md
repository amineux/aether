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
| TypedWindow (honest pin stub) | Host-tested; CXL.mem window **not** a milestone | `core/src/window.rs`, [WINDOW.md](WINDOW.md) |
| Bank color (Compute refuse / Exchange ok) | Implemented, host-tested | `core/src/color.rs` |
| `IommuMap` Soft SMMU (STE→CD→S1/S2 walk, ATS invalidate) | Implemented, host-tested | `core/src/iommu.rs` |
| Tile scheduler + SpectralCut refuse | Implemented (n≤32 Fiedler placement; enum n≤8) | `core/src/{sched,cut}.rs` |
| AffinityLaplacian `L = D − A` | Implemented (integer prototype, n≤32 host-tested) | `core/src/laplacian.rs` |
| Hodge flow-class quotas | Implemented | `core/src/hodge.rs` |
| OperatorKernelHandle (collective × Hodge) | Implemented, host-tested | `core/src/opkernel.rs` |
| SparsifiedCollective (milli threshold) | Implemented, host-tested | `core/src/sparsify.rs` |
| Accel HAL + SoftNPU + virtqueue MMIO | Implemented (in-kernel BAR path B); I32 + software F16/F32 | `hal/`, `drivers/`, `core/src/accel.rs` |
| Path-A QEMU `aether-accel` | Optional device model + host test; stock QEMU stays B | `qemu/`, `make accel-test` / `make qemu-accel` |
| SoftCommandProcessor (`backend = 3`) | Software CP: `CpCmd` + Soft SMMU SID + IRQ/fence | `drivers/src/fakecp.rs` |
| IreeShapedCp (`backend = 4`) | IREE HAL dispatch packet + Soft SMMU `ssid=2` + IRQ/fence; not a vendor | `drivers/src/ireecp.rs` |
| Fence / timeline | Software CP-shaped seq / wait / complete (not silicon) | `core/src/fence.rs` |
| Partner sketch `PartnerNpuStub` | No-op `AccelDevice` (not a CP path) | `drivers/src/partner.rs` |
| PJRT/IREE-shaped host nouns | Types only; no graph IR | `core/src/abi.rs`, `docs/ABI.md` |
| x86_64 QEMU + ring-3 `/init` | Working vertical slice | `boot/x86_64/`, `user/init/`, `make qemu` |
| Per-task PML4 + SMEP/SMAP | Documented x86 subset (CR3 + USER-local 2 MiB) | `kernel/src/mm/paging.rs`, `core/src/aspace.rs` |
| User-level threads (`SYS_CLONE`) | Additive nr 10; share caller aspace; not Linux clone | `kernel/src/{task,syscall}.rs`, `user/init` |
| Growable user `mmap` (`SYS_MMAP`) | Additive nr 11; anonymous 4 KiB USER pages; not POSIX | `kernel/src/{syscall,mm/paging}.rs`, `user/init` |
| In-kernel ramfs for `/init` | Named files; seed from virtio-blk or blobs; not POSIX | `core/src/{ramfs,bootfs}.rs`, `kernel/src/{elfload,virtio_blk}.rs` |
| RISC-V virt boot | S-mode + U-mode `/init` + PLIC SoftNPU doorbell | `boot/riscv64/`, `user/init/`, `make qemu-riscv` |
| aarch64 virt boot | EL1 + EL0 `/init` + TTBR0 isolate + in-kernel SoftNPU | `boot/aarch64/`, `user/init/`, `make qemu-aarch64` |
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
| Hardware SMMU | Soft SMMU deepened (STE→CD→S1/S2 + ATS invalidate) but is still software only; a real device can still DMA past it. Partner silicon required. |
| Custom QEMU virtio-accel | Path A landed as optional (`qemu/`; `make accel-test`). Path B is still what stock `make qemu` runs. CI does not rebuild QEMU. Guest does not yet bind PCI BAR0 |
| RISC-V userspace is a subset | U-mode `/init` + `ecall`/`sret` + Sv39 isolate + in-kernel SoftNPU. PLIC software doorbell (UART THRE); no virtio-mmio `-device` |
| aarch64 userspace is a subset | EL0 `/init` + `svc`/`eret` + TTBR0 isolate + in-kernel SoftNPU (timer/kthread drain). No GICv3, no virtio-mmio |
| Fiedler is integer power iteration | n≤32 host-tested median-cut; enum stays n≤8. Not GiFt-Placer |
| ChipletFleet | KILL as calendar. Thin `ChipletTaskScope` host stub; not a Year-1 pillar, not a partner ask |
| SMP is a QEMU smoke | INIT-SIPI + `gs` + two-hart steal on `-smp 2`; APs are kernel-only |
| No secret KASLR / `fork` COW | HH + boot-time slide + PIE-reloc (`.rela.dyn` + unused alias unmapped) + KPTI + PCID + one-page COW + growable anon `SYS_MMAP` + identity teardown landed (`ffffffff80000000+PA` + 16 MiB slots; user CR3 has no HH / no identity DMA; tagged `mov cr3` when CPUID.PCID, else full flush; `USER_COW_BASE` RO until write; `USER_MMAP_BASE` `0x02C0_0000` first-fit 4 KiB). Kernel CR3 keeps SIPI / mailbox / trampoline / virtio-blk / APIC islands only; SoftNPU is Soft SMMU + HH. Not a secret slide, not Meltdown-complete, not POSIX `mmap` / `fork` |
| No FDT mmap | RISC-V / aarch64 print an explicit Multiboot-missing fallback; they do not invent a map |
| No CXL.mem | `TypedWindow` (`CxlMemStub`) is a host-tested pin/map stub; `MemorySpace::CxlRegion` is still a typed place. Not a HDM decoder, not QEMU CXL. See [WINDOW.md](WINDOW.md) |
| Cap CDT / revoke | **Landed** (small parent/child + `revoke_in`). Not a seL4 CNode. No user syscall. Kernel World is still one shared table |
| Hardware fence / timeline | **Landed** as a software model (seq / wait / complete + credits). Timeout is software. QEMU IRQ is still software. Not a silicon fence |

x86_64 **does** have ring-3 `/init` + `syscall`/`sysret` and cap checks on
send/recv/map/accel. RISC-V now has the same syscall numbers over
`ecall`/`sret` (U-mode `/init`, in-kernel SoftNPU, PLIC software
doorbell). aarch64 now has the same syscall numbers over
`svc`/`eret` (EL0 `/init`, in-kernel SoftNPU, no GIC doorbell).

## How a silicon team plugs `AccelDevice`

```text
1. PCI / MMIO / NoC probe. Fill AccelInfo { backend: 4 (or your id),
   vendor, ... }. Do not reuse 0 (SoftNPU), 1 (virtqueue SoftNPU),
   2 (PartnerNpuStub), 3 (Soft-CP), or 4 (IreeShapedCp).
2. Implement aether_hal::AccelDevice { probe, submit, poll, map }.
   SoftCommandProcessor is the Aether-native packet example.
   IreeShapedCp is the partner-shaped IREE HAL packet example.
3. map(): bind_stream + pin from a Memory cap walk. Refuse anything
   that did not come from the cap table. Refuse a silent remote
   (place, local) — aether_hal::map_fabric already does.
   IommuMap is the Soft-SMMU table (STE→CD→S1/S2 walk; abort until Bound;
   ATS invalidate is a software ATC). A hardware SMMU is still required
   on silicon; do not treat this as one.
4. submit(): pack AccelJobDesc into the chip's command packet. Soft-CP
   uses the 64-byte CpCmd in [ACCEL.md](ACCEL.md) with a packed StreamId.
   IreeShapedCp uses the 96-byte IreeHalCmd (IREE HAL nouns; not AccelOp).
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
`SoftCommandProcessor` or `IreeShapedCp`. `PartnerNpuStub` is a leftover
no-op sketch, not a partnership and not this path.

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
    Host property tests in `core/src/caps_props.rs` lock this (not a proof).

Not enforced in hardware yet: SMMU stream IDs, RISC-V virtio-mmio,
aarch64 GICv3 / virtio-mmio, measured boot. The RISC-V PLIC is programmed and the
SoftNPU used-ring is claimed on source 10; that is still a software
doorbell on the path-B BAR, not a silicon MSI. Revoke descendants is host-tested (`revoke` /
`revoke_in`); there is no `SYS_REVOKE` and no kernel-global CNode walk.
On x86, isolation is “cap tables + ring-3 + per-task USER leaves +
SMEP/SMAP + KPTI trampoline + PCID (if CPUID) + Soft SMMU.” On RISC-V it is “cap tables + U-mode +
task-local U leaves + SUM off + Soft SMMU.” Soft SMMU is a deepened
software table (STE→CD→S1/S2 + ATS invalidate) a real device can
ignore; hardware SMMU needs partner silicon. The kernel runs higher-half; user
CR3 does not map HH or the identity DMA window (KPTI subset, not
Meltdown-complete; PCID is a tagged-TLB gate, not a speculation
barrier). On aarch64 it is “cap tables + EL0 +
task-local AP_EL0 leaves + Soft SMMU” (no PAN on cortex-a72).

## CI status

| Job | Command | Intent |
| --- | --- | --- |
| Host tests | `cargo test --workspace` | Caps + CDT properties, fabric, arenas, color, map, typed window stub, sched, SoftNPU, Laplacian, ELF, ramfs, bootfs, mmap, opkernel, sparsify |
| x86_64 boot | `make qemu-ci` | Ring-3 `/init` + virtqueue demo; greps Multiboot mmap + SMEP/SMAP + aspace isolate + `[mm] pcid` + embedded ramfs |
| x86_64 virtio-blk | `make qemu-blk-ci` | `-drive` AETHFS01; greps `[blk] virtio-blk seed /init` + SoftNPU |
| x86_64 PCID on | `make qemu-pcid-ci` | requests `+pcid,+invpcid`; TCG cannot advertise it (warn + fallback). `[mm] pcid ok` if KVM implements PCID |
| x86_64 PCID off | `make qemu-nopcid-ci` | `-cpu qemu64,-pcid`; greps `[mm] pcid fallback` |
| x86_64 SMP smoke | `make qemu-smp-ci` | `-smp 2`; greps AP online + work-steal + SoftNPU banner |
| RISC-V boot | `make qemu-riscv-ci` | U-mode `/init` + `ecall` + PLIC SoftNPU used-ring + aspace isolate + fabric banner |
| aarch64 boot | `make qemu-aarch64-ci` | EL0 `/init` + `svc` + aspace isolate + fabric banner |

x86_64 is the supported path. RISC-V CI greps U-mode `/init`.
aarch64 CI greps EL0 `/init`. Neither is a second-architecture
product.

## Non-claims

We will not claim:

- Partnerships with NVIDIA, any ASIC house, or any compiler project
- Benchmarks vs Linux / seL4 / CUDA / any NPU SDK
- seL4-level formal proofs
- A CUDA-style unified virtual address space
- Cache coherence across chiplets
- Wafer-scale marketing; tile SRAM is the honest first place
- Readiness for tape-out or safety certification
- That the RISC-V or aarch64 port is a product-class second architecture
- That `AffinityLaplacian` is a production eigensolver
- That chiplet-local steal / `ChipletTaskScope` is ChipletFleet, a
  Year-1 pillar, a partner ask, or unpublished Fleet numbers
- That `IommuMap` / Soft SMMU is a hardware SMMU
- That `TypedWindow` / `CxlMemStub` is CXL.mem silicon or QEMU CXL
- That `SoftCommandProcessor` is a silicon driver
- That `IreeShapedCp` is an IREE runtime, a PJRT plugin, or a signed vendor
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
   type error. Soft SMMU gives per-stream non-identity IOVA and a
   STE→CD→Stage-1/2 walk in software — policy and the software table
   are tested; a hardware SMMU is not programmed.
3. **Placement that names the package graph.** A SpectralCut is a
   capability. The Laplacian is a first-class `L = D − A`. Cross-die
   placement is refused because the cut said so, not because a hint
   was ignored.
4. **A kernel that stays out of FLOPs.** Named phases
   (`Compute | Exchange | Barrier`) are tags. Fusion stays in IREE /
   PJRT / the vendor stack. The host ABI is shaped like those runtimes
   on purpose.
5. **A HAL split that is real.** The same `aether_core` demo runs on
   the host, on x86_64 QEMU (then ring-3 `/init`), on RISC-V virt
   (then U-mode `/init`), and on aarch64 virt (then EL0 `/init`).
   Porting was a trampoline + UART + timer + page tables. The fabric
   does not encode x86.

The pitch is not “replace CUDA.” It is: your compiler keeps scheduling
FLOPs; Aether schedules partitions, fences, colors, and who is allowed
to name a tile.

If that contract matches the chip, start at `aether_hal::AccelDevice`
and tell us which opcode / dtype / route fields the command processor
already has.
