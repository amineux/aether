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
| ramfs / virtio-blk for `/init` | **done** (in-kernel ramfs; seed from blobs; virtio-blk deferred) |
| Per-task PML4 / SMEP / SMAP | **done** (x86 subset: CR3 switch + USER-local 2 MiB windows) |
| User-level threads (clone) | **done** (additive `SYS_CLONE=10`; share caller's PML4/satp; not Linux clone) |

## Month 3–4

| Item | Status |
| --- | --- |
| Virtqueue-shaped MMIO (doorbell + used-ring IRQ) | **done** (in-kernel BAR; SoftNPU backend; path B canonical) |
| Custom QEMU `virtio-accel` device | deferred (path A optional later; golden MMIO trace locks the BAR) |
| `IommuMap` pin/translate; refuse without Memory+MAP | **done** (Soft SMMU; non-identity IOVA) |
| Soft SMMU / software stream IDs | **done** (per-stream IOVA namespaces; not hardware) |
| Hardware SMMU / stream IDs | not started (no SID programmed on a real SMMU) |
| Arena tenant/bank color; Compute refuse + Exchange/transfer | **done** |
| Partner `AccelDevice` sketch (`PartnerNpuStub`) | **done** (no-op; not a partnership; not a CP path) |
| SoftCommandProcessor (`backend = 3`) | **done** (packed `CpCmd` + Soft SMMU SID + IRQ/fence; host tests) |

## Month 5–6 (this cut): Portability & partners

Landed:

- **RISC-V virt bring-up.** `boot/riscv64` trampoline + Sv39 identity
  map; `kernel/src/arch/riscv64` UART / SBI timer / stvec. The later
  S-mode userspace cut (below) adds `sret` / `ecall` `/init`.
- **AffinityLaplacian.** First-class `L = D − A` in `core/src/laplacian.rs`
  with integer Rayleigh, Fiedler-ish power iteration, heat-kernel and
  commute-time helpers. The n≤32 placement cut (below) extends this.
- **Diligence pack.** [DILIGENCE.md](DILIGENCE.md) — what ships, what is
  stubbed, how to plug `AccelDevice`, security invariants, CI, non-claims,
  and a design-win narrative that does not invent a partner.
- **Deep-dive agenda.** [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) — a
  60–90 min script for an NVIDIA / ASIC OS team. No meeting is claimed.

Honest limits of this cut:

- RISC-V userspace is a **documented subset**, not a second product
  kernel. See the S-mode userspace section.
- Fiedler is integer power iteration (n≤32 host-tested), not a
  production eigensolve and not GiFt-Placer. Enumeration stays at n≤8.
- Nobody from a silicon team has reviewed this. The agenda is so they
  could.
- SoftCommandProcessor is a **software CP**, not a silicon driver. It
  uses Soft SMMU (`StreamId` + bind/abort). QEMU still demos SoftNPU.
  `PartnerNpuStub` is unchanged.

## Year-2: aarch64 thin HAL (this cut)

Landed as a **thin HAL test**, same recipe as the original RISC-V
bring-up — **not** a second product kernel. The later EL0 userspace
cut (below) adds `eret` / `svc` `/init`.

- `boot/aarch64` trampoline + TTBR0 identity map (4 GiB, 1 GiB
  blocks). Drops EL2→EL1 when QEMU starts us in the hypervisor.
- `kernel/src/arch/aarch64`: PL011 UART, GICv2 + CNTV (PPI 27),
  VBAR_EL1. `make qemu-aarch64` / `make qemu-aarch64-ci` use QEMU
  `-machine virt,gic-version=2 -cpu cortex-a72` (documented in the
  Makefile).
- Same `aether_core` self-check as x86 / RISC-V (Soft SMMU pin
  refuse, CDT banner). `aether-core` / `aether-hal` / syscalls
  unchanged. Extra PEs stay parked.
- The thin-HAL cut had **no** EL0. The EL0 userspace cut (below)
  adds `eret` `/init`. Still no GICv3, no FDT mmap parser.
  Success is Angel semihosting `SYS_EXIT` (`-semihosting`).

Do not treat this as a product-class second architecture.

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

## Year-1 H2: SMP smoke (this cut)

Landed on x86_64 QEMU only — **not** a product SMP kernel, **not**
per-task isolation:

- `smp_start_aps` does INIT-SIPI to APIC ID 1. Trampoline lives at
  `0x8000`. `make qemu-smp` / `make qemu-smp-ci` use `-smp 2`.
- Per-CPU `gs` (`IA32_GS_BASE` → `PerCpu { cpu_id, ticks }`).
- One fixed IPI (vector 48) so the AP's local tick moves.
- Work-steal: BSP `pick` + AP `steal` on the existing `TileScheduler`
  ready pool. Serial: `[smp] SMP smoke ok (2 harts)`. Host test:
  `drive_two_cpu_tiles`.
- APs stay in kernel mode. `/init` + SoftNPU virtqueue stay BSP-only.
  `make qemu-ci` is still UP and must keep working.

RISC-V extra harts stay parked.

## Year-1 H2: per-task PML4 + SMEP/SMAP (this cut)

Landed on x86_64 only — **documented subset**, not a POSIX MM, not
KPTI / KASLR (higher-half alias is a later cut):

- Each ring-3 task (`/init` @ `0x2000000`, optional `/probe` @
  `0x2400000`) gets its own PML4: trampoline identity map cloned,
  USER only on that task's 2 MiB ELF window, the other user window
  unmapped (`P=0`). Kernel CR3 stays the boot tables (supervisor-only).
- Context switch writes CR3 for user↔kernel and user↔user.
- CR4.SMEP + CR4.SMAP on the BSP and on AP 1. `SFMASK` clears
  `RFLAGS.AC`; `STAC`/`CLAC` wrap user copies.
- `/init` is still a static non-PIE ELF. `/probe` is a second static
  non-PIE ELF that yields only (does not `SYS_EXIT`).
- Host test: `core/src/aspace.rs` walks two synthetic maps.
- QEMU: `[mm] aspace isolate ok` + `make qemu-ci` greps SMEP/SMAP.

Still stubbed: KASLR, KPTI, PCID, COW, growable `mmap`,
per-task cap tables, APs in ring-3. SoftNPU still touches `/init`
tensors through the **intentional** kernel identity window (DMA).

## Year-2 H1: cap CDT / revoke (this cut)

Landed as a **small** derivation tree — inspired by seL4, **not** a
CNode/MDB and **not** a proof claim:

- `Capability` stores `parent`; the node is `(tenant, generation)`.
- `derive` and GRANT-copy set the parent edge; GRANT-move relocates a
  slot and does not walk descendants.
- `revoke(parent)` empties the lineage in that table.
  `revoke_in(parent, others)` empties grant-children in named tables.
- Host tests: mint child → revoke parent → child unusable; unrelated
  caps live. Boot demo + QEMU `[cdt] revoke descendants ok`.
- No new syscall (0–8 frozen). Kernel World still has one shared
  `CapTable`.

Do not treat this as the SpecForge Y2H1 calendar (CXL objects and
Laplacian-in-sched stay unscheduled).

## Multiboot mmap (this cut)

Landed as a **documented subset**, not a general physical MM:

- x86_64 trampoline stashes Multiboot EAX/EBX at `0x7000`. `mm::init`
  parses the mmap (Multiboot1 on `make qemu`; Multiboot2 parser is
  host-tested for a future loader). Type-1 regions feed the bitmap.
- Usable RAM below 16 MiB is printed, then clipped (boot page tables,
  AP SIPI, trampoline, kernel image). Regions above the 4 GiB identity
  map are ignored. Bitmap cap remains 128 MiB of frames.
- Missing / empty mmap is an explicit serial fallback to the arch
  window — not a silent 128 MiB @ 16 MiB lie. RISC-V / aarch64 have
  no Multiboot and take that fallback (no FDT parser).
- Host tests in `core/src/mmap.rs`. QEMU: `[mm] mmap: multiboot1` plus
  the planned window. `make qemu-ci` greps the parse line.

Still stubbed: KASLR, hotplug, FDT, managing RAM past the identity
4 GiB (HH is only a 2 GiB alias of low PA).

## OperatorKernelHandle (this cut)

Landed as a **research kernel surface**, not a compiler and not a
collective ISA:

- `CapKind::OperatorKernel` names an `OperatorKernelHandle`
  (`core/src/opkernel.rs`): collective topology (`Tree` / `Ring` /
  `Torus`) plus one bound Hodge class.
- Bind refuses Tree+Curl (`CurlOnTree`) and Tree+Harmonic
  (`HarmonicTreeReduce`) — the same policy as `FlowHodgeQuota`.
  Ring and torus never set `TREE_OFFLOAD`.
- Inject builds a fabric header from the handle (flow +
  TREE_OFFLOAD / RING_RESERVE) and `Fabric::send` admits it.
  A requested class other than the bound one is `ClassMismatch`
  before quota.
- Mint / derive go through the existing cap table. Revoke of a
  parent empties descendants (CDT already landed).
- Host tests lock bind / inject / refuse. Boot demo + serial
  `[opkernel] tree+gradient inject + harmonic-tree REFUSE`.
  No new syscall (0–8 frozen). No QEMU collective engine.

## SparsifiedCollective

Landed as a **research kernel surface**, not a spectral compiler and
not an eigensolver:

- `SparsifiedCollective` (`core/src/sparsify.rs`) wraps an
  `OperatorKernelHandle` (or a FlowClass header) plus integer milli
  energy and a threshold.
- Hodge refuse still wins: Tree+Harmonic is `HarmonicTreeReduce`
  even when energy is below the threshold (Drop does not skip policy).
- Harmonic energy strictly below the threshold is dropped before
  inject (no enqueue, quota untouched). At or above, inject as
  Harmonic. Gradient and Curl pass through; their energy is ignored.
- Caps stay on `CapKind::OperatorKernel`. No new syscall (0–8
  frozen). No QEMU collective engine.
- Host tests lock drop / keep / Gradient-Curl / refuse. Boot demo
  + serial `[sparsify] below-threshold DROP + above KEEP + harmonic-tree REFUSE`.

## Hardware fence/timeline (this cut)

Landed as a **software model** of a CP-shaped timeline — **not** a
silicon fence unit, **not** CUDA streams:

- `TimelineId` + monotonic seq (`FenceId`). The seq is what
  `AccelJobDesc.fence_id` and `CpCmd` already carry (`u64`, ABI
  unchanged).
- `submit` allocates seq + credit. `wait` polls the retired
  watermark. `complete` is in-order CP retire.
- An issued but not-yet-retired `wait_for` is allowed (the CP would
  stall). A never-issued pred is refused. That is not the old
  host-side "refuse submit until pred completes" shortcut.
- `timeout` is a software overlay: it releases a credit without
  claiming a device IRQ.
- SoftCommandProcessor and SoftNPU retire through
  `Timeline::complete` / `retire_into`. The QEMU used-ring IRQ
  (software doorbell) now retires the World timeline the same way.
- Host tests: submit → wait → complete; credit exhaustion; timeout
  refuse; multi-job in-order. Boot demo + serial
  `[fence] timeline seq#… submit -> wait -> complete`.
- No new syscall (0–8 frozen). `AccelDevice` / `UserAccelJob`
  unchanged.

This is still not a hardware fence. QEMU does not write a silicon
timeline register.

## F16/F32 dtypes (this cut)

Landed as **software IEEE** on SoftNPU — **not** a tensor ISA, **not**
libm, **not** a hard-float HAL, **not** a FLOP benchmark:

- `DType`: `I32=0` (unchanged), `F16=1`, `F32=2`. Jobs, `AccelJobWire`,
  and `CpCmd` carry the byte. Unknown values stay `UnsupportedDType`.
- SoftNPU matmul/wave: integer-only `binary32` add/mul; F16 converts
  through those helpers. Subnormals flush to zero.
- A DMA view without `load_u16` refuses F16 (host-tested).
- `UserAccelJob` / syscalls 0–8 unchanged (`/init` stays I32).
- Host tests + boot demo + serial
  `[accel] SoftNPU F32/F16 soft-float 2x2 ok (software IEEE; not a tensor ISA)`.

A real tile ISA is still a compiler concern.

## RISC-V S-mode userspace (this cut)

Landed as a **documented subset**, not a product-class second
architecture, not a PLIC virtio port, not `/probe` on this HAL:

- `sret` into a static non-PIE riscv64 `/init` at `0x8200_0000`
  (RAM lives at `0x8000_0000`; the x86 `0x0200_0000` hole is not RAM).
  Syscall via `ecall` (`a7` = number; numbers 0–10 match [ABI.md](ABI.md)).
- Per-task Sv39: clone the trampoline identity map, split the RAM 1 GiB
  leaf into 2 MiB pages, U-bit only on that task's window. Host twin in
  `core/src/aspace.rs` (`Sv39As`). `sstatus.SUM` wraps user copies.
- SoftNPU / virtqueue is the **in-kernel BAR** (same as x86). The
  later PLIC cut (below) adds a software doorbell on that BAR. No
  virtio-mmio device, no FDT mmap. Extra harts stay parked. No
  `/probe` ELF on this arch.
- `make qemu-riscv` / `make qemu-riscv-ci` greps
  `[init] U-mode /init`, `ecall debug_print ok`,
  `U-MODE /init VIA ECALL/SRET`, and aspace isolate.

Still stubbed after that cut: real virtio-mmio, FDT mmap, extra-hart
SMP, `/probe`, product-class second kernel. aarch64 EL0 is a later
documented subset (below).

## SpecForge virtio path B (this cut)

Landed as the Y1H1 virtio **path B** decision — **not** a QEMU
`-device`, **not** an upstream virtio device:

- [ACCEL.md](ACCEL.md) ADR: path A (custom QEMU virtio-accel) vs path B
  (in-kernel BAR is the canonical demo). **B chosen.** Path A stays
  optional later.
- BAR layout is **frozen** (`magic`, `version`, `status`, `qsize`,
  `doorbell`, `used_idx`). SoftNPU behind `AccelMmio` remains what
  `make qemu` runs. Stock QEMU only.
- Host golden MMIO trace records cfg / doorbell / used-ring accesses
  for one SoftNPU submit/complete
  (`drivers/src/{mmio,softnpu,virtio_accel}.rs`).
- No new QEMU device C code. No vendor claim.

## AffinityLaplacian n≤32 placement

Landed as a **prototype eigensolve** — **not** GiFt-Placer, **not** a
production package solver, **not** an EDA replacement:

- `MAX_VERTS = 32`. Masks stay `u32` (`vert_mask` handles n=32).
- `SpectralCut::from_fiedler` / `from_placement` take a Fiedler
  median-cut of `AffinityLaplacian`. Host tests: n=16 and n=32
  `two_chiplet_mesh` smokes recover the chiplet bipartition.
- `SpectralCut::min_balanced` still enumerates for n ≤ 8
  (`ENUM_MAX`). Above that it is `TooLarge` — O(2ⁿ) is not a
  placement path.
- `TileScheduler::bind_laplacian_cut` installs that cut. `pick`
  refuses CrossCut on a bound cut; BIND is still required on
  `bind_place`. A soft Fiedler-side score hint is not a refuse.
- Complexity (dense integer): iterate O(iters·n²), commute O(n³).
  No libm. No new syscall. AccelDevice / qemu arch CI unchanged.

## Higher-half kernel map (this cut)

Landed as a **documented subset**, not KASLR, not KPTI, not PCID,
not COW, not a POSIX MM:

- x86_64 kernel is linked at the classic `-2 GiB` map
  (`0xffffffff80400000` = `0xffffffff80000000 + 4 MiB` LMA).
  The trampoline still copies the flat binary to physical `0x400000`,
  then jumps to the higher-half `_start`.
- `PML4[511]` aliases the first 2 GiB of the identity PDs into that
  window (`PDPT[510/511] → PD0/PD1`). QEMU `-m 128M` and the kernel
  image fit. `code-model=kernel`.
- **Identity 4 GiB stays mapped on purpose.** SoftNPU `IdentityDma`,
  page-table walks (tables addressed by PA), AP SIPI @ `0x8000`,
  Multiboot mailbox @ `0x7000`, and user ELF windows (`0x2000000` /
  `0x2400000`) still use the low map. Do not treat the leftover
  identity window as a bug.
- Per-task PML4 clones copy `PML4[511]`, so syscall/IRQ handlers
  remain reachable after CR3 switch. USER bits stay off on HH.
- Host test: `core/src/aspace.rs` walks the HH alias. QEMU:
  `[mm] higher-half ok` + RIP in the `-2 GiB` map.
  `make qemu-ci` greps that line.
- RISC-V / aarch64 keep their identity maps. No new syscall.
  `AccelDevice` unchanged.

Still stubbed: KASLR (slide the image), KPTI (separate user CR3
without kernel HH), PCID, COW, tearing down the identity window.

## User-level threads / `SYS_CLONE` (this cut)

Landed as a **documented subset**, not Linux `clone`, not `fork`,
not POSIX pthreads, not per-thread cap tables:

- Additive syscall **10** (`clone(entry, stack, flags)`). Numbers
  0–9 are unchanged. `flags` must be 0. Documented in [ABI.md](ABI.md).
- Child shares the caller's PML4 / satp (same 2 MiB USER window).
  Own kernel stack, own `InterruptFrame`, own user stack. Context
  switch skips CR3/satp when the root is unchanged.
- Child does not return from clone: it starts at `entry` with
  arg0 = tid. `/init` carves an 8 KiB BSS stack and prints
  `[init] user-thread share-aspace`. Parent prints
  `[init] clone ok (shared aspace)`.
- Scheduler already had a ready pool (`CpuQueue`, `MAX_THREADS=8`).
  The task table grew to 8 slots so `/init` + `/probe` + clone +
  `kthread-B` all stay runnable. `SYS_EXIT` is still guest-wide.
- Host tests: `user_clone_pair_ok`, shared-aspace walk, four-thread
  RR. QEMU: those two `/init` lines plus `[sched] clone tid=`.
  `make qemu-ci` / `qemu-riscv-ci` / `qemu-smp-ci` grep them.
- AccelDevice / `UserAccelJob` / SoftNPU / HH identity DMA / SMEP
  / SMAP / RISC-V U-mode / enter_user PIT snapshot unchanged.

Still stubbed: `CLONE_*` flags, TLS, per-thread exit, a new aspace
(`fork`), per-task cap tables. `/probe` is still a second ELF with
its own PML4 — that is not `SYS_CLONE`.

## In-kernel ramfs for `/init` (this cut)

Landed as a **documented subset**, not POSIX, not a block device,
not virtio-blk:

- `RamFs` in `core/src/ramfs.rs`: flat named files over borrowed
  slices. `seed` / `open` / `read` / `bytes`. Host tests cover
  `/init` + `/probe`, chunked read, missing/duplicate/bad names.
- Boot seeds `/init` (and x86 `/probe`) from the existing embedded
  ELF blobs. The loader **opens those names** and copies `PT_LOAD`
  from the ramfs bytes — it does not call `include_bytes!` at the
  load site.
- QEMU: `[ramfs] open /init ok` (plus `/probe` on x86) and
  `[boot] loaded /init … (static non-PIE, ramfs)`.
  `make qemu-ci` / `qemu-smp-ci` / `qemu-riscv-ci` grep the open line.
- No new syscall. Numbers 0–10 stay as in [ABI.md](ABI.md).
  User `open`/`read` is not this cut.
- SoftNPU path B, Soft SMMU, higher-half identity DMA, RISC-V
  U-mode, `SYS_CLONE`, and the enter_user PIT snapshot are
  unchanged.

**virtio-blk** is a follow-up: a QEMU `-drive` plus a virtio-blk
driver on x86 would be a larger cut and must not disturb the
in-kernel SoftNPU BAR. A later cut can copy blocks into a reserved
window and `seed` the same `/init` / `/probe` names.

## RISC-V PLIC + SoftNPU doorbell (this cut)

Landed as a **documented subset**, not virtio-mmio, not a QEMU
`-device`, not a product-class second kernel:

- SiFive PLIC at `0x0c000000` on QEMU virt. Hart 0 S-mode is
  context 1 (OpenSBI keeps M). Priority / enable / threshold /
  claim-complete are programmed in `kernel/src/arch/riscv64/plic.rs`.
- SoftNPU stays the **in-kernel AccelMmio BAR** (path B; same
  frozen offsets). Full virtio-mmio is still open.
- Software doorbell: after `AccelDevice::submit` kicks the BAR,
  the kernel raises UART0 THRE → PLIC source 10. The SEI handler
  claims source 10, acks THRE, and `World::run_pending_accel`
  services the same used ring the x86 kthread poll path does.
  SSIP remains enabled as a spare trap; it is not the doorbell.
- `make qemu-riscv` / `make qemu-riscv-ci` greps
  `[boot] PLIC hart0 S-mode`,
  `[plic] claim irq=10 SoftNPU used-ring`, and
  `[accel] used-ring IRQ job#`.
- No new syscall (0–10 frozen). `AccelDevice` / `UserAccelJob`
  unchanged. x86 higher-half SoftNPU is untouched. aarch64 EL0
  (below) is a later cut.

Still stubbed: virtio-mmio BAR, FDT mmap, extra-hart SMP, `/probe`
on this arch, product-class second kernel.

## aarch64 EL0 userspace (this cut)

Landed as a **documented subset**, not a product-class second
architecture, not GICv3, not virtio-mmio, not `/probe` on this HAL:

- `eret` into a static non-PIE aarch64 `/init` at `0x4200_0000`
  (RAM lives at `0x4000_0000`; the x86 `0x0200_0000` hole is not RAM).
  Syscall via `svc #0` (`x8` = number; numbers 0–10 match [ABI.md](ABI.md)).
- Per-task TTBR0: clone the trampoline identity map, split the RAM
  1 GiB block into 2 MiB pages, AP_EL0 only on that task's window.
  Host twin in `core/src/aspace.rs` (`Ttbr0As`). No PAN (cortex-a72
  is v8.0); EL1 copies do not need a SUM analogue.
- SoftNPU / virtqueue is the **in-kernel BAR** (same as x86).
  Completions drain on the CNTV tick and kthread poll — not a GIC
  SPI doorbell. No virtio-mmio device, no FDT mmap. Extra PEs stay
  parked. No `/probe` ELF on this arch.
- `make qemu-aarch64` / `make qemu-aarch64-ci` greps
  `[init] EL0 /init`, `svc debug_print ok`,
  `EL0 /init VIA SVC/ERET`, and aspace isolate.

Still stubbed: GICv3, real virtio-mmio, FDT mmap, extra-PE SMP,
`/probe`, product-class second kernel. x86 HH and RISC-V
U-mode / PLIC are untouched.

## STUB markers in the tree

Search for `// STUB:` / `STUB` :

| Item | Where | Intent |
| --- | --- | --- |
| F16/F32 dtypes | `core/src/accel.rs` | **done** (software IEEE F16/F32 on SoftNPU; not a tensor ISA; `UserAccelJob` still I32) |
| Multiboot mmap | `kernel/src/mm/mod.rs` | **done** (Multiboot1 mmap → frames; Multiboot2 parser host-tested; documented 16 MiB clip + 128 MiB cap; no FDT) |
| Higher-half + KASLR / KPTI / PCID / COW | linker / `kernel/src/mm/paging.rs` | **done** as HH subset (`ffffffff80000000+PA` + identity kept for DMA). KASLR / KPTI / PCID / COW still stub |
| Hardware SMMU | `core/src/iommu.rs` | Soft SMMU (software SID + IOVA PT) landed; program a real SMMU |
| VirtIO-Accel QEMU device | `docs/ACCEL.md` | Path B landed (in-kernel BAR + golden MMIO trace). Path A optional later |
| Cap derivation tree | `core/src/caps.rs` | **done** (small parent/child + `revoke_in`; not a seL4 CNode) |
| aarch64 EL0 / GICv3 / virtio | `kernel/src/arch/aarch64` | **done** as EL0 `/init` + `svc`/`eret` + TTBR0 isolate + in-kernel SoftNPU (timer/kthread drain). GICv3 / virtio-mmio still stub |
| RISC-V ring-3 / PLIC virtio | `kernel/src/arch/riscv64` | **done** as U-mode + PLIC software doorbell (path B AccelMmio; UART THRE → source 10). Real virtio-mmio still stub |
| Production Fiedler | `core/src/laplacian.rs` | **done** as a prototype (n≤32 host-tested median-cut + sched bind). Not GiFt-Placer; enum stays n≤8 |
| OperatorKernelHandle | `core/src/opkernel.rs` | **done** (cap + Hodge bind/refuse; not a compiler; no new syscall) |
| SparsifiedCollective | `core/src/sparsify.rs` | **done** (integer milli threshold; Hodge refuse still wins; not an eigensolve) |
| Real CXL.mem window | `MemorySpace::CxlRegion` | QEMU stub place today; no coherent load |
| Compiler ISA blob | `abi::Executable` | Kernel stores a handle; IREE/PJRT owns the bytes |
| Hardware fence/timeline | `core/src/fence.rs` | **done** (CP-shaped seq / wait / complete + credit limit; timeout is software; QEMU IRQ is still software; not a silicon timeline) |
| User-level threads (clone) | `kernel/src/{task,syscall}.rs` | **done** (`SYS_CLONE=10` shares caller aspace; not Linux clone; `flags` must be 0) |
| ramfs / virtio-blk for `/init` | `core/src/ramfs.rs`, `kernel/src/elfload.rs` | **done** as in-kernel ramfs (seed from blobs; open `/init` + `/probe`). virtio-blk still stub |

Blocking sync IPC waiter lists are no longer a stub: `SYS_RECV` and
`SYS_ACCEL_WAIT` block the caller and the kernel wakes on send / used-ring
IRQ. The fabric object itself still returns `WouldBlock`; the
kernel thread queue sleeps.

## Suggested next cuts (technical, not calendar)

1. **Custom QEMU virtio-accel** (path A) that DMA-reads the frozen BAR.
   SoftNPU can stay the executor behind the device. Optional later;
   path B (in-kernel BAR + golden MMIO trace) is the canonical demo.
   Soft-CP already covers a second AccelDevice path on the host.
2. **Hardware SMMU.** Soft SMMU already allocates per-stream IOVAs;
   program a real SMMU context / PT walk. Do not claim the software
   table is silicon.
3. **RISC-V virtio-mmio.** PLIC + SoftNPU software doorbell landed
   (path B BAR; UART THRE → source 10). A real virtio-mmio BAR
   behind the PLIC is still open.
4. **KPTI / KASLR / PCID / COW.** Higher-half linker + trampoline
   alias landed; identity 4 GiB is an intentional DMA window. Do not
   claim Meltdown unmap or a random slide.
5. **Per-task cap tables.** Kernel World still shares one `CapTable`.
   Intra-table + named-table `revoke_in` landed; a user syscall did not.
6. **aarch64 GICv3 / virtio-mmio.** EL0 `/init` + in-kernel SoftNPU
   landed; a real virtio-mmio BAR behind GICv3 is still open.
7. **`CLONE_*` / TLS / per-thread exit.** `SYS_CLONE` shares aspace
   with `flags=0`. A new aspace (`fork`) and a thread-local `exit`
   that does not kill the guest are still open.
8. **virtio-blk for `/init`.** In-kernel ramfs landed (seed from
   embedded blobs). A QEMU drive + virtio-blk driver is still open
   and must not break SoftNPU path B.

## Two-year plan

[YEAR2_PLAN.md](YEAR2_PLAN.md) holds both tracks (2026-09-06):

- **Active (Falsifier revision):** Soft SMMU SIDs, SoftCommandProcessor,
  SMP smoke, per-task PML4 + SMEP/SMAP, a minimal cap CDT / revoke,
  an aarch64 thin HAL, Multiboot mmap → frames,
  OperatorKernelHandle, SparsifiedCollective, the hardware-shaped
  fence/timeline, SoftNPU F16/F32 software IEEE, RISC-V S-mode
  userspace, AffinityLaplacian n≤32 placement, and the SpecForge
  virtio path-B ADR + golden MMIO trace, the x86 higher-half
  kernel map, user-level threads via `SYS_CLONE`, and in-kernel
  ramfs for `/init`, RISC-V PLIC + SoftNPU software doorbell, and
  aarch64 EL0 `/init` (this cut) are landed. ABI stays stable
  (0–10 unchanged). Custom QEMU virtio-accel (path A), virtio-blk,
  KPTI / KASLR remain deferred.
- **Aspirational (SpecForge appendix):** original Y1H1–Y2H2 acceptance.
  Bank QoS beyond admit/refuse, partner-stub enrichment, CXL objects,
  and a Y2 bring-up climax stay killed as milestones. Cap CDT was
  killed *as a calendar item*; the small revoke slice is unscheduled
  Y2H1 security work, not a SpecForge clock.

Soft SMMU (PR #7), SoftCommandProcessor (PR #8), SMP smoke (PR #9),
per-task PML4 / SMEP / SMAP (PR #10), cap CDT / revoke (PR #12), the
  aarch64 thin HAL (PR #13), Multiboot mmap (PR #14),
  OperatorKernelHandle (PR #15), SparsifiedCollective (PR #16),
  the hardware-shaped fence/timeline (PR #17), SoftNPU F16/F32
  software IEEE (PR #18), RISC-V S-mode userspace (PR #19),
  AffinityLaplacian n≤32 placement (PR #20), SpecForge virtio
  path B (PR #21), the x86 higher-half kernel map (PR #22),
  user-level threads / `SYS_CLONE` (PR #23), in-kernel ramfs
  for `/init` (PR #24), RISC-V PLIC + SoftNPU doorbell (PR #25),
  and aarch64 EL0 `/init` (this cut) are **done** as
  research-prototype slices.
  Custom QEMU virtio-accel (path A), virtio-blk, KPTI / KASLR,
  and the other stubs above are still open.

The public site (`site/`) is a research leave-behind, not a vendor
pitch. Its HAL-path and roadmap copy should match this active track
and [DILIGENCE.md](DILIGENCE.md) non-claims — no partnership, no
booked silicon bring-up, no manufacturing climax.

## What we will not claim

- Benchmarks vs Linux / seL4 / CUDA / any NPU SDK
- seL4-level formal proofs (the cap table is inspired, not verified)
- A CUDA-style unified virtual address space
- Wafer-scale marketing; tile SRAM is the honest first place
- Cache coherence across chiplets (UCIe/EMIB are transport)
- Readiness for tape-out or safety certification
- Partnerships with silicon vendors (`PartnerNpuStub` is a sketch)
- In-kernel ML graph IR / fusion (compilers schedule FLOPs)
- That the RISC-V or aarch64 port is a product-class second architecture

If you are a silicon OS team: start at `aether_hal::AccelDevice`,
`AccelJobDesc`, and `SoftCommandProcessor` (`CpCmd` in [ACCEL.md](ACCEL.md)),
then tell us which opcode/dtype/route fields your command processor
already has. `PartnerNpuStub` is a leftover no-op sketch, not a starting
point. The rest of Aether is meant to stay out of your way.
[DILIGENCE.md](DILIGENCE.md) is the leave-behind;
[DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) is the meeting.
