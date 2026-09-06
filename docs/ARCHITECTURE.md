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
5. **Fence-ordered jobs.** submit → wait → complete (or timeout),
   credit-limited per partition — a CP-shaped seq/timeline, not a
   CUDA stream and not a silicon fence unit. Timeout is software.
6. **Named phases.** `Compute | Exchange | Barrier` tags on jobs and
   messages. The kernel does not fuse them.
7. **Kernel = submission shim + resource solver.** No ML graph IR or
   fusion in-kernel. Compilers own the ISA. The host ABI is shaped like
   PJRT/IREE HAL (Device, MemorySpace, Buffer, Executable, Event). See
   [ABI.md](ABI.md).
8. **Cuts and Hodge classes remain capabilities.** A `SpectralCut` is a
   bound partition of the package graph; a `FlowClass` on every message
   selects gradient / curl / harmonic policy. An `OperatorKernelHandle`
   binds a collective topology to one of those classes.
   `SparsifiedCollective` may drop below-threshold harmonic before
   inject (integer milli; Hodge refuse unchanged). See [CUT.md](CUT.md).

What this document will not claim: a CUDA-style unified virtual address
space; seL4-level formal proofs; wafer-scale marketing that hides SRAM-first
placement; or cache coherence across chiplets.

## Boot (x86_64 / QEMU)

```
QEMU -kernel build/aether.elf
        │  multiboot1, 32-bit protected mode, paging off
        ▼
boot/x86_64/trampoline.S
        │  stash Multiboot EAX/EBX at 0x7000
        │  identity-map 4 GiB (2 MiB pages)
        │  enable PAE + EFER.LME + paging
        │  copy payload → 0x400000
        ▼
kernel::_start  (Rust, x86_64-unknown-none)
        │  stack in BSS, serial, mmap → frames, heap, IDT, GDT/TSS, SYSCALL, PIT
        │  smp_start_aps: INIT-SIPI AP 1, per-CPU gs, IPI, work-steal smoke
        ▼
init::run_kernel_selfcheck
        │  host-identical fabric + cut + hodge + SoftNPU
        ▼
elfload::load_init / load_probe  (embedded static ELF64s)
        │  clone per-task PML4; SMEP/SMAP; CR3 switch
        ▼
iretq → ring-3 /init
        │  syscall: debug_print, recv (block), yield, send, map, accel_*
        ▼
SYS_EXIT → isa-debug-exit
```

The trampoline is ELF32 so `qemu-system-x86_64 -kernel` (multiboot1) will
load it. The Rust kernel is ELF64, objcopy'd to a flat binary, and
`.incbin`'d into the trampoline.

Physical sketch (128 MiB guest):

| Range | Use |
| --- | --- |
| `0x1000–0x7000` | Boot page tables (PML4/PDPT/4×PD) |
| `0x8000–0x8FFF` | AP SIPI trampoline + mailbox (`make qemu-smp`) |
| `0x100000` | Multiboot loader + embedded kernel blob |
| `0x400000` | Kernel `.text` (after copy) |
| `0x0200_0000–0x0220_0000` | `/init` ELF + user stack (USER 2 MiB in `/init` PML4 only) |
| `0x0240_0000–0x0260_0000` | `/probe` ELF + user stack (USER 2 MiB in `/probe` PML4 only) |
| mmap type-1, clip 16 MiB, cap 128 MiB | Frame allocator (user images reserved). QEMU `-m 128M` is typically `0x0100_0000–0x07fe_0000` (ACPI reserved at the top) |

The boot path parses the Multiboot1 mmap (Multiboot2 parser is
host-tested). Type-1 regions below 16 MiB are printed then clipped so
the trampoline / page tables / AP SIPI / kernel image stay out of the
free pool. Missing mmap is an explicit arch-window fallback, not a
silent 128 MiB map. Higher-half and KASLR are still not in v0.1.

## Crate graph

```
aether-core     alloc-free: caps, fabric, arenas, sched, SoftNPU math, demo
     ▲
aether-hal      AccelDevice / Console / Timer
     ▲
aether-drivers  AccelMmio virtqueue + SoftNpuDevice + SoftCommandProcessor + PartnerNpuStub
     ▲
aether-kernel   arch, mm, syscall/sysret, ELF loader, tasks
user/init       static non-PIE ELF64 `/init` (embedded blob)
user/probe      optional second static ELF64 (own PML4 @ 0x2400000)
```

`aether-core` is the portable specification. Host tests execute the same
`run_boot_demo()` the kernel prints.

## Module map (kernel)

| Path | Responsibility |
| --- | --- |
| `kernel/src/arch/x86_64` | UART, IDT/PIC, PIT, GDT/TSS, SYSCALL MSRs, SMP (`gs` / APIC) |
| `kernel/src/mm` | Multiboot mmap → frames, bump heap, per-task PML4 clone, SMEP/SMAP, USER bits |
| `core/src/mmap.rs` | Host-tested Multiboot1 / Multiboot2 mmap parser + frame plan |
| `kernel/src/syscall.rs` | Numbered ABI; ring-3 trap dispatch + cap checks |
| `kernel/src/task.rs` | PIT preemption, yield, blocking recv/accel_wait |
| `kernel/src/elfload.rs` | Static ELF64 loader (embedded `build/init.elf`) |
| `kernel/src/world.rs` | Init cap table, fabric, arenas, virtqueue SoftNPU |
| `kernel/src/init.rs` | Kernel-side `run_boot_demo` self-check |
| `core/src/elf.rs` | Host-tested ELF64 parser |
| `core/src/aspace.rs` | Host-tested identity-map clone + USER-local walk |
| `core/src/preempt.rs` | Host-tested RR + block/wake queue |
| `core/src/sysnr.rs` | Frozen syscall numbers + user C ABI |
| `core/src/caps.rs` | Cap table |
| `core/src/fabric.rs` | Endpoints and messages |
| `core/src/arena.rs` | Bank-aware allocator + tenant color |
| `core/src/color.rs` | BankColor admit / refuse |
| `core/src/iommu.rs` | Soft SMMU pin/translate (per-stream, non-identity IOVA) |
| `drivers/src/fakecp.rs` | SoftCommandProcessor (`CpCmd` + SID + IRQ/`retire_into`) |
| `core/src/sched.rs` | Tile scheduler + color gate |
| `core/src/accel.rs` | Job desc + reference matmul |
| `core/src/observe.rs` | Event ring |
| `core/src/cut.rs` | ChipletSpectralCut + affinity graph |
| `core/src/laplacian.rs` | `AffinityLaplacian` (`L = D − A`) |
| `kernel/src/arch/riscv64` | UART0, stvec, SBI timer, Sv39 walk |
| `kernel/src/arch/aarch64` | PL011, VBAR, GICv2 + CNTV, TTBR0 walk |
| `core/src/hodge.rs` | FlowHodgeQuota policy + quotas |
| `core/src/opkernel.rs` | OperatorKernelHandle (collective × Hodge class) |
| `core/src/sparsify.rs` | SparsifiedCollective (drop below-threshold harmonic) |
| `core/src/space.rs` | Typed `MemorySpace` + `(place, local)` |
| `core/src/activity.rs` | Fabric activity behind a uniform endpoint |
| `core/src/partition.rs` | Spatial slice + QoS + blast radius |
| `core/src/fence.rs` | CP-shaped timeline (seq / wait / complete; credit limit; timeout is software) |
| `core/src/phase.rs` | Compute / Exchange / Barrier tags |
| `core/src/abi.rs` | PJRT/IREE-shaped host objects (no graph IR) |

## Boot (RISC-V / QEMU virt)

Thin v0.1 of the port — **kmain + serial + `aether_core` self-check**, not
ring-3. Same fabric, map API, and bank-color checks. New trampoline only.

```
QEMU -machine virt -kernel build/aether-riscv.elf
        │  OpenSBI (default -bios), S-mode, a0=hartid, a1=dtb
        ▼
boot/riscv64/trampoline.S
        │  park extra harts, UART0 hello
        │  Sv39 identity-map 4 GiB (1 GiB leaves)
        ▼
kernel::kmain  (Rust, riscv64gc-unknown-none-elf)
        │  UART, frames, heap, stvec, SBI timer
        ▼
init::run_kernel_selfcheck   (same aether_core path as x86)
        │  sifive_test 0x5555 on success
        ▼
wfi idle
```

```
make qemu-riscv
```

Physical sketch (128 MiB guest, RAM at `0x80000000`):

| Range | Use |
| --- | --- |
| `0x00100000` | sifive_test finisher |
| `0x10000000` | UART0 (16550) |
| `0x80200000` | Kernel `.text` (OpenSBI payload) |
| `0x81000000–0x88000000` | Frame allocator window (arch fallback; no FDT mmap) |

No PLIC virtio, no `sret` userspace, no FDT mmap parser. Serial
prints `[mm] mmap: fallback (no Multiboot on this HAL)`.

## Boot (aarch64 / QEMU virt)

Thin v0.1 of the port — **kmain + serial + `aether_core` self-check**,
not EL0. Same fabric, map API, bank-color, and CDT checks. New
trampoline only. **Not** a product-class second kernel.

```
QEMU -machine virt,gic-version=2 -cpu cortex-a72 -kernel build/aether-aarch64.elf
        │  -semihosting -nic none; x0=dtb; EL1 (or EL2 → EL1 in the trampoline)
        ▼
boot/aarch64/trampoline.S
        │  park extra PEs, PL011 hello
        │  TTBR0 identity-map 4 GiB (1 GiB blocks)
        ▼
kernel::kmain  (Rust, aarch64-unknown-none)
        │  UART, frames, heap, VBAR, GICv2 + CNTV
        ▼
init::run_kernel_selfcheck   (same aether_core path as x86)
        │  Angel SYS_EXIT 0 on success
        ▼
wfi idle
```

```
make qemu-aarch64
```

Physical sketch (128 MiB guest, RAM at `0x40000000`):

| Range | Use |
| --- | --- |
| `0x08000000` | GICv2 distributor |
| `0x08010000` | GICv2 CPU interface |
| `0x09000000` | PL011 UART |
| `0x40080000` | Kernel `.text` |
| `0x41000000–0x48000000` | Frame allocator window (arch fallback; no FDT mmap) |

No EL0, no virtqueue, no GICv3, no FDT mmap parser. Extra PEs stay
parked. Serial prints `[mm] mmap: fallback (no Multiboot on this HAL)`.

## HAL ports

RISC-V and aarch64 are the HAL-split test:

1. New `boot/<arch>` + linker script.
2. Implement `kernel/src/arch/<arch>`: console, timer, irq ack, page tables.
3. Keep `aether-core` / `aether-hal` unchanged.

The fabric does not encode x86. aarch64 repeated the RISC-V recipe
(PL011 + GIC timer + TTBR). Ring-3 / virtqueue stay x86 until a later
cut. Neither thin port is product-class.

## SMP

Year-1 H2 **smoke**, not a product scheduler.

`arch::irq::smp_start_aps` (x86_64) copies a 16-bit trampoline to
`0x8000`, sends INIT-SIPI to APIC ID 1, and waits for `ap_entry`.
QEMU `-smp 2` (`make qemu-smp`) brings the AP up; `make qemu` / `make qemu-ci`
stay uniprocessor and time out cleanly ("UP only").

What this cut does:

- Per-CPU `PerCpu` via `IA32_GS_BASE` (`cpu_id` at `gs:0`, local tick).
- Fixed IPI vector 48 increments the AP's local tick (APIC EOI).
- BSP + AP drive `TileScheduler::pick` / `steal` on a shared ready pool
  (APs prefer steal). Serial proof: `[smp] SMP smoke ok (2 harts)`.
- Host test: `TileScheduler::drive_two_cpu_tiles`.

What it does not do:

- APs never enter ring-3. `/init` and `kthread-B` stay BSP-only.
- No more than one AP (APIC ID 1). RISC-V extra harts and aarch64 extra
  PEs stay parked.
- Not a Linux-style CFS, not a coherence claim, not a benchmark.

## Userspace

`/init` is a **static non-PIE ELF64** (`ET_EXEC`, `EM_X86_64`) linked at
`0x0200_0000`. There is no ramfs or virtio-blk in this cut: `make qemu`
builds `user/init`, copies the ELF to `build/init.elf`, and the kernel
`include_bytes!` the blob. The loader copies `PT_LOAD` segments into the
identity-mapped user window and `iretq`s to `e_entry` with CS=`0x23`.

An optional second static ELF, `/probe`, is linked at `0x0240_0000`
(`user/probe`, `build/probe.elf`). It yields only and does not
`SYS_EXIT`.

Each ring-3 task has its **own PML4**: the trampoline identity 4 GiB
is cloned, USER is set only on that task's 2 MiB window, and the other
user window is unmapped. The kernel CR3 (boot tables at `0x1000`) stays
supervisor-only. Context switch writes CR3. CR4.SMEP and CR4.SMAP are
enabled on the BSP and on AP 1; `SFMASK` clears `RFLAGS.AC` and
`STAC`/`CLAC` wrap user copies. This is **not** higher-half, KPTI,
KASLR, or a POSIX MM.

Entry is `syscall` (STAR / LSTAR / SFMASK, EFER.SCE). Same-thread return
is `sysretq`; a context switch returns via `iretq`. Well-known CPtrs
minted before the drop: `0` = fabric endpoint, `1` = accel queue.
`SYS_SEND` / `SYS_RECV` / `SYS_MAP` / `SYS_ACCEL_*` `require()` the cap
before touching the object.

A kernel companion thread (`kthread-B`) shares the PIT quantum with
`/init` so preemption is visible on the serial log. `SYS_RECV` on an
empty endpoint and `SYS_ACCEL_WAIT` before the SoftNPU runs actually
block and reschedule.

PIE / `ET_DYN` is rejected (no relocator).

Host proof of the clone/walk contract lives in `core/src/aspace.rs`.
QEMU prints `[mm] aspace isolate ok` after walking both CR3s.
