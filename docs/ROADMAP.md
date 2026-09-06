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
| `IommuMap` pin/translate; refuse without Memory+MAP | **done** (identity IOVA) |
| Hardware SMMU / stream IDs | not started (`MapRequest.stream_id` is a placeholder) |
| Arena tenant/bank color; Compute refuse + Exchange/transfer | **done** |
| Partner `AccelDevice` sketch (`PartnerNpuStub`) | **done** (no-op; not a partnership) |

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

## STUB markers in the tree

Search for `// STUB:` / `STUB` :

| Item | Where | Intent |
| --- | --- | --- |
| SMP AP bring-up | `kernel/src/arch/irq.rs` `smp_start_aps` | INIT-SIPI, per-CPU `gs`, IPI |
| F16/F32 dtypes | `core/src/accel.rs` | Soft-float or a real tensor ISA |
| Multiboot mmap | `kernel/src/mm/mod.rs` | Stop assuming 128 MiB @ 16 MiB |
| Higher-half + KASLR | linker / trampoline | Standard kernel hardening |
| Hardware SMMU | `core/src/iommu.rs` | Replace identity IOVA with stream IDs |
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
   BAR layout. SoftNPU can stay the executor behind the device.
2. **SMMU page tables.** `IommuMap` already tracks windows; program a
   real stream ID instead of identity IOVA.
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

Nothing in that file marks Soft SMMU, virtio-accel QEMU, SMP, or the
other stubs above as done.

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
`AccelJobDesc`, and `PartnerNpuStub`, then tell us which opcode/dtype/route
fields your command processor already has. The rest of Aether is meant
to stay out of your way. [DILIGENCE.md](DILIGENCE.md) is the leave-behind;
[DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) is the meeting.
