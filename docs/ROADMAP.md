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
| ramfs / virtio-blk for `/init` | **done** (in-kernel ramfs; seed from virtio-blk or embedded blobs) |
| Per-task PML4 / SMEP / SMAP | **done** (x86 subset: CR3 switch + USER-local 2 MiB windows) |
| User-level threads (clone) | **done** (additive `SYS_CLONE=10`; share caller's PML4/satp; not Linux clone) |
| Growable user `mmap` | **done** (additive `SYS_MMAP=11`; anonymous 4 KiB USER pages; not POSIX) |

## Month 3–4

| Item | Status |
| --- | --- |
| Virtqueue-shaped MMIO (doorbell + used-ring IRQ) | **done** (in-kernel BAR; SoftNPU backend; path B canonical for stock QEMU) |
| Custom QEMU `virtio-accel` device | **done** as optional path A (`qemu/aether_accel.c`; `make accel-test` / `make qemu-accel`). Stock `make qemu` stays path B. CI does not rebuild QEMU. |
| `IommuMap` pin/translate; refuse without Memory+MAP | **done** (Soft SMMU; non-identity IOVA) |
| Soft SMMU / software stream IDs | **done** (STE→CD→S1/S2 walk + ATS invalidate; not hardware) |
| Hardware SMMU / stream IDs | not started (no SID programmed on a real SMMU; partner silicon) |
| Arena tenant/bank color; Compute refuse + Exchange/transfer | **done** |
| Partner `AccelDevice` sketch (`PartnerNpuStub`) | **done** (no-op; not a partnership; not a CP path) |
| SoftCommandProcessor (`backend = 3`) | **done** (packed `CpCmd` + SET_SID-at-submit + XQueue + Soft SMMU SID + IRQ/fence; host tests) |
| Partner-shaped IREE HAL CP (`IreeShapedCp`, `backend = 4`) | **done** (frozen `IreeHalCmd` from public IREE HAL nouns; Soft SMMU `ssid=2` + SET_SID-at-submit; not a signed vendor) |
| Soft-CP SID-at-submit (Host1x-shaped) | **done** (job-head SET_SID; Soft SMMU submit latch + SID budget; two-SID host tests + `[sid]` serial). Not a Tegra driver. |
| PJRT/IREE-shaped host crate | **done** (`host/aether-pjrt`; SoftNPU / IreeShapedCp; not a PJRT plugin) |
| Soft-CP XQueue (software) | **done** (two queues; queue-boundary suspend/resume; SET_SID inherits / sticks on the queue; not a silicon queuing unit; not XSched LD_PRELOAD) |
| SoftChipletSync scoped timelines | **done** (wave / CU / chiplet / package + optional CCT; Fleet / CPElide inspiration; fence-count host tests; not Vulkan, not UCIe, not ChipletFleet placement) |

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
- `IreeShapedCp` (`backend = 4`) is the **partner-shaped HAL spine**:
  an IREE HAL dispatch packet (public Device/Buffer/Executable/Event
  nouns), not Soft-CP 2.0 and not a signed vendor. SoftNPU path B and
  Soft-CP stay.

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
  BDF. Walk is **STE → CD (SSID ≤ S1CDMax) → Stage-1 → Stage-2** on
  software block tables, not a guest 4K PTE walk and not silicon.
- Default `map` installs Nested with **identity Stage-2** (`IPA == PA`)
  so SoftNPU `resolve(iova)` is still the guest PA. `bind_nested`
  allocates a distinct IPA (`SOFT_SMMU_IPA_BASE`) so host tests can
  watch both stages; `unbind_stage2` then yields `Stage2Fault`.
- **SID lifecycle** (OpenVMM / smmuv3-accel shaped): Unbound → Captured
  on first sighting → Bound on Memory+MAP `bind_stream` / first `map`.
  Translate **aborts** (`StreamAbort`) until Bound. Illegal SSID /
  missing CD is also `StreamAbort`. `unbind_cd` drops one SSID;
  `unbind_stream` / `flr` is the STE-wide FLR analogue.
- **ATS-shaped invalidate** (`InvCmd::{Ats,Tlbi,CfgSte,CfgCd,All}`):
  software ATC only. Tables stay. SoftNPU does not depend on the ATC.
- Non-identity IOVA allocator: each (STE, CD) gets a window above
  4 GiB (`SOFT_SMMU_IOVA_BASE`). `iova != guest_pa` for the QEMU demo.
- `translate` / `walk` / `resolve` / `unmap` are stream-aware
  (`WrongStream`, `NotMapped`, `CrossTenant`, `StreamAbort`,
  `Stage2Fault`). Memory+MAP is still required to pin or bind.
- SoftNPU / virtqueue DMA writes IOVAs into the avail ring and resolves
  them back to guest PA before `KernelDma` / `SliceMem` loads. No
  IdentityDma shortcut when Soft SMMU is populated.
- Host tests cover stream A vs B, chiplet SIDs, abort-until-bound,
  SSID/CD hardening, nested S1/S2, ATS invalidate / unbind_cd / FLR,
  translate hit/miss, unmap, cap refuse, and non-identity IOVA.

Hardware SMMU (program a real SID / PT walk on an IOMMU) is still a
stub and still requires partner silicon. QEMU does not emulate an SMMU
for this path. Bank QoS beyond existing admit/refuse is out of scope.

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

Still stubbed: KPTI, PCID, COW, growable `mmap`,
per-task cap tables, APs in ring-3. SoftNPU still touches `/init`
tensors through the **intentional** kernel identity window (DMA).
KASLR is the later documented subset (boot-time slide + dual-map +
PIE reloc / unused-alias unmap).

## Year-2 H1: cap CDT / revoke (this cut)

Landed as a **small** derivation tree — inspired by seL4, **not** a
CNode/MDB and **not** a proof claim:

- `Capability` stores `parent`; the node is `(tenant, generation)`.
- `derive` and GRANT-copy set the parent edge; GRANT-move relocates a
  slot and does not walk descendants.
- `revoke(parent)` empties the lineage in that table.
  `revoke_in(parent, others)` empties grant-children in named tables.
- Host tests: mint child → revoke parent → child unusable; unrelated
  caps live. Property cases in `core/src/caps_props.rs`. Boot demo +
  QEMU `[cdt] revoke descendants ok`. Tests, not a proof.
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

Still stubbed: hotplug, FDT, managing RAM past the identity
4 GiB (HH is only a 2 GiB alias of low PA). KASLR + PIE reloc is a
later documented subset (does not grow the physical window).

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
timeline register. SoftChipletSync (`core/src/chipsync.rs`) layers
scoped (wave / CU / chiplet / package) timelines and optional CCT
elision on this model — still software, still not a Vulkan timeline
product, still not UCIe.

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

Landed as the Y1H1 virtio **path B** decision — **not** an upstream
virtio device. Path A later landed as optional:

- [ACCEL.md](ACCEL.md) ADR: path A (custom QEMU virtio-accel) vs path B
  (in-kernel BAR is the canonical demo). **B remains what `make qemu`
  runs** (stock QEMU). Path A is `qemu/aether_accel.c` +
  `make qemu-accel`.
- BAR layout is **frozen** (`magic`, `version`, `status`, `qsize`,
  `doorbell`, `used_idx`). SoftNPU behind `AccelMmio` remains the
  stock demo. Changing an offset is a dual SoftNPU + path-A + golden
  update.
- Host golden MMIO trace records cfg / doorbell / used-ring accesses
  for one SoftNPU submit/complete
  (`drivers/src/{mmio,softnpu,virtio_accel}.rs`). Path A’s C test
  checks the same published cfg values.
- Path A is a QEMU patch/plugin-shaped softmmu device, not a CI QEMU
  rebuild. No vendor claim.

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

Chiplet-local steal (`ChipletTaskScope`) is a **thin exploration stub**
on the same mesh tests — **KILL** as a calendar milestone, not a Year-1
pillar, not a partner ask. See [YEAR2_PLAN.md](YEAR2_PLAN.md).

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
- The HH cut kept identity 4 GiB for SoftNPU `IdentityDma`, PT walks,
  AP SIPI @ `0x8000`, Multiboot @ `0x7000`, and user ELF windows.
  The later **identity teardown** cut unmaps the bulk of that window
  (SoftNPU now uses Soft SMMU + HH). Do not treat leftover *islands*
  as a bug.
- Per-task PML4 clones copy `PML4[511]`, so syscall/IRQ handlers
  remain reachable after CR3 switch. USER bits stay off on HH.
- Host test: `core/src/aspace.rs` walks the HH alias. QEMU:
  `[mm] higher-half ok` + RIP in the `-2 GiB` map.
  `make qemu-ci` greps that line.
- RISC-V / aarch64 keep their identity maps. No new syscall.
  `AccelDevice` unchanged.

Still stubbed at the HH cut: KASLR (later subset), KPTI (separate
user CR3 without kernel HH), PCID, COW. Identity teardown is a
later subset.

## KASLR boot-time slide (this cut)

Landed as a **documented subset**, not PIE / relocatable KASLR, not
KPTI, not PCID, not COW, not Meltdown unmap:

- The trampoline picks a slide from `{0, 16 MiB, 32 MiB}` — all
  inside the last 2 GiB so `code-model=kernel` 32-bit signed
  addresses still resolve. Multiboot cmdline `kaslr=0|1|2|off`
  selects the index (`off` = 0). No cmdline → RDRAND if CPUID.1:ECX[30],
  else TSC (stock `qemu64` has no RDRAND; `#UD` is avoided).
  `make qemu-ci` / `qemu-smp-ci` pass `-append kaslr=1` so the
  16 MiB slot is deterministic.
- HH PDs are **cloned** (`0x71000` / `0x72000`). Identity PDs at
  `0x3000…` never move. SoftNPU later moved off identity DMA
  (Soft SMMU + HH). AP SIPI @ `0x8000` and Multiboot mailbox @
  `0x7000` stay on the low-2 MiB identity island.
- An 8 MiB kernel span is dual-mapped at `KERNEL_VMA + slide + PA`.
  The trampoline jumps to `0xffffffff80400000 + slide`. RIP is the
  slid VA. The KASLR cut left the canonical alias mapped so
  `code-model=kernel` absolute symbols still resolved. The later
  PIE cut (below) applies `.rela.dyn` and unmaps that unused alias.
- Host tests: `core/src/aspace.rs` (`parse_kaslr_cmdline`, dual-map
  walk, identity of the overwritten HH slot unchanged). QEMU:
  `[mm] kaslr slide=0x1000000` + `[mm] higher-half ok` with RIP in
  the slid window. `make qemu-ci` greps the slide line.
- RISC-V / aarch64 keep their identity maps. No new syscall.
  `AccelDevice` unchanged.

Sequenced follow-ups (do not claim them here):

1. **PIE + reloc table.** `relocation-model=pic`, keep `.rela.dyn`,
   apply `R_*_RELATIVE` in the trampoline, then unmap the unused
   canonical alias. **Landed** as a later subset.
2. **KPTI.** Separate user CR3 without kernel HH (Meltdown unmap).
   Identity DMA / SIPI must keep a supervisor-only low map on the
   kernel CR3. **Landed** as a later subset.
3. **PCID** so KPTI CR3 switches are not a full TLB shootdown.
   **Landed** as a later subset (CPUID-gated; fallback is full flush).
4. **COW** / growable `mmap` — unrelated to this slide.
   **Landed** as a later one-page subset.

Still stubbed at the KASLR cut: PIE / unmap of the unused alias
(later subset), KPTI (later subset), PCID, COW. Identity teardown
is a later subset.

## KPTI user CR3 (this cut)

Landed as a **documented subset**, not Meltdown-complete KAISER, not
PCID, not PIE / reloc, not COW:

- User CR3 maps the task's 2 MiB ELF window (USER) plus four
  supervisor 4 KiB pages at `0x73000` (syscall/IRQ trampoline, shadow
  IDT, entry stack). `PML4[511]` is empty — no kernel higher-half.
  The identity 4 GiB is **not** in the user map.
- Kernel CR3 keeps HH + the KASLR dual-map. The KPTI cut still had
  identity 4 GiB for SoftNPU `IdentityDma`; the later teardown cut
  left only SIPI / mailbox / trampoline / virtio-blk / APIC islands.
  SoftNPU kthread-B always runs on kernel CR3 (`KernelDma` + Soft SMMU).
- Syscall / IRQ from ring-3 land in the identity trampoline, switch
  CR3 to the kernel map, then jump to the higher-half handler.
  Return copies the `iretq` frame onto the trampoline stack and
  switches back. PCID (next cut) tags those `mov cr3`s when the CPU
  advertises it; without PCID each switch is still a full flush.
- Host tests: `core/src/aspace.rs` (`kpti_user_has_no_hh_or_identity_dma`).
  QEMU: `[mm] kpti ok` plus the existing SoftNPU / `/init` / clone /
  KASLR / HH greps. `make qemu-ci` greps the kpti line.
- RISC-V / aarch64 keep their identity user maps. No new syscall.
  `AccelDevice` unchanged.

Honest limits (do not market these as done):

- The four trampoline pages are still mapped in user CR3
  (supervisor-only). That is not a complete Meltdown unmap.
- Unused KASLR canonical alias is unmapped by the later PIE cut.
- No speculation barriers, no NX on trampoline data.

Sequenced follow-ups: PIE + reloc (unmap unused alias; **landed**),
PCID (next), COW / growable `mmap`.

Still stubbed at the KPTI cut: PIE / unmap of the unused alias
(later subset), PCID (later subset), COW. Identity teardown is a
later subset.

## PCID tagged TLB (this cut)

Landed as a **documented subset**, not Meltdown-complete, not a
Linux-style PCID allocator, not a speculation barrier:

- CPUID.1:ECX[17] (`PCID`) gates `CR4.PCIDE`. Kernel aspace is PCID
  1; each distinct user PML4 (`/init`, `/probe`) gets the next id
  from 2. `SYS_CLONE` shares the caller's PML4 and therefore the
  same PCID.
- `mov cr3` carries the PCID and the no-flush bit (CR3[63]) so a
  KPTI kernel↔user switch does not shoot down the other context.
  INVPCID type 1 (CPUID.7:EBX[10]) flushes one PCID on remap
  (`allow_user_2m`) / aspace teardown. No INVPCID → `mov cr3` with
  bit 63 clear for that PCID, then restore.
- **Fallback:** stock `qemu64` has no PCID. TCG QEMU (GitHub Actions
  and `make qemu`) **cannot advertise** `+pcid,+invpcid` — it warns
  `TCG doesn't support requested feature` and the guest full-flushes.
  `make qemu-pcid-ci` requests the flags and accepts either
  `[mm] pcid ok` (KVM / a TCG that implements PCID) or that warning
  plus `[mm] pcid fallback`. `make qemu-nopcid-ci` forces `-pcid`.
  `make qemu-ci` greps `[mm] pcid` on whatever `qemu64` advertises.
  Host tests lock the CR3 bit packing independently of QEMU.
- Host tests: `core/src/aspace.rs` (`cr3_tagged`, `PcidAlloc` kernel
  vs user vs clone-share). QEMU: `[mm] pcid ok` or
  `[mm] pcid fallback`. SoftNPU kthread-B stays on kernel CR3
  (identity DMA). RISC-V / aarch64 unchanged. No new syscall.

Honest limits (do not market these as done):

- This is a TLB-tag optimization for KPTI CR3 switches. Trampoline
  pages remain mapped. No lfence / speculation barriers. Not a
  Meltdown-complete claim.
- PCIDs are a handful of boot aspaces, not a recycled 12-bit
  allocator under fork load.
- Unused KASLR alias is unmapped by the later PIE cut. Identity
  4 GiB stayed on the kernel CR3 at this cut (teardown is later).

Still stubbed at the PCID cut: PIE / unmap of the unused alias
(later subset), COW (later subset). Identity teardown is a later
subset.

## Copy-on-write page subset (this cut)

Landed as a **documented subset**, not `fork`, not POSIX `mmap`,
not a general writable-share, not RISC-V / aarch64:

- One 4 KiB USER page at `0x0280_0000` (`USER_COW_BASE`). Boot
  fills a template word (`COW_TEMPLATE_WORD`) and maps the **same
  PA** read-only into `/init` and `/probe` (separate KPTI PML4s).
  The rest of that 2 MiB slot stays unmapped. SoftNPU kthread-B
  stays on kernel CR3 (`KernelDma` + Soft SMMU; identity 4 GiB
  later torn down).
- A ring-3 write is a present + write + user `#PF`. The handler
  (after the KPTI trampoline has already loaded kernel CR3)
  allocates a private frame, copies 4 KiB, sets RW on **that**
  aspace only, INVPCID / tagged flush of the user PCID, and
  resumes. `/probe` still names the template.
- `SYS_CLONE` shares the caller's PML4, so a sibling sees the
  private page after the break — they are not a second isolation
  domain. No new syscall (0–10 frozen).
- Host tests: `core/src/aspace.rs` (`cow_shared_until_write`,
  `clone_threads_share_cow_break`). QEMU:
  `[mm] cow ok`, `[init] cow write ok (private page)`,
  `[probe] cow still template`. `make qemu-ci` greps those.
- RISC-V / aarch64 do not map the VA. KPTI trampoline, PCID,
  KASLR slide, and SoftNPU path B are unchanged.

Honest limits (do not market these as done):

- One page, one template, x86 only. Not file-backed COW, not
  `fork` of the whole aspace, not growable `mmap`.
- Unused KASLR alias is unmapped by the later PIE cut. Identity
  teardown is a later subset.

Sequenced follow-ups: PIE + reloc (unmap unused alias; **landed**),
identity teardown (**landed**), growable `mmap` / `fork`-shaped
aspace clone.

## PIE + reloc table (this cut)

Landed as a **documented subset**, not a secret slide, not
Meltdown-complete, not a user-ELF relocator:

- x86_64 kernel is `relocation-model=pic` + `code-model=small`
  (static-PIE). `code-model=kernel` + PIC is rejected by lld
  (`R_X86_64_32S` against absolute symbols). RIP-relative
  displacements cover the 8 MiB image; two absolute asm operands
  (`_start` stack `lea`, `SYSCALL_KSTACK`) were rewritten
  RIP-relative. RISC-V / aarch64 rustflags stay static.
- Linker keeps `.rela.dyn` (not discarded). `objcopy -O binary`
  leaves the table in `kernel.bin`. `scripts/pack_kernel_relocs.py`
  appends a 16-byte trailer (`magic`, count, offset, entsize=24)
  so the trampoline can walk `Elf64_Rela` without an ELF parser.
  Only `R_X86_64_RELATIVE` is accepted (CI-sized image has a few
  hundred). Formula: `*r_offset = addend + slide` via the identity
  map (`LMA = VA - KERNEL_VMA`).
- After apply, HH PD0 indices 2..5 (the 8 MiB link-time span) are
  zeroed when `slide != 0` and CR3 is reloaded. The unused
  canonical alias (`0xffffffff80400000`) is not present. Slide 0
  keeps that map — it *is* the running window. Identity 4 GiB was
  still mapped at this cut (teardown is later).
- Host tests: `core/src/reloc.rs` (addend+slide, trailer, refuse
  non-RELATIVE / OOB) and `core/src/aspace.rs`
  (`pie_unmaps_unused_canonical_alias`). QEMU:
  `[mm] pie reloc n=` + `[mm] kaslr unused alias unmapped` +
  RIP in the slid window + existing SoftNPU / KPTI / PCID / COW /
  clone greps. `make qemu-ci` / `qemu-smp-ci` grep those lines.
- KPTI trampoline, `enter_user`, SoftNPU kthread-B, and `SYS_CLONE`
  are unchanged in contract. Function pointers in `.data` are
  relocated before `_start`.

Honest limits (do not market these as done):

- Three 16 MiB slots, cmdline / TSC / RDRAND. Not a secret ASLR
  entropy claim. An attacker who can read the slide mailbox or
  RIP still knows the map.
- User `/init` is still a static non-PIE ELF (`core::elf` rejects
  `ET_DYN`). Not `fork`, not growable `mmap`.

Still stubbed at the PIE cut: a recycled PCID allocator,
Meltdown-complete trampoline unmap, POSIX MM. Identity teardown
is a later subset.

## Identity teardown (this cut)

Landed as a **documented subset**, not a complete low-memory unmap,
not Meltdown-complete, not RISC-V / aarch64 (those maps *are*
identity):

- After `[mm] higher-half ok`, `teardown_identity` clears identity
  2 MiB PD leaves except keep islands. SoftNPU / Soft-CP DMA
  resolves **only** through Soft SMMU (`resolve_stream` /
  `resolve`). Empty-iommu passthrough remains for host golden
  MMIO tests that skip pin. CPU tensor access is `KernelDma` →
  `phys_to_kva` (HH on x86). No IdentityDma shortcut for tensors
  when Soft SMMU is populated.
- **Remaining identity islands** (2 MiB leaves, supervisor-only):
  - `[0, 2 MiB)` — boot PTs, Multiboot mailbox @ `0x7000`, AP
    SIPI @ `0x8000`, KPTI trampoline @ `0x73000`, HH PDs @
    `0x71000`.
  - virtio-blk DMA window `0x02A00000–0x02C00000`.
  - APIC MMIO `0xFEE00000` (above the HH 2 GiB window).
- Torn down: user ELF `0x2000000` / `0x2400000`, SoftNPU arena
  `0x01000000`, kernel LMA `0x400000`, growable `SYS_MMAP`
  window `0x02C00000`, and the rest of RAM. Page-table walks
  and ELF / COW / mmap / user copies use `phys_va` (HH). User
  buffers are walked in the task CR3, then loaded via HH.
  `USER_MMAP_BASE` is a KPTI user-only 4 KiB grow window, not
  an identity island.
- Host tests: `identity_keep_islands_and_kva`,
  `teardown_drops_ram_keeps_islands`,
  `service_refuses_guest_pa_when_smmu_bound`. QEMU:
  `[mm] identity teardown ok (islands: low 2MiB + virtio-blk +
  APIC; SoftNPU via Soft SMMU + HH)` plus existing SoftNPU /
  KPTI / PCID / COW / PIE / blk greps. `make qemu-ci` /
  `qemu-smp-ci` / `qemu-blk-ci` grep that line.
- RISC-V / aarch64 keep identity (kernel map). `KernelDma` is
  PA=VA there. No new syscall. KPTI / PCID / COW / PIE / virtio-blk
  contracts are unchanged except SoftNPU no longer needs the
  4 GiB window.

Honest limits (do not market these as done):

- Islands remain on purpose. A forged kernel pointer into the
  low 2 MiB, virtio-blk window, or APIC leaf still hits identity.
- Not a complete Meltdown unmap (KPTI trampoline pages stay
  mapped in user CR3). Not POSIX `mmap`. Not hardware SMMU.

Still stubbed: a recycled PCID allocator, Meltdown-complete
trampoline unmap, POSIX MM, hardware SMMU.

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

Landed as a **documented subset**, not POSIX, not a user `open` /
`read` syscall:

- `RamFs` in `core/src/ramfs.rs`: flat named files over borrowed
  slices. `seed` / `open` / `read` / `bytes`. Host tests cover
  `/init` + `/probe`, chunked read, missing/duplicate/bad names.
- Boot seeds `/init` (and x86 `/probe`) from virtio-blk when a
  drive is present (below), otherwise from the embedded ELF blobs.
  The loader **opens those names** and copies `PT_LOAD` from the
  ramfs bytes — it does not call `include_bytes!` at the load site.
- QEMU: `[ramfs] open /init ok` (plus `/probe` on x86) and
  `[boot] loaded /init … (static non-PIE, ramfs)`.
  `make qemu-ci` / `qemu-smp-ci` grep `[ramfs] seed embedded`
  (no drive). `qemu-riscv-ci` greps the open line.
- No new syscall. Numbers 0–10 stay as in [ABI.md](ABI.md).
  User `open`/`read` is not this cut.
- SoftNPU path B, Soft SMMU, higher-half identity DMA, RISC-V
  U-mode, `SYS_CLONE`, and the enter_user PIT snapshot are
  unchanged.

## virtio-blk → ramfs (this cut)

Landed as a **documented x86 subset**, not a block layer, not
virtio 1.0 modern MMIO, not RISC-V / aarch64:

- QEMU `-drive file=build/bootfs.img,if=none,format=raw,id=bootfs`
  plus `-device virtio-blk-pci,drive=bootfs,disable-legacy=off`.
  Image is **AETHFS01** (`core/src/bootfs.rs` / `scripts/mkbootfs.py`):
  a flat named-file pack, not FAT/GPT.
- Legacy virtio-pci I/O (`kernel/src/virtio_blk.rs`): PCI scan for
  vendor `0x1AF4` / device `0x1001`, BAR0 I/O, poll the used ring
  (no IRQ). Blocks DMA into `BLK_WINDOW_BASE` (`0x02A0_0000`, 2 MiB
  identity). SoftNPU arenas stay at `0x0100_0000`. The AccelMmio
  BAR is a software array — this path does not touch it.
- On success: `ramfs::seed` `/init` and `/probe` from the window.
  Loader still `open`s those names. Serial:
  `[blk] virtio-blk seed /init ok` (+ `/probe`).
- **Fallback:** no device (or probe/parse fail) → seed the
  embedded blobs and print `[ramfs] seed embedded`. `make qemu-ci`
  / `qemu-smp-ci` stay drive-less. `make qemu-blk-ci` requires the
  drive and greps the `[blk]` seed lines plus SoftNPU / `/init`.
- Host tests: `core/src/bootfs.rs` (pack/parse/seed, refuse bad
  magic/OOB/dup). No new syscall (0–10 frozen).
- RISC-V / aarch64 keep embedded seed. SoftNPU path B, KPTI, PCID,
  COW, PIE, SMP, `SYS_CLONE` unchanged.

Honest limits (do not market these as done):

- Legacy I/O virtqueue only. No modern virtio-pci MMIO, no
  virtio-mmio, no MSI-X, no write path, no general FS.
- One boot-time read. Not a user block device.

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
  `EL0 /init VIA SVC/ERET`, aspace isolate, and
  `[accel] used-ring IRQ job#`.

Still stubbed: GICv3, real virtio-mmio, FDT mmap, extra-PE SMP,
`/probe`, product-class second kernel. x86 HH and RISC-V
U-mode / PLIC are untouched.

## STUB markers in the tree

Search for `// STUB:` / `STUB` :

| Item | Where | Intent |
| --- | --- | --- |
| F16/F32 dtypes | `core/src/accel.rs` | **done** (software IEEE F16/F32 on SoftNPU; not a tensor ISA; `UserAccelJob` still I32) |
| Multiboot mmap | `kernel/src/mm/mod.rs` | **done** (Multiboot1 mmap → frames; Multiboot2 parser host-tested; documented 16 MiB clip + 128 MiB cap; no FDT) |
| Higher-half + KASLR / KPTI / PCID / COW / mmap | linker / `kernel/src/mm/paging.rs` | **done** as HH + KASLR + PIE-reloc (`.rela.dyn` + unused alias unmapped) + KPTI + PCID + one-page COW subset (`USER_COW_BASE` RO until write fault) + growable anon `SYS_MMAP=11`. `fork` still stub |
| Hardware SMMU | `core/src/iommu.rs` | Soft SMMU deepened (STE→CD→S1/S2 + ATS invalidate); program a real SMMU |
| VirtIO-Accel QEMU device | `docs/ACCEL.md` | Path B landed (in-kernel BAR + golden MMIO trace). Path A optional later |
| Cap derivation tree | `core/src/caps.rs` | **done** (small parent/child + `revoke_in`; not a seL4 CNode) |
| aarch64 EL0 / GICv3 / virtio | `kernel/src/arch/aarch64` | **done** as EL0 `/init` + `svc`/`eret` + TTBR0 isolate + in-kernel SoftNPU (timer/kthread drain). GICv3 / virtio-mmio still stub |
| RISC-V ring-3 / PLIC virtio | `kernel/src/arch/riscv64` | **done** as U-mode + PLIC software doorbell (path B AccelMmio; UART THRE → source 10). Real virtio-mmio still stub |
| Production Fiedler | `core/src/laplacian.rs` | **done** as a prototype (n≤32 host-tested median-cut + sched bind). Not GiFt-Placer; enum stays n≤8 |
| ChipletFleet | `core/src/sched.rs` | **KILL as calendar.** Thin host stub (`ChipletTaskScope` affinity / steal). Not a Year-1 pillar, not a partner ask |
| OperatorKernelHandle | `core/src/opkernel.rs` | **done** (cap + Hodge bind/refuse; not a compiler; no new syscall) |
| SparsifiedCollective | `core/src/sparsify.rs` | **done** (integer milli threshold; Hodge refuse still wins; not an eigensolve) |
| Real CXL.mem window | `MemorySpace::CxlRegion`, `core/src/window.rs` | **killed as a milestone.** `TypedWindow` is an honest pin stub (SID refuse, host tests), not this item. Not a HDM decoder, not QEMU CXL. See [WINDOW.md](WINDOW.md) |
| Compiler ISA blob | `abi::Executable` + `host/aether-pjrt` | Kernel stores a handle; host shim packs `IreeHalCmd` / submits `AccelOp`; IREE/PJRT would own the bytes. Not a plugin. |
| Hardware fence/timeline | `core/src/fence.rs` | **done** (CP-shaped seq / wait / complete + credit limit; timeout is software; QEMU IRQ is still software; not a silicon timeline) |
| SoftChipletSync | `core/src/chipsync.rs` | **done** as software scoped timelines + hierarchical counters + optional CCT. Not Vulkan, not UCIe, not ChipletFleet placement. Fence counts only |
| User-level threads (clone) | `kernel/src/{task,syscall}.rs` | **done** (`SYS_CLONE=10` shares caller aspace; not Linux clone; `flags` must be 0) |
| Growable user `mmap` | `kernel/src/{syscall,mm/paging}.rs` | **done** (`SYS_MMAP=11` anonymous 4 KiB USER pages; not POSIX; no file / no `MAP_SHARED`) |
| ramfs / virtio-blk for `/init` | `core/src/{ramfs,bootfs}.rs`, `kernel/src/{elfload,virtio_blk}.rs` | **done** as in-kernel ramfs + x86 virtio-blk seed (AETHFS01; embedded fallback). Not POSIX / not a block layer |

Blocking sync IPC waiter lists are no longer a stub: `SYS_RECV` and
`SYS_ACCEL_WAIT` block the caller and the kernel wakes on send / used-ring
IRQ. The fabric object itself still returns `WouldBlock`; the
kernel thread queue sleeps.

## Six-month plan (next calendar)

[SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) is the Falsifier-revised
calendar after Year-1 + hardening (through PR #37) and the M1 opcode
device (PR #38):

- **M1 landed (PR #38):** `IreeShapedCp` (`backend = 4`) — frozen IREE
  HAL packet; research opcodes, not a vendor-as-partner claim.
- **M2 landed (PR #41):** PJRT/IREE-shaped host shim (`host/aether-pjrt`)
  packs frozen `IreeHalCmd` and submits through `IreeShapedCp`.
  SoftNPU stays the `make qemu` path-B demo.
- **M4 landed (PR #47):** Soft-CP XQueue (two software queues;
  queue-boundary suspend/resume; SID sticks to the queue). Not a
  silicon queueing unit. Not an XSched LD_PRELOAD shim.
- **Optional M2 leave-behind (this cut):** Soft SMMU bring-up kit
  (`docs/bringup/`, `scripts/smmu_{dump,replay}.py`) — software-table
  dump/replay of STE→CD→S1/S2 + ATS, tied to `AccelDevice::map` /
  Soft-CP SID bind. Not a Soft-SMMU redo. Keep it thin.
- Soft SMMU / Soft-CP / SMP / PML4 are **not** re-scheduled.
- **M3 landed:** Soft-CP SID-at-submit (Host1x-shaped SET_SID; SID
  inherits / sticks on the XQueue). Not a Host1x driver.
- **SoftChipletSync landed:** scoped timelines `{wave, CU, chiplet,
  package}` + optional CCT elision. Fleet / CPElide inspiration only.
  Not UCIe, not Vulkan, not ChipletFleet placement. Fence-count host
  tests; single-die numbers are not partner proof.
- SpecForge OS-completeness theater (fork, POSIX, CXL productization,
  ChipletFleet, formal caps, site-as-milestone, PartnerNpuStub without
  opcodes) is **not** the schedule. PR #46 was a site progress refresh,
  not a milestone. There is no OS-completeness M3–M4 clock.

## Suggested next cuts (technical, not calendar)

The Kernel **calendar** is [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md)
(M1–M4 done; SoftChipletSync landed). The list below is leftover
engineering.

1. **Optional PASID / SVA.** Process-ASID on Soft-SMMU CDs if the host
   shim needs per-client VAS. Software only. SoftChipletSync scoped
   timelines landed (chiplet-local fence domains; not UCIe).
2. **Guest driver for path A.** The QEMU `aether-accel` device and
   host model landed (`qemu/`, `make accel-test`). Stock `make qemu`
   stays path B. A kernel `VirtioAccelMmio` that talks PCI BAR0
   (GPA in the job wire; Soft SMMU stays the cap table) is still
   open. Soft-CP (`backend = 3`) and `IreeShapedCp` (`backend = 4`)
   already cover extra AccelDevice paths on the host.
   SIX_MONTH_PLAN pulls this **only if** path-A DMA must prove Soft-SMMU
   IOVA.
3. **Hardware SMMU.** Soft SMMU now walks STE→CD→Stage-1/2 and has an
   ATS-shaped invalidate in software. The bring-up kit dumps those
   tables. Program a real SMMU context / PT walk. Do not claim the
   software table is silicon. Partner silicon is still required.
4. **RISC-V virtio-mmio.** PLIC + SoftNPU software doorbell landed
   (path B BAR; UART THRE → source 10). A real virtio-mmio BAR
   behind the PLIC is still open.
5. **`fork` / POSIX `mmap`.** Growable anonymous `SYS_MMAP=11` landed
   (64 KiB window; first-fit; not file-backed). PIE-reloc KASLR +
   one-page COW + KPTI + PCID landed. Do not claim Meltdown-complete,
   a secret slide, or POSIX `mmap` / `fork`.
6. **Per-task cap tables.** Kernel World still shares one `CapTable`.
   Intra-table + named-table `revoke_in` landed; a user syscall did not.
7. **aarch64 GICv3 / virtio-mmio.** EL0 `/init` + in-kernel SoftNPU
   landed; a real virtio-mmio BAR behind GICv3 is still open.
8. **`CLONE_*` / TLS / per-thread exit.** `SYS_CLONE` shares aspace
   with `flags=0`. A new aspace (`fork`) and a thread-local `exit`
   that does not kill the guest are still open.
9. **Modern virtio-blk / virtio-mmio.** Legacy PCI I/O + AETHFS01
   seed landed on x86 (`make qemu-blk-ci`). A virtio 1.0 MMIO BAR,
   RISC-V / aarch64 virtio-mmio, and a user block device are still
   open. SoftNPU path B stays the in-kernel BAR.
10. **A real PJRT plugin / IREE HAL driver.** `host/aether-pjrt` is
   the host contract (Device / MemorySpace / Buffer / Executable /
   Event → frozen `IreeHalCmd` on IreeShapedCp). `GetPjRtApi` and
   `iree_hal_driver_t` are still out of tree. Not a vendor integration.

## Two-year plan

[YEAR2_PLAN.md](YEAR2_PLAN.md) holds both tracks (2026-09-06). The
Falsifier ACTIVE track through PR #37 is **complete as research
slices**; do not sequence new work against it. Next calendar:
[SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) (M1–M4 done; SoftChipletSync
landed).

- **Landed (Falsifier revision):** Soft SMMU SIDs, SoftCommandProcessor,
  IreeShapedCp (IREE HAL packet, `backend = 4`; not a signed vendor),
  SMP smoke, per-task PML4 + SMEP/SMAP, a minimal cap CDT / revoke,
  an aarch64 thin HAL, Multiboot mmap → frames,
  OperatorKernelHandle, SparsifiedCollective, the hardware-shaped
  fence/timeline, SoftNPU F16/F32 software IEEE, RISC-V S-mode
  userspace, AffinityLaplacian n≤32 placement, and the SpecForge
  virtio path-B ADR + golden MMIO trace, the x86 higher-half
  kernel map, the KASLR boot-time slide, the PIE-reloc / unused-alias
  unmap, the KPTI user-CR3 subset, the PCID tagged-TLB subset, the
  one-page COW subset, user-level threads via `SYS_CLONE`, and
  in-kernel ramfs for `/init`, x86 virtio-blk → ramfs seed,
  RISC-V PLIC + SoftNPU software
  doorbell, and aarch64 EL0 `/init` are landed. ABI stays stable
  (0–10 unchanged; `SYS_MMAP=11` additive). Path A (optional QEMU
  `aether-accel` device) landed as a host-tested model + optional
  softmmu build. `fork` remains deferred.
- **Aspirational (SpecForge appendix):** original Y1H1–Y2H2 acceptance.
  Bank QoS beyond admit/refuse, partner-stub enrichment, CXL objects,
  and a Y2 bring-up climax stay killed as milestones. Exploration E
  (`TypedWindow`) is an unscheduled honest stub, not that CXL check-off.
  Cap CDT was
  killed *as a calendar item*; the small revoke slice is unscheduled
  Y2H1 security work, not a SpecForge clock.

Soft SMMU (PR #7), SoftCommandProcessor (PR #8), IreeShapedCp
  (PR #38; IREE HAL packet, not a vendor), SMP smoke (PR #9),
per-task PML4 / SMEP / SMAP (PR #10), cap CDT / revoke (PR #12), the
  aarch64 thin HAL (PR #13), Multiboot mmap (PR #14),
  OperatorKernelHandle (PR #15), SparsifiedCollective (PR #16),
  the hardware-shaped fence/timeline (PR #17), SoftNPU F16/F32
  software IEEE (PR #18), RISC-V S-mode userspace (PR #19),
  AffinityLaplacian n≤32 placement (PR #20), SpecForge virtio
  path B (PR #21), the x86 higher-half kernel map (PR #22),
  user-level threads / `SYS_CLONE` (PR #23), in-kernel ramfs
  for `/init` (PR #24), RISC-V PLIC + SoftNPU doorbell (PR #25),
  aarch64 EL0 `/init` (PR #26), the x86 KASLR boot-time
  slide (PR #27), the x86 KPTI user-CR3 subset (PR #28), the
  x86 PCID tagged-TLB subset (PR #29), the one-page COW
  subset (PR #30), the x86 PIE-reloc / unused-alias unmap
  (PR #31), and x86 virtio-blk → ramfs
  (this cut)
  are **done** as research-prototype slices.
  Optional path-A QEMU `aether-accel` (this cut) landed as a
  host-tested device model; stock QEMU stays path B. `fork` and
  the other stubs above are still open. Growable anonymous
  `SYS_MMAP` landed.

The public site (`site/`) is a research leave-behind, not a vendor
pitch. Lead with the working QEMU slice (Year-1 + hardening landed),
not a v0.1 prototype disclaimer. HAL-path and roadmap copy should
match the landed Year-1 track, [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md),
and [DILIGENCE.md](DILIGENCE.md) non-claims
— no partnership, no booked silicon bring-up, no manufacturing climax.

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
- That `TypedWindow` / `CxlMemStub` is CXL.mem silicon (Exploration E is a stub)

If you are a silicon OS team: start at `aether_hal::AccelDevice`,
`AccelJobDesc`, and either `SoftCommandProcessor` (`CpCmd`) or
`IreeShapedCp` (`IreeHalCmd` — IREE HAL nouns) in [ACCEL.md](ACCEL.md),
then tell us which opcode/dtype/route fields your command processor
already has. `PartnerNpuStub` is a leftover no-op sketch, not a starting
point. The rest of Aether is meant to stay out of your way.
[DILIGENCE.md](DILIGENCE.md) is the leave-behind;
[DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) is the meeting.
