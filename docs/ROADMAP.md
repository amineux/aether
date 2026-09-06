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

## Month 3–4 (this cut)

| Item | Status |
| --- | --- |
| Virtqueue-shaped MMIO (doorbell + used-ring IRQ) | **done** (in-kernel BAR; SoftNPU backend) |
| Custom QEMU `virtio-accel` device | not started — stock QEMU + in-tree emulator |
| `IommuMap` pin/translate; refuse without Memory+MAP | **done** (identity IOVA) |
| Hardware SMMU / stream IDs | not started (`MapRequest.stream_id` is a placeholder) |
| Arena tenant/bank color; Compute refuse + Exchange/transfer | **done** |
| Partner `AccelDevice` sketch (`PartnerNpuStub`) | **done** (no-op; not a partnership) |

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
| RISC-V / aarch64 | `kernel/src/arch` | New boot + irq/timer/serial |
| Fiedler eigensolve | `core/src/cut.rs` | Power iteration on `L=D−A`; v0.1 enumerates n≤8 |
| AffinityLaplacian | (none) | First-class `L` object; heat-kernel / commute-time distances for placement |
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
3. **RISC-V port.** Same `aether-core`, new trampoline. This is the test
   that the HAL split is real.
4. **Per-task page tables.** Isolation becomes a hardware fact.
5. **Cap CDT / revoke.** Descendants die with the parent.

## What we will not claim

- Benchmarks vs Linux / seL4 / CUDA / any NPU SDK
- seL4-level formal proofs (the cap table is inspired, not verified)
- A CUDA-style unified virtual address space
- Wafer-scale marketing; tile SRAM is the honest first place
- Cache coherence across chiplets (UCIe/EMIB are transport)
- Readiness for tape-out or safety certification
- Partnerships with silicon vendors (`PartnerNpuStub` is a sketch)
- In-kernel ML graph IR / fusion (compilers schedule FLOPs)

If you are a silicon OS team: start at `aether_hal::AccelDevice`,
`AccelJobDesc`, and `PartnerNpuStub`, then tell us which opcode/dtype/route
fields your command processor already has. The rest of Aether is meant
to stay out of your way.
