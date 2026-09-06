# Roadmap and stubs

Aether v0.1 is a **working vertical slice**: QEMU boot, fabric demo,
host-tested invariants. It is not a product kernel.

## STUB markers in the tree

Search for `// STUB:` / `STUB` :

| Item | Where | Intent |
| --- | --- | --- |
| SMP AP bring-up | `kernel/src/arch/irq.rs` `smp_start_aps` | INIT-SIPI, per-CPU `gs`, IPI |
| F16/F32 dtypes | `core/src/accel.rs` | Soft-float or a real tensor ISA |
| SYSCALL/SYSRET + ring-3 | `kernel/src/syscall.rs` | Trap gate; init becomes a user task |
| ELF loader | (none yet) | Load `/init` from a ramfs or virtio-blk |
| Multiboot mmap | `kernel/src/mm/mod.rs` | Stop assuming 128 MiB @ 16 MiB |
| Higher-half + KASLR | linker / trampoline | Standard kernel hardening |
| IOMMU / SMMU | `AccelDevice::map` | Make Memory caps physically true |
| VirtIO-Accel QEMU device | `docs/ACCEL.md` | Optional; software backend is enough to demo |
| Blocking sync IPC | `core/src/fabric.rs` | Waiter lists + scheduler sleep |
| Cap derivation tree | `core/src/caps.rs` | Revoke descendants |
| RISC-V / aarch64 | `kernel/src/arch` | New boot + irq/timer/serial |
| Fiedler eigensolve | `core/src/cut.rs` | Power iteration on `L=D−A`; v0.1 enumerates n≤8 |
| AffinityLaplacian | (none) | First-class `L` object; heat-kernel / commute-time distances for placement |
| OperatorKernelHandle | (none) | Cap for a compiled collective (tree vs ring vs torus); binds a Hodge class |
| SparsifiedCollective | (none) | Drop harmonic components below a spectral threshold before inject |
| Real CXL.mem window | `MemorySpace::CxlRegion` | QEMU stub place today; no coherent load |
| Compiler ISA blob | `abi::Executable` | Kernel stores a handle; IREE/PJRT owns the bytes |
| Hardware fence/timeline | `core/src/fence.rs` | Software credits on QEMU; doorbell IRQ later |

## Suggested next cuts (technical, not calendar)

1. **User tasks.** TSS, `syscall`, a flat ELF64 loader, init in ring 3.
   This is when cap checks become the only send path.
2. **Real virtqueue MMIO.** Either a tiny QEMU device or virtio-mmio over
   a reserved region so submit is not in-process.
3. **Preemptive threads.** Context switch on PIT; `SYS_YIELD` and
   `accel_wait` actually block.
4. **Bank coloring.** Arena allocator takes a tenant color; scheduler
   refuses a wave whose arena bank is foreign without an explicit xfer.
5. **RISC-V port.** Same `aether-core`, new trampoline. This is the test
   that the HAL split is real.

## What we will not claim

- Benchmarks vs Linux / seL4 / CUDA / any NPU SDK
- seL4-level formal proofs (the cap table is inspired, not verified)
- A CUDA-style unified virtual address space
- Wafer-scale marketing; tile SRAM is the honest first place
- Cache coherence across chiplets (UCIe/EMIB are transport)
- Readiness for tape-out or safety certification
- Partnerships with silicon vendors
- In-kernel ML graph IR / fusion (compilers schedule FLOPs)

If you are a silicon OS team: start at `aether_hal::AccelDevice` and
`AccelJobDesc`, then tell us which opcode/dtype/route fields your command
processor already has. The rest of Aether is meant to stay out of your
way.
