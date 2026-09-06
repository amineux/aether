# Aether architecture

Aether is a **fabric kernel**: the OS is a capability machine whose primitive
is a typed message, not a POSIX process. The research bet is that AI packages
(CPU + NPU + HBM chiplets) want that model more than they want a Linux
compatibility layer with a vendor runtime bolted on.

This document describes what is implemented in v0.1 and how the pieces fit.
It does not invent performance numbers.

## Thesis

1. **Tiles are peers.** A CPU thread and an NPU wave are both `Job`s. The
   scheduler sees deadlines and bank affinity for both.
2. **Capabilities are the only names.** You cannot speak to an endpoint, map
   a tensor, or ring an accel doorbell without a `CPtr` in *your* table.
3. **Memory is owned, not shared.** Crossing a tile boundary is an ownership
   transfer. The kernel does not promise cache coherence.
4. **The HAL is the silicon contract.** Drivers implement `AccelDevice`. The
   rest of the kernel does not know if the backend is SoftNPU, VirtIO, or a
   real command processor.

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
