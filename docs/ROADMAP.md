# Roadmap and stubs

Aether v0.1 is a **working vertical slice**: QEMU boot, ring-3 `/init`,
fabric + SoftNPU via virtqueue MMIO, host-tested invariants. It is not a
product kernel.

## Month 1–2

| Item | Status |
| --- | --- |
| Ring-3 + `syscall`/`sysret` (STAR/LSTAR/SFMASK, TSS.RSP0) | **done** |
| Cap checks on send/recv/map/accel from ring-3 | **done** |
| ELF64 static non-PIE loader; `/init` embedded blob | **done** |
| Preemptive threads on PIT; `SYS_YIELD` / blocking wait | **done** |
| CI: `cargo test --workspace` + `make qemu` (isa-debug-exit) | **done** |
| ramfs / virtio-blk for `/init` | not started (blob is enough) |
| Per-task PML4 / SMEP / SMAP | not started |
| User-level threads (clone) | not started — kthread-B + `/init` mix |

## Month 3–4

| Item | Status |
| --- | --- |
| Virtqueue-shaped MMIO (doorbell + used-ring IRQ) | **done** (in-kernel BAR; SoftNPU backend) |
| Custom QEMU `virtio-accel` device | not started — stock QEMU + in-tree emulator |
| `IommuMap` pin/translate; refuse without Memory+MAP | **done** (Soft SMMU; non-identity IOVA) |
| Soft SMMU / software stream IDs | **done** (per-stream IOVA namespaces; not hardware) |
| Hardware SMMU / stream IDs | not started (no SID programmed on a real SMMU) |
| Arena tenant/bank color; Compute refuse + Exchange/transfer | **done** |
| Partner `AccelDevice` sketch (`PartnerNpuStub`) | **done** (no-op; not a partnership; not a CP path) |
| SoftCommandProcessor (`backend = 3`) | **done** (packed `CpCmd` + Soft SMMU SID + IRQ/fence; host tests) |

## Month 5–6 (this cut): Portability & partners

Landed:

- **RISC-V virt bring-up.** `boot/riscv64` trampoline + Sv39 identity
  map; `kernel/src/arch/riscv64` UART / SBI timer / stvec. `make qemu-riscv`
  boots to `kmain`, prints serial hello, and runs the same
  `aether_core` self-check as x86 (including map + bank-color).
  `aether-core` / `aether-hal` unchanged. **No** `sret` / ELF `/init` on
  this arch — that is v0.1 of the port.
- **AffinityLaplacian.** First-class `L = D − A` in `core/src/laplacian.rs`
  with integer Rayleigh, Fiedler-ish power iteration, heat-kernel and
  commute-time helpers. Host tests. `SpectralCut::from_fiedler` is wired;
  placement for n≤8 still enumerates.
- **Diligence pack.** [DILIGENCE.md](DILIGENCE.md) — what ships, what is
  stubbed, how to plug `AccelDevice`, security invariants, CI, non-claims,
  and a design-win narrative that does not invent a partner.
- **Deep-dive agenda.** [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) — a
  60–90 min script for an NVIDIA / ASIC OS team. No meeting is claimed.

Honest limits of this cut:

- RISC-V is a **thin HAL test**, not a second full kernel. Ring-3, virtqueue
  MMIO, and PIT preemption stay x86_64. A later cut would repeat that work
  on `sret`.
- Fiedler is integer power iteration on n≤8, not a production eigensolve.
- Nobody from a silicon team has reviewed this. The agenda is so they
  could.
- SoftCommandProcessor is a **software CP**, not a silicon driver. It
  uses Soft SMMU (`StreamId` + bind/abort). QEMU still demos SoftNPU.
  `PartnerNpuStub` is unchanged.

## Year-1 H1: Soft SMMU

Landed (software only — **not** a hardware SMMU, **not** an SMMUv3 emulator):

- Per-stream Soft-SMMU block table in `core/src/iommu.rs`. Stream A and
  stream B may pin the same guest PA to different IOVAs. Same-SID
  guest-PA overlap is `Overlap` (or `CrossTenant` if another tenant
  already holds the window).
- **Chiplet StreamIDs** (`StreamId` = `chiplet | tile | ssid`), not PCIe
  BDF. STE → CD (SSID) indexing is a software table, not a guest walk.
- **SID lifecycle** (OpenVMM / smmuv3-accel shaped): Unbound → Captured
  on first sighting → Bound on Memory+MAP `bind_stream` / first `map`.
  Translate **aborts** (`StreamAbort`) until Bound. `unbind_stream` is
  the FLR analogue.
- Non-identity IOVA allocator: each (STE, CD) gets a window above
  4 GiB (`SOFT_SMMU_IOVA_BASE`). `iova != guest_pa` for the QEMU demo.
- `translate` / `resolve` / `unmap` are stream-aware (`WrongStream`,
  `NotMapped`, `CrossTenant`, `StreamAbort`). Memory+MAP is still
  required to pin or bind.
- SoftNPU / virtqueue DMA writes IOVAs into the avail ring and resolves
  them back to guest PA before `IdentityDma` / `SliceMem` loads.
- Host tests cover stream A vs B, chiplet SIDs, abort-until-bound,
  translate hit/miss, unmap, cap refuse, and non-identity IOVA.

Hardware SMMU (program a real SID / PT walk on an IOMMU) is still a
stub. QEMU does not emulate an SMMU for this path. Bank QoS beyond
existing admit/refuse is out of scope.

## STUB markers in the tree

Search for `// STUB:` / `STUB` :

| Item | Where | Intent |
| --- | --- | --- |
| SMP AP bring-up | `kernel/src/arch/irq.rs` `smp_start_aps` | INIT-SIPI, per-CPU `gs`, IPI |
| F16/F32 dtypes | `core/src/accel.rs` | Soft-float or a real tensor ISA |
| Multiboot mmap | `kernel/src/mm/mod.rs` | Stop assuming 128 MiB @ 16 MiB |
| Higher-half + KASLR | linker / trampoline | Standard kernel hardening |
| Hardware SMMU | `core/src/iommu.rs` | Soft SMMU (software SID + IOVA PT) landed; program a real SMMU |
| VirtIO-Accel QEMU device | `docs/ACCEL.md` | Optional; in-kernel MMIO + SoftNPU is the demo |
| Cap derivation tree | `core/src/caps.rs` | Revoke descendants |
| aarch64 | (none) | Not started; RISC-V was the HAL test |
| RISC-V ring-3 / PLIC virtio | `kernel/src/arch/riscv64` | Repeat the x86 userspace + virtqueue cut on S-mode |
| Production Fiedler | `core/src/laplacian.rs` | Power iteration is a prototype; Cut enumerates n≤8 |
| OperatorKernelHandle | (none) | Cap for a compiled collective (tree vs ring vs torus); binds a Hodge class |
| SparsifiedCollective | (none) | Drop harmonic components below a spectral threshold before inject |
| Real CXL.mem window | `MemorySpace::CxlRegion` | QEMU stub place today; no coherent load |
| Compiler ISA blob | `abi::Executable` | Kernel stores a handle; IREE/PJRT owns the bytes |
| Hardware fence/timeline | `core/src/fence.rs` | Software credits on QEMU; doorbell IRQ is now software |

Blocking sync IPC waiter lists are no longer a stub: `SYS_RECV` and
`SYS_ACCEL_WAIT` block the caller and the kernel wakes on send / used-ring
IRQ. The fabric object itself still returns `WouldBlock`; the
kernel thread queue sleeps.

## Suggested next cuts (technical, not calendar)

1. **Custom QEMU virtio-accel** (or virtio-mmio) that DMA-reads the same
   BAR layout. SoftNPU can stay the executor behind the device. Deferred;
   Soft-CP already covers a second AccelDevice path on the host.
2. **Hardware SMMU.** Soft SMMU already allocates per-stream IOVAs;
   program a real SMMU context / PT walk. Do not claim the software
   table is silicon.
3. **RISC-V userspace.** Same `aether-core`, `sret` + page-table isolate.
   Only worth it after the x86 ABI stays stable.
4. **Per-task page tables.** Isolation becomes a hardware fact.
5. **Cap CDT / revoke.** Descendants die with the parent.
6. **aarch64.** Same recipe as RISC-V: trampoline, UART, GIC timer, TTBR.

## Two-year plan

[YEAR2_PLAN.md](YEAR2_PLAN.md) holds both tracks (2026-09-06):

- **Active (Falsifier revision):** Soft SMMU SIDs on the AccelDevice
  map path, one real-shaped second AccelDevice (concrete command packet
  + fence/IRQ), ABI stay stable. Custom QEMU virtio-accel, SMP, ELF
  beyond `/init`, Laplacian expansion, and aarch64 are deferred.
- **Aspirational (SpecForge appendix):** original Y1H1–Y2H2 acceptance.
  Bank QoS beyond admit/refuse, partner-stub enrichment, CXL objects,
  cap CDT-as-calendar, and a Y2 bring-up climax are killed as
  milestones.

Soft SMMU (PR #7) and SoftCommandProcessor (this cut) are **done** as
software models. That file's "not done" line is stale for those two.
Custom QEMU virtio-accel, SMP, and the other stubs above are still open.

## What we will not claim

- Benchmarks vs Linux / seL4 / CUDA / any NPU SDK
- seL4-level formal proofs (the cap table is inspired, not verified)
- A CUDA-style unified virtual address space
- Wafer-scale marketing; tile SRAM is the honest first place
- Cache coherence across chiplets (UCIe/EMIB are transport)
- Readiness for tape-out or safety certification
- Partnerships with silicon vendors (`PartnerNpuStub` is a sketch)
- In-kernel ML graph IR / fusion (compilers schedule FLOPs)
- That the RISC-V port is a product-class second architecture

If you are a silicon OS team: start at `aether_hal::AccelDevice`,
`AccelJobDesc`, and `SoftCommandProcessor` (`CpCmd` in [ACCEL.md](ACCEL.md)),
then tell us which opcode/dtype/route fields your command processor
already has. `PartnerNpuStub` is a leftover no-op sketch, not a starting
point. The rest of Aether is meant to stay out of your way.
[DILIGENCE.md](DILIGENCE.md) is the leave-behind;
[DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) is the meeting.
