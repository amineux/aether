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
   `TypedWindow` (`Hbm` / `CxlMemStub` / `Dram`) is a Soft-SMMU pin stub
   for those places ([WINDOW.md](WINDOW.md)) — not CXL.mem silicon.
3. **`(place, local)` addresses.** Remote access is an explicit DMA/NoC
   Exchange. The HAL refuses a silent coherent load.
4. **Partition profiles.** Spatial slice + QoS (bw/credits) + blast-radius
   isolation. Scheduler and accel bind to a partition.
5. **Fence-ordered jobs.** submit → wait → complete (or timeout),
   credit-limited per partition — a CP-shaped seq/timeline, not a
   CUDA stream and not a silicon fence unit. Timeout is software.
   SoftChipletSync may further tag that seq with a visibility scope
   `{wave, CU, chiplet, package}`; that is still software, not UCIe.
6. **Named phases.** `Compute | Exchange | Barrier` tags on jobs and
   messages. The kernel does not fuse them.
7. **Kernel = submission shim + resource solver.** No ML graph IR or
   fusion in-kernel. Compilers own the ISA. The host ABI is shaped like
   PJRT/IREE HAL (Device, MemorySpace, Buffer, Executable, Event). See
   [ABI.md](ABI.md) and the working host session in [HOST.md](HOST.md).
8. **Cuts and Hodge classes remain capabilities.** A `SpectralCut` is a
   bound partition of the package graph; a `FlowClass` on every message
   selects gradient / curl / harmonic policy. An `OperatorKernelHandle`
   binds a collective topology to one of those classes.
   `SparsifiedCollective` may drop below-threshold harmonic before
   inject (integer milli; Hodge refuse unchanged). See [CUT.md](CUT.md).

What this document will not claim: a CUDA-style unified virtual address
space; seL4-level formal proofs; wafer-scale marketing that hides SRAM-first
placement; cache coherence across chiplets; that SoftChipletSync is a
Vulkan timeline or UCIe product; that SoftNoI-IS synthesizes NoI topology
or UniCNet; or that `TypedWindow` is
CXL.mem silicon.

## Boot (x86_64 / QEMU)

```
QEMU -kernel build/aether.elf
        │  multiboot1, 32-bit protected mode, paging off
        ▼
boot/x86_64/trampoline.S
        │  stash Multiboot EAX/EBX + KASLR slide at 0x7000
        │  identity-map 4 GiB (2 MiB pages)
        │  PML4[511] aliases first 2 GiB at 0xffffffff80000000
        │    (dedicated HH PDs; dual-map 8 MiB at +slide)
        │  enable PAE + EFER.LME + paging
        │  copy payload → LMA 0x400000
        │  apply .rela.dyn (addend + slide); unmap unused alias
        │  jump to VA 0xffffffff80400000+slide
        ▼
kernel::_start  (Rust, x86_64-unknown-none, higher-half)
        │  stack in BSS, serial, mmap → frames, heap, IDT, GDT/TSS, SYSCALL, PIT
        │  smp_start_aps: INIT-SIPI AP 1, per-CPU gs, IPI, work-steal smoke
        ▼
init::run_kernel_selfcheck
        │  host-identical fabric + cut + hodge + SoftNPU
        ▼
elfload::mount_boot_ramfs + load /init /probe
        │  seed ramfs from virtio-blk or embedded blobs; open named files
        │  clone per-task PML4; SMEP/SMAP; CR3 switch
        ▼
iretq → ring-3 /init
        │  syscall: debug_print, clone, recv (block), yield, send, map, accel_*
        ▼
SYS_EXIT → isa-debug-exit
```

The trampoline is ELF32 so `qemu-system-x86_64 -kernel` (multiboot1) will
load it. The Rust kernel is ELF64, objcopy'd to a flat binary, and
`.incbin`'d into the trampoline.

Physical sketch (128 MiB guest):

| Range | Use |
| --- | --- |
| `0x1000–0x7000` | Boot page tables (PML4/PDPT/4×PD identity) |
| `0x7000–0x700F` | Multiboot mailbox + KASLR slide + PIE reloc count |
| `0x71000–0x72FFF` | Dedicated HH PD0/PD1 (KASLR dual-map; identity PDs untouched) |
| `0x73000–0x76FFF` | KPTI trampoline (code + shadow IDT + entry stack); supervisor 4 KiB in user CR3 |
| `0x8000–0x8FFF` | AP SIPI trampoline + mailbox (`make qemu-smp`) |
| `0x100000` | Multiboot loader + embedded kernel blob |
| `0x400000` | Kernel `.text` LMA (after copy); VMA `0xffffffff80400000+slide` |
| `0x0200_0000–0x0220_0000` | `/init` ELF + user stack (USER 2 MiB in `/init` PML4 only) |
| `0x0240_0000–0x0260_0000` | `/probe` ELF + user stack (USER 2 MiB in `/probe` PML4 only) |
| `0x0280_0000–0x0280_1000` | Shared COW template (USER 4 KiB, RO until write; same PA in `/init` + `/probe`) |
| `0x02A0_0000–0x02C0_0000` | virtio-blk window (vring + AETHFS01 image; kernel identity island; reserved) |
| `0x02C0_0000–0x02C1_0000` | Growable `SYS_MMAP` window (anon 4 KiB USER on the task CR3; not identity) |
| mmap type-1, clip 16 MiB, cap 128 MiB | Frame allocator (user images reserved). QEMU `-m 128M` is typically `0x0100_0000–0x07fe_0000` (ACPI reserved at the top) |

The boot path parses the Multiboot1 mmap (Multiboot2 parser is
host-tested). Type-1 regions below 16 MiB are printed then clipped so
the trampoline / page tables / AP SIPI / kernel image stay out of the
free pool. Missing mmap is an explicit arch-window fallback, not a
silent 128 MiB map. Higher-half (`ffffffff80000000+PA`) plus a
boot-time KASLR slide (0 / 16 / 32 MiB dual-map; CI forces
`kaslr=1`) plus PIE `.rela.dyn` apply and unused-alias unmap
is landed. The link-time VA is not usable when the slide is
non-zero.
KPTI user CR3 (no HH, no identity DMA, 4 KiB trampoline) is landed
as a documented subset — not Meltdown-complete. PCID tags those
CR3 switches when CPUID.1:ECX[17] is set. TCG QEMU cannot
advertise `+pcid` (`make qemu-pcid-ci` requests it and accepts
the TCG warning + fallback; KVM may print `[mm] pcid ok`).
`make qemu-nopcid-ci` forces `-pcid`. A documented COW subset maps
one shared 4 KiB USER page at `0x0280_0000` into `/init` and
`/probe`; a write fault copies the frame. Kernel CR3 identity is
torn down except SIPI / mailbox / trampoline / virtio-blk / APIC
islands. SoftNPU DMA goes Soft SMMU IOVA → guest PA → HH.

## Crate graph

```
aether-core     alloc-free: caps, fabric, arenas, sched, SoftNPU math, ramfs, demo
     ▲
aether-hal      AccelDevice / Console / Timer
     ▲
aether-drivers  AccelMmio virtqueue + SoftNpuDevice + SoftCommandProcessor + IreeShapedCp + PartnerNpuStub
     ▲
     ├── aether-kernel   arch, mm, syscall/sysret + ecall/sret, ELF loader, tasks
     ├── aether-pjrt     std host shim: abi nouns → IreeHalCmd → IreeShapedCp (SoftNPU = qemu demo)
     └── aether-accel-client  doorbell: same frozen IreeHalCmd (not PJRT, not MicroPerceptron)
examples/partner-hello  host clone-and-run of that packet (no QEMU rebuild)
user/init       static non-PIE ELF64 `/init` (x86 @ 0x2000000, RISC-V @ 0x82000000)
user/probe      optional second static ELF64 (own PML4 @ 0x2400000)
```

`aether-core` is the portable specification. Host tests execute the same
`run_boot_demo()` the kernel prints. `aether-pjrt` is host-only (not
linked into the kernel); see [HOST.md](HOST.md).

## Module map (kernel)

| Path | Responsibility |
| --- | --- |
| `kernel/src/arch/x86_64` | UART, IDT/PIC, PIT, GDT/TSS, SYSCALL MSRs, SMP (`gs` / APIC) |
| `kernel/src/mm` | Multiboot mmap → frames, bump heap, HH + per-task PML4 clone, SMEP/SMAP, USER bits |
| `core/src/mmap.rs` | Host-tested Multiboot1 / Multiboot2 mmap parser + frame plan |
| `kernel/src/syscall.rs` | Numbered ABI; ring-3 trap dispatch + cap checks |
| `kernel/src/task.rs` | PIT preemption, yield, blocking recv/accel_wait, `SYS_CLONE` |
| `kernel/src/elfload.rs` | Static ELF64 loader (ramfs `open` `/init` / `/probe`) |
| `kernel/src/virtio_blk.rs` | x86 legacy virtio-blk PCI I/O → AETHFS01 → ramfs seed |
| `core/src/ramfs.rs` | Host-tested in-kernel ramfs (named files; seed from blk or blobs) |
| `core/src/bootfs.rs` | Host-tested AETHFS01 pack/parse |
| `kernel/src/world.rs` | Init cap table, fabric, arenas, virtqueue SoftNPU |
| `kernel/src/init.rs` | Kernel-side `run_boot_demo` + blast-radius + SID-at-submit + firewall + SoftGreenCtx + SoftSFI + PASID/SVA + OperatorInject + SoftNoI-IS clips |
| `core/src/elf.rs` | Host-tested ELF64 parser |
| `core/src/aspace.rs` | Host-tested identity + HH alias clone + USER-local walk |
| `core/src/preempt.rs` | Host-tested RR + block/wake queue |
| `core/src/sysnr.rs` | Frozen syscall numbers + user C ABI |
| `core/src/caps.rs` | Cap table |
| `core/src/fabric.rs` | Endpoints and messages |
| `core/src/arena.rs` | Bank-aware allocator + tenant color |
| `core/src/color.rs` | BankColor admit / refuse |
| `core/src/iommu.rs` | Soft SMMU STE→CD→S1/S2 walk + ATS invalidate + SET_SID latch + PASID/SVA mm↔SSID (not hardware) |
| `core/src/sid.rs` | Host1x-shaped SID-at-submit clip (two tenants / two SIDs; not a Tegra driver) |
| `core/src/window.rs` | TypedWindow stub (`Hbm`/`CxlMemStub`/`Dram` + SID); not CXL.mem silicon |
| `drivers/src/fakecp.rs` | SoftCommandProcessor (`CpCmd` + SET_SID-at-submit + XQueue + SoftGreenCtx + SoftChipletSync + SoftCCT + SoftNoI-IS + SoftCmdFirewall + PASID/SVA + IRQ/`retire_into`) |
| `drivers/src/firewall.rs` | SoftCmdFirewall copy-then-validate (Host1x lesson; cmd-stream integrity, not confidential GPU) |
| `drivers/src/softsfi.rs` | Soft-CP host for the toy SoftSFI sandbox (keeps `fakecp.rs` thin) |
| `drivers/src/sva.rs` | Soft-CP host for PASID/SVA (mm↔SSID bind; VA DMA; unmap→SSID TLB) |
| `drivers/src/opinject.rs` | Soft-CP host for OperatorInject (resident worker + SID/firewall; keeps `fakecp.rs` thin) |
| `drivers/src/noi.rs` | Soft-CP host for SoftNoI-IS XQueue admit (keeps `fakecp.rs` thin) |
| `drivers/src/ireecp.rs` | IreeShapedCp (`IreeHalCmd` + SET_SID-at-submit + IRQ/`retire_into`; `backend = 4`) |
| `qemu/` | Optional path-A `aether-accel` device (frozen BAR + SoftNPU I32 + host Soft-SMMU IOVA proof) |
| `core/src/sched.rs` | Tile scheduler + color gate + laplacian cut bind |
| `core/src/accel.rs` | Job desc + reference matmul |
| `core/src/observe.rs` | Event ring |
| `core/src/cut.rs` | ChipletSpectralCut + affinity graph (n≤32 Fiedler; enum n≤8) |
| `core/src/laplacian.rs` | `AffinityLaplacian` (`L = D − A`; n≤32 prototype placement) |
| `kernel/src/arch/riscv64` | UART0, stvec, SBI timer, PLIC + SoftNPU doorbell, Sv39 isolate, `sret`/`ecall` |
| `kernel/src/arch/aarch64` | PL011, VBAR, GICv2 + CNTV, TTBR0 isolate, EL0 `svc`/`eret` |
| `core/src/hodge.rs` | FlowHodgeQuota policy + quotas |
| `core/src/opkernel.rs` | OperatorKernelHandle (collective × Hodge class) |
| `core/src/sparsify.rs` | SparsifiedCollective (drop below-threshold harmonic) |
| `core/src/space.rs` | Typed `MemorySpace` + `(place, local)` |
| `core/src/activity.rs` | Fabric activity behind a uniform endpoint |
| `core/src/partition.rs` | Spatial slice + QoS + blast radius |
| `core/src/fence.rs` | CP-shaped timeline (seq / wait / complete; credit limit; timeout is software) |
| `core/src/chipsync.rs` | SoftChipletSync scoped timelines (wave/CU/chiplet/package) + hierarchical counters + SoftCCT elision + SoftNoI-IS advertisement |
| `core/src/greenctx.rs` | SoftGreenCtx fake SM/WQ partitions (70/30) + memcpy interference + migrate-to-yield (not MIG) |
| `core/src/softsfi.rs` | SoftSFI toy Soft-CP ISA + SFI verifier (GPU-AToLL shape; SID `base+bound`; SID-proved `atomic_add`; heap named `Unmodeled` refuse) |
| `core/src/sva.rs` | PASID/SVA clip (Linux SVA-shaped mm↔SSID; not ARM SVA / CUDA UVA) |
| `core/src/opinject.rs` | OperatorInject resident worker + versioned op table (memcpy/saxpy + hot-add scale; GPUOS / Mirage MPK; not NVRTC) |
| `core/src/noi.rs` | SoftNoI-IS fake NoI + Interference Score admit + software fabric-class tag (PARL/NoI metric; not topology synth, not UniCNet, not a vendor header) |
| `core/src/phase.rs` | Compute / Exchange / Barrier tags |
| `core/src/abi.rs` | PJRT/IREE-shaped host objects (no graph IR) |
| `host/aether-pjrt` | std host session: abi nouns → frozen `IreeHalCmd` → IreeShapedCp; Event create/record/wait on existing fences (not `GetPjRtApi`, not XLA) |
| `examples/accel-client` | Second consumer of the same 96-byte image (doorbell sketch; not a plugin) |
| `examples/partner-hello` | Host clone-and-run of that packet; [PARTNER.md](PARTNER.md). No QEMU |

## Boot (RISC-V / QEMU virt)

Documented subset — **S-mode kernel + U-mode `/init`**, not a
product-class second kernel. Same `aether_core` self-check, then
`sret` into a static ELF. SoftNPU is the in-kernel virtqueue (path
B). Completions are claimed on the PLIC (UART THRE software
doorbell). Extra harts stay parked.

```
QEMU -machine virt -kernel build/aether-riscv.elf
        │  OpenSBI (default -bios), S-mode, a0=hartid, a1=dtb
        ▼
boot/riscv64/trampoline.S
        │  park extra harts, UART0 hello
        │  Sv39 identity-map 4 GiB (1 GiB leaves)
        ▼
kernel::kmain  (Rust, riscv64gc-unknown-none-elf)
        │  UART, frames, heap, stvec, SBI timer, PLIC, World
        ▼
init::run_kernel_selfcheck   (same aether_core path as x86)
        ▼
elfload::mount_boot_ramfs + load /init
        │  seed ramfs from embedded riscv64 ELF; open `/init`
        │  clone per-task satp; U only on the 2 MiB window
        ▼
sret → U-mode /init
        │  ecall: debug_print, recv, yield, send, map, accel_*
        ▼
SYS_EXIT → sifive_test 0x5555
```

```
make qemu-riscv
```

Physical sketch (128 MiB guest, RAM at `0x80000000`):

| Range | Use |
| --- | --- |
| `0x00100000` | sifive_test finisher |
| `0x0c000000` | SiFive PLIC (hart 0 S-mode context 1) |
| `0x10000000` | UART0 (16550; THRE → PLIC source 10 SoftNPU doorbell) |
| `0x80200000` | Kernel `.text` (OpenSBI payload) |
| `0x81000000–0x88000000` | Frame allocator window (arch fallback; no FDT mmap) |
| `0x8200_0000–0x8220_0000` | `/init` ELF + user stack (U-bit 2 MiB in task satp) |
| `0x8300_0000–0x8400_0000` | SoftNPU arena banks (identity; reserved) |

PLIC is live; SoftNPU stays the in-kernel BAR (no virtio-mmio
`-device`, no `/probe`, no FDT mmap parser). Serial prints
`[plic] claim irq=10 SoftNPU used-ring`,
`[mm] mmap: fallback (no Multiboot on this HAL)`, and
`[init] U-mode /init`.

## Boot (aarch64 / QEMU virt)

Documented subset — **EL1 kernel + EL0 `/init`**, same fabric, map API,
bank-color, and CDT checks. **Not** a product-class second kernel.

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
        │  load /init @ 0x42000000 (own TTBR0, AP_EL0 on 2 MiB)
        ▼
eret → EL0 /init   (svc #0, numbers 0–10)
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
| `0x42000000–0x42200000` | `/init` ELF + user stack (AP_EL0 2 MiB in task TTBR0) |
| `0x43000000–0x44000000` | SoftNPU arena banks (identity; reserved) |

SoftNPU stays the in-kernel BAR (no virtio-mmio `-device`, no GICv3
doorbell, no `/probe`, no FDT mmap parser). Extra PEs stay parked.
Serial prints `[mm] mmap: fallback (no Multiboot on this HAL)`,
`[mm] aspace isolate ok`, and `[init] EL0 /init`.

## HAL ports

RISC-V and aarch64 are the HAL-split test:

1. New `boot/<arch>` + linker script.
2. Implement `kernel/src/arch/<arch>`: console, timer, irq ack, page tables.
3. Keep `aether-core` / `aether-hal` unchanged.

The fabric does not encode x86. aarch64 now also has EL0 `/init` +
`svc`/`eret` + TTBR0 isolate + in-kernel SoftNPU (timer/kthread
drain, not a GIC doorbell). RISC-V has U-mode `/init` + in-kernel
SoftNPU + a PLIC software doorbell. Neither port is product-class.

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

`/init` is a **static non-PIE ELF64** (`ET_EXEC`). On x86_64 it is
`EM_X86_64` linked at `0x0200_0000`. On RISC-V it is `EM_RISCV`
linked at `0x8200_0000` (QEMU virt RAM). An in-kernel **ramfs**
holds named files (`/init`, optional `/probe`). On x86, boot prefers
a virtio-blk AETHFS01 drive (`make qemu-blk`) and **seeds** those
names from the reserved window at `0x02A0_0000`; without a drive it
seeds the embedded blobs (`include_bytes!`). The loader `open`s
`/init` from ramfs and copies `PT_LOAD` into the identity-mapped
user window, then drops to user (`iretq` / `sret`). This is not
POSIX and not a general block layer. SoftNPU stays path B on stock
`make qemu`. Path A is an optional QEMU device (`qemu/`,
`make qemu-accel`).

An optional second static ELF, `/probe`, is linked at `0x0240_0000`
(`user/probe`, `build/probe.elf`). It yields only and does not
`SYS_EXIT`.

Each ring-3 task has its **own KPTI PML4**: USER is set only on that
task's 2 MiB window, the other user window is unmapped, `PML4[511]`
is empty (no kernel HH), and the identity 4 GiB is not present.
Four supervisor 4 KiB pages at `0x73000` are the syscall/IRQ
trampoline. The kernel CR3 (boot tables at `0x1000`) keeps HH plus
identity *islands* (low 2 MiB SIPI / mailbox / trampoline, virtio-blk,
APIC). SoftNPU uses `KernelDma` + Soft SMMU, not a 4 GiB identity
window. Context
switch stays on kernel CR3 while in the kernel; the trampoline
loads the user CR3 just before `iretq`. CR4.SMEP and CR4.SMAP are
enabled on the BSP and on AP 1; `SFMASK` clears `RFLAGS.AC` and
`STAC`/`CLAC` wrap user copies. Kernel `.text` is linked at
`0xffffffff80400000` and runs at that VA plus a boot-time slide
after `.rela.dyn` is applied; the unused canonical alias is
unmapped when the slide is non-zero. PCID (when CPUID advertises
it) tags kernel vs user `mov cr3` so the KPTI switch is not a
full TLB flush; INVPCID (or bit-63-clear) covers remap. A
documented COW subset maps one shared 4 KiB USER page at
`0x0280_0000` read-only in `/init` and `/probe`; a write fault
copies the frame on that aspace only. This is **not**
Meltdown-complete, a secret slide, `fork`, or a POSIX MM.

x86 entry is `syscall` (STAR / LSTAR / SFMASK, EFER.SCE). Same-thread
return is `sysretq`; a context switch returns via `iretq`. RISC-V
entry is `ecall` (`a7` = number); return is `sret`. Well-known CPtrs
minted before the drop: `0` = fabric endpoint, `1` = accel queue.
`SYS_SEND` / `SYS_RECV` / `SYS_MAP` / `SYS_ACCEL_*` `require()` the cap
before touching the object.

A kernel companion thread (`kthread-B`) shares the timer quantum with
`/init` so preemption is visible on the serial log. `SYS_RECV` on an
empty endpoint and `SYS_ACCEL_WAIT` before the SoftNPU runs actually
block and reschedule.

`SYS_CLONE` (nr 10) starts a second user thread on `/init`'s PML4 /
satp: own stack and register state, same USER window. That is **not**
Linux `clone` and not `/probe` (a second ELF with its own aspace).
The child prints `[init] user-thread share-aspace` and yields;
`SYS_EXIT` is still guest-wide.

`SYS_MMAP` (nr 11) grows that aspace with anonymous 4 KiB USER pages
in a reserved window. That is **not** POSIX `mmap`: no file, no
`MAP_SHARED`. SoftNPU stays on kernel CR3; Soft SMMU is unchanged.

PIE / `ET_DYN` is rejected (no relocator). RISC-V has no `/probe` in
this cut. SoftNPU on RISC-V is the same in-kernel virtqueue; used-ring
completions are claimed on the PLIC (UART THRE doorbell), not a
virtio-mmio device.

Host proof of the aspace clone/walk contract lives in
`core/src/aspace.rs` (`IdentityAs` for x86, `Sv39As` for RISC-V),
including the shared-map case `SYS_CLONE` uses and the anonymous
grow `SYS_MMAP` adds.
QEMU prints `[mm] aspace isolate ok` after walking both CR3s
and `[ramfs] open /init ok` after the boot ramfs mount
(`[blk] virtio-blk seed /init` when a drive is attached).
