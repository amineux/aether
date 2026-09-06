# Aether architecture

Aether is a **fabric kernel**: the OS is a capability machine whose primitive
is a typed message, not a POSIX process. The research bet is that AI packages
(CPU + NPU + HBM chiplets) want that model more than they want a Linux
compatibility layer with a vendor runtime bolted on.

This document describes what is implemented in v0.1 and how the pieces fit.
It does not invent performance numbers.

## Thesis

Accelerators are activities on a capability fabric, not devices behind ioctl.

Memory is a typed place: tile SRAM, HBM, CXL region—never a single address space by default.

The kernel schedules partitions and fences; compilers schedule FLOPs.

Chiplets extend the NoC; UCIe is transport, not the programming model.

Isolation is spatial (slices/columns) first, temporal second—QoS and blast radius are invariants.

Supporting rules (implemented in types, not just prose):

1. **Activities are endpoints.** A CPU tile and a virt accel share one
   `Activity` / `EndpointId` object. There is no `/dev` ioctl surface.
2. **Typed memory spaces.** Buffers bind to
   `HOST | DEVICE_HBM | TILE_SRAM | CXL_REGION | SCRATCH | STREAMING`.
   `UNIFIED_MEMORY` is an explicit capability bit, never the default.
3. **`(place, local)` addresses.** Remote access is an explicit DMA/NoC
   Exchange. The HAL refuses a silent coherent load.
4. **Partition profiles.** Spatial slice + QoS (bw/credits) + blast-radius
   isolation. Scheduler and accel bind to a partition.
5. **Fence-ordered jobs.** submit → fence/timeline → complete/timeout,
   credit-limited per partition — not CUDA streams.
6. **Named phases.** `Compute | Exchange | Barrier` tags on jobs and
   messages. The kernel does not fuse them.
7. **Kernel = submission shim + resource solver.** No ML graph IR or
   fusion in-kernel. Compilers own the ISA. The host ABI is shaped like
   PJRT/IREE HAL (Device, MemorySpace, Buffer, Executable, Event). See
   [ABI.md](ABI.md).
8. **Cuts and Hodge classes remain capabilities.** A `SpectralCut` is a
   bound partition of the package graph; a `FlowClass` on every message
   selects gradient / curl / harmonic policy. See [CUT.md](CUT.md).

What this document will not claim: a CUDA-style unified virtual address
space; seL4-level formal proofs; wafer-scale marketing that hides SRAM-first
placement; or cache coherence across chiplets.

## Boot (x86_64 / QEMU)

```
QEMU -kernel build/aether.elf
        │  multiboot1, 32-bit protected mode, paging off
        ▼
boot/x86_64/trampoline.S
        │  identity-map 4 GiB (2 MiB pages)
        │  enable PAE + EFER.LME + paging
        │  copy payload → 0x400000
        ▼
kernel::_start  (Rust, x86_64-unknown-none)
        │  stack in BSS, serial, frames, heap, IDT, PIT
        ▼
init::run_demo
        │  fabric + arena + virtio-accel SoftNPU
        ▼
hlt idle
```

The trampoline is ELF32 so `qemu-system-x86_64 -kernel` (multiboot1) will
load it. The Rust kernel is ELF64, objcopy'd to a flat binary, and
`.incbin`'d into the trampoline.

Physical sketch (128 MiB guest):

| Range | Use |
| --- | --- |
| `0x1000–0x7000` | Boot page tables (PML4/PDPT/4×PD) |
| `0x100000` | Multiboot loader + embedded kernel blob |
| `0x400000` | Kernel `.text` (after copy) |
| `0x0100_0000–0x0800_0000` | Frame allocator window |

Higher-half, KASLR, and a real multiboot mmap parser are not in v0.1.

## Crate graph

```
aether-core     alloc-free: caps, fabric, arenas, sched, SoftNPU math, demo
     ▲
aether-hal      AccelDevice / Console / Timer
     ▲
aether-drivers  VirtioAccelQueue + SoftNpuDevice
     ▲
aether-kernel   arch, mm, console, syscall ABI, built-in init
```

`aether-core` is the portable specification. Host tests execute the same
`run_boot_demo()` the kernel prints.

## Module map (kernel)

| Path | Responsibility |
| --- | --- |
| `kernel/src/arch/x86_64` | UART, IDT/PIC, PIT, port I/O |
| `kernel/src/mm` | Frames, bump heap, page walk |
| `kernel/src/syscall.rs` | Numbered ABI (debug_print, yield, send/recv, map, accel_*) |
| `kernel/src/init.rs` | Built-in init task / demo |
| `core/src/caps.rs` | Cap table |
| `core/src/fabric.rs` | Endpoints and messages |
| `core/src/arena.rs` | Bank-aware allocator |
| `core/src/sched.rs` | Tile scheduler |
| `core/src/accel.rs` | Job desc + reference matmul |
| `core/src/observe.rs` | Event ring |
| `core/src/cut.rs` | ChipletSpectralCut + affinity graph |
| `core/src/hodge.rs` | FlowHodgeQuota policy + quotas |
| `core/src/space.rs` | Typed `MemorySpace` + `(place, local)` |
| `core/src/activity.rs` | Fabric activity behind a uniform endpoint |
| `core/src/partition.rs` | Spatial slice + QoS + blast radius |
| `core/src/fence.rs` | Timeline / credit-limited submit |
| `core/src/phase.rs` | Compute / Exchange / Barrier tags |
| `core/src/abi.rs` | PJRT/IREE-shaped host objects (no graph IR) |

## HAL ports (future arches)

To add RISC-V or aarch64:

1. New `boot/<arch>` + linker script.
2. Implement `kernel/src/arch/<arch>`: console, timer, irq ack, page tables.
3. Keep `aether-core` / `aether-hal` unchanged.

The fabric does not encode x86.

## SMP

`arch::irq::smp_start_aps` is a **STUB**. Per-CPU state is a single tick
counter. Work-stealing is implemented in the scheduler data structure and
exercised on the host; it is not yet driven by multiple hardware threads.

## Userspace

v0.1 has no ELF loader and no ring-3. Init is compiled into the kernel and
calls `syscall::dispatch` as a function. The syscall numbers are stable so a
later `syscall`/`sysret` gate can land without rewriting the demo.
