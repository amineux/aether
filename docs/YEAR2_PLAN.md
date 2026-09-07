# Year 2 plan

Leave-behind for Kernel tracking. SpecForge draft 2026-09-06; Falsifier
revision 2026-09-06. Filed on main via PR.

**Status (2026-09-07):** the Falsifier ACTIVE track through PR #37 is
**complete as research slices.** `IreeShapedCp` (PR #38) landed the
six-month M1 opcode device. `host/aether-pjrt` (PR #41) landed M2.
The closed M1–M4 calendar is [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md)
(M1–M4 done; SoftChipletSync landed). The **next calendar** is
[MONTH5_PLAN.md](MONTH5_PLAN.md). Do
not re-schedule Soft SMMU / Soft-CP / SMP / PML4 / `IreeShapedCp`
from the KEEP list below. SpecForge appendix remains aspirational
only.

## Non-negotiables

- Research prototype. SoftNPU + stock QEMU (in-kernel BAR) is the
  canonical demo (SpecForge path B). A custom QEMU `-device` (path A)
  is optional (`qemu/`, `make qemu-accel`); stock `make qemu` stays B.
- No fake NVIDIA partnership, no FLOP benchmarks, no tape-out / readiness
  claims.
- [`docs/ABI.md`](ABI.md) syscall 0–8 stay frozen; additive only
  (`exit=9`, `clone=10`; later `mmap=11`). Live freeze is **0–11** in
  [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md). In-kernel ramfs adds no user
  syscall.
- Prefer extending `aether_hal::AccelDevice` + `AccelJobDesc` over
  inventing a second IR.

## Falsifier revision — execution track

**This was the ACTIVE track through PR #37.** SpecForge's half-year calendar
([appendix](#specforge-criteria-aspirational-appendix)) is aspirational
only — do not schedule Kernel work against it. Closed M1–M4 sequencing lives in
[SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md); Month 5 lives in
[MONTH5_PLAN.md](MONTH5_PLAN.md). Soft SMMU (PR #7;
deepened as STE→CD→S1/S2 + ATS invalidate, still not hardware) and
the SoftCommandProcessor AccelDevice (packed `CpCmd` + IRQ/fence) are
**done** as software models. `IreeShapedCp` (`backend = 4`) is **done**
as the partner-shaped HAL spine: a frozen IREE HAL dispatch packet
(public Device / Buffer / Executable / Event nouns from
`iree-org/iree` headers), Soft SMMU `ssid = 2`, IRQ/fence. It is **not**
Soft-CP 2.0, **not** `PartnerNpuStub` enrichment, **not** a signed
vendor. SoftNPU path B and Soft-CP stay. SMP smoke (INIT-SIPI + per-CPU `gs` +
two-hart work-steal) is **done** as a QEMU `-smp 2` slice. SpecForge
virtio path B (in-kernel BAR canonical + golden MMIO trace) is
**done**; path A landed as an optional QEMU device model
(`qemu/aether_accel.c`; CI runs `make accel-test`, not a QEMU
rebuild). Other stubs remain open (see [ROADMAP.md](ROADMAP.md)). Per-task PML4 + SMEP/SMAP is **done** as
an x86 documented subset (CR3 switch, task-local USER 2 MiB windows).
The higher-half kernel map (`ffffffff80000000+PA`) is **done** as
a documented subset (identity torn down except SIPI / mailbox /
trampoline / virtio-blk / APIC islands; SoftNPU via Soft SMMU + HH). A
boot-time KASLR slide (0 / 16 / 32 MiB dual-map; cmdline / entropy)
plus PIE `.rela.dyn` apply and unused-alias unmap is **done** as a
documented subset. A KPTI
user-CR3 subset (no HH / no identity DMA in user CR3; 4 KiB
trampoline; kernel CR3 keeps identity islands + HH; not Meltdown-complete) is
**done** as a documented subset. A PCID tagged-TLB subset
(CR4.PCIDE when CPUID.PCID; kernel PCID 1 / per-aspace user PCIDs;
INVPCID on remap; full-flush fallback) is **done** as a
documented subset — not a Meltdown-complete claim. A one-page
COW subset is **done** as the next documented x86 cut. A minimal cap CDT / revoke is **done** as
unscheduled Y2H1 security work (parent/child edges + `revoke_in`;
not a seL4 CNode, not a calendar milestone). An aarch64 thin HAL
(`make qemu-aarch64`) is **done** as a RISC-V-shaped bring-up (PL011
+ GICv2 + TTBR). The later EL0 userspace cut (`eret`/`svc` `/init` +
TTBR0 isolate + in-kernel SoftNPU) is **done** as a documented
subset — not product-class. Multiboot mmap → frames
is **done** as a documented x86 subset (clip 16 MiB, 128 MiB bitmap
cap; explicit fallback on RISC-V / aarch64; not a general MM).
OperatorKernelHandle is **done** as an unscheduled research kernel
surface (`CapKind::OperatorKernel` + Hodge bind/refuse; not a
compiler, not a new syscall, not a collective engine).
SparsifiedCollective is **done** as the sibling slice (integer milli
threshold; drop below-threshold harmonic before inject; Hodge refuse
still wins; not an eigensolve). The hardware-shaped fence/timeline
is **done** as a software model (`TimelineId` + seq / wait /
complete; timeout is software; QEMU IRQ is still software; not a
silicon fence). SoftNPU F16/F32 is **done** as software IEEE
(`DType` 1/2; FTZ; not a tensor ISA; `UserAccelJob` still I32).
RISC-V S-mode userspace is **done** as a documented subset
(`sret`/`ecall` `/init` + Sv39 U-isolate + in-kernel SoftNPU).
RISC-V PLIC + SoftNPU software doorbell (UART THRE → source 10;
path B BAR; not virtio-mmio) is **done** as the follow-up interrupt
path. Neither is product-class. AffinityLaplacian n≤32 placement is
**done** as a prototype eigensolve (`from_placement` +
`bind_laplacian_cut`; enum stays n≤8; not GiFt-Placer). SpecForge
virtio path B is **done** (ADR in [ACCEL.md](ACCEL.md); golden MMIO
trace on SoftNPU submit/complete; BAR frozen). Path A is **done**
as optional (`qemu/`; stock QEMU stays B). The
x86 higher-half kernel map is **done** as a documented subset
(`ffffffff80000000+PA`; identity torn down except documented islands). The KASLR
boot-time slide is **done** as a documented subset (16 MiB slots,
dual-map). PIE-reloc + unused-alias unmap is **done**. The KPTI user-CR3 subset is
**done** (trampoline entry; kernel CR3 keeps identity islands + HH; not
Meltdown-complete). Identity teardown is **done** (SoftNPU via Soft
SMMU + HH). The PCID tagged-TLB subset is **done**
(CPUID-gated `mov cr3`; fallback is a full flush). A one-page
COW subset (`USER_COW_BASE` RO in `/init` + `/probe` until a
write fault; not `fork` / POSIX `mmap`) is **done** as the next
documented x86 subset. User-level threads via `SYS_CLONE` (nr 10) are
**done** as a documented subset (share caller PML4/satp; `flags=0`;
not Linux clone / fork; `SYS_EXIT` still guest-wide). Growable
anonymous `SYS_MMAP` (nr 11) is **done** as a documented subset
(4 KiB USER pages in a reserved window; not POSIX `mmap`).
In-kernel ramfs for `/init` (and x86 `/probe`) is **done** as a
documented subset (seed from virtio-blk or embedded blobs; loader
`open`/`read`; not POSIX; no new syscall). x86 virtio-blk → ramfs
(legacy PCI I/O + AETHFS01; `make qemu-blk-ci`) is **done** as the
follow-up; `make qemu-ci` stays on the embedded fallback.
`IreeShapedCp` (`backend = 4`) is **done** as the partner-shaped HAL
spine (frozen `IreeHalCmd` from public IREE HAL nouns; Soft SMMU
`ssid = 2`; host tests + boot probe). Not a signed vendor. SoftNPU
path B and Soft-CP (`backend = 3`) are unchanged. The host crate
`aether-pjrt` (`host/aether-pjrt`) is **done** as the PJRT/IREE-shaped
compiler contract: abi nouns → frozen `IreeHalCmd` → `IreeShapedCp`
(not a PJRT plugin, not an IREE HAL driver, not a vendor runtime).
SoftNPU stays the `make qemu` path-B demo.

### KEEP / ACTIVE Y1

1. Soft SMMU: non-identity IOVA + `stream_id` on the `AccelDevice` map
   path (not a SoftMMU product claim). Map without Memory+MAP still
   refuses; wrong-stream DMA fault/reject is a software model. The
   table now walks STE→CD→Stage-1/2 and has an ATS-shaped invalidate;
   hardware SMMU still requires partner silicon.
2. One real-shaped `AccelDevice` path beyond SoftNPU that packs a
   **concrete command packet** + fence/IRQ complete (software model OK).
   Distinct backend id; probe/submit/poll/map contract tests. This is
   **not** `PartnerNpuStub` enrichment theater.
   **Landed** as Soft-CP (`backend = 3`, Aether-native `CpCmd`) and as
   the partner-shaped spine `IreeShapedCp` (`backend = 4`, IREE HAL
   `IreeHalCmd`). The latter is a public HAL noun mapping, not a signed
   vendor.
3. Keep `AccelDevice` / `AccelJobDesc` ABI stable. New surface = new
   numbers or caps bits only after [ABI.md](ABI.md) amendment in the
   same PR. Do not reshape `UserAccelJob` wire without a version bump
   in [ACCEL.md](ACCEL.md).

### KILL as milestones (calendar theater)

These stay in the SpecForge appendix as written. They are **not**
Kernel calendar items:

- Bank QoS beyond admit/refuse (`PartitionProfile` already admits or
  refuses; EventRing credit-drain theater is not a Y1 goal).
- Second partner stub enrichment without a signed partner
  (`PartnerNpuStub` stays a labeled sketch).
- CXL region objects / CXL.mem window — **stays killed as a milestone.**
  Keep the typed `MemorySpace::CxlRegion` place. `TypedWindow`
  ([WINDOW.md](WINDOW.md)) is an unscheduled honest stub (inspiration
  nouns + Soft SMMU SID refuse). It is **not** pin/map productization
  and does **not** check off SpecForge Y2H1.
- Cap CDT as a *calendar* item. A small derivation-edge revoke landed
  as unscheduled Y2H1 security work; do not treat it as a SpecForge
  half-year clock, a seL4 clone, or a reason to pull CXL / Laplacian.
- Y2 manufacturing / bring-up playbook as a climax goal
  ([DILIGENCE.md](DILIGENCE.md) already exists).
- ChipletFleet as a *calendar* item. A thin host-tested chiplet-affinity
  stub (`ChipletTaskScope` on `two_chiplet_mesh`) is not a Year-1
  pillar, not a partner ask, and not a Fleet claim.

### DEFER

After AccelDevice bites a real-shaped path — not before:

- Guest kernel driver for path A (PCI BAR0 / `VirtioAccelMmio`).
  The QEMU device model landed (`qemu/aether_accel.c`; `make
  accel-test` / `make qemu-accel`). Path B remains the canonical
  stock-QEMU demo + golden MMIO trace. RISC-V PLIC + software
  doorbell (UART THRE → SoftNPU AccelMmio) landed; a virtio-mmio
  BAR behind the PLIC is still open.
- ELF beyond this subset (`fork` / POSIX `mmap`). Growable anonymous
  `SYS_MMAP=11` landed. Boot-time
  slide + dual-map + PIE `.rela.dyn` landed (16 MiB slots; unused
  alias unmapped). KPTI user CR3 landed (no HH / no identity DMA;
  trampoline only; not Meltdown-complete). PCID tagged TLB landed
  (CPUID-gated; stock `qemu64` often full-flushes). One-page COW
  landed (`USER_COW_BASE`; write fault copies; x86 only). In-kernel
  ramfs for `/init` + `/probe` landed (seed from virtio-blk or
  embedded blobs; no user `open`/`read`). x86 virtio-blk (legacy
  PCI I/O + AETHFS01) landed; modern virtio-mmio is still open.
  Per-task PML4 +
  SMEP/SMAP + optional `/probe` + higher-half linker/trampoline is
  landed. Kernel identity 4 GiB is torn down except SIPI / mailbox /
  trampoline / virtio-blk / APIC islands; SoftNPU is Soft SMMU + HH.
  Growable `SYS_MMAP` at `0x02C0_0000` is user-only, not an identity
  island.
- aarch64 GICv3 / virtio-mmio (EL0 `/init` landed; virtqueue BAR is
  in-kernel, not a `-device`).

### PR order for Kernel (revised)

1. `docs/YEAR2_PLAN.md` with both SpecForge criteria + this Falsifier
   active track
2. Soft SMMU SIDs on the AccelDevice path
3. Second AccelDevice software CP with a real command packet (not
   `PartnerNpuStub` enrichment theater)
4. SMP smoke (INIT-SIPI, per-CPU `gs`, two-hart work-steal)
5. Per-task PML4 + SMEP/SMAP (documented x86 subset)
6. Minimal cap CDT / revoke (internal API + host/QEMU demo)
7. aarch64 thin HAL (QEMU virt, no EL0)
8. Multiboot mmap → frames (documented subset)
9. OperatorKernelHandle (Hodge-bound collective cap)
10. SparsifiedCollective (drop below-threshold harmonic)
11. Hardware-shaped fence/timeline (seq / wait / complete)
12. SoftNPU F16/F32 software IEEE
13. RISC-V S-mode userspace (documented subset)
14. AffinityLaplacian n≤32 placement in sched
15. SpecForge virtio path B (ADR + golden MMIO trace)
16. RISC-V PLIC + SoftNPU software doorbell (path B BAR)
17. aarch64 EL0 userspace (documented subset) — **this cut**
18. Optional virtio-accel QEMU `-device` (path A) — **landed** as a
    host-tested device model; guest PCI bind + MicroPerceptron later

### Active file touch map

| Step | Primary touches |
| --- | --- |
| Soft SMMU SIDs | `core/src/iommu.rs`, `drivers/src/{mmio,softnpu,fakecp}.rs`, tests under `core/` |
| Second software CP | `hal/`, `drivers/` (new backend, not partner-stub paint), `docs/ACCEL.md` |
| Partner-shaped IREE HAL CP | `drivers/src/ireecp.rs`, `hal/src/lib.rs`, `docs/ACCEL.md` (opcode/packet ADR) |
| Cross-cutting | this file, ROADMAP status rows, CI only if a new host test target appears |
| SMP smoke | `kernel/src/arch/{irq,x86_64/{smp,cpu,apic,idt}}.rs`, `Makefile`, `qemu-smp-ci` |
| Per-task PML4 | `kernel/src/{mm,task,elfload}.rs`, `core/src/aspace.rs`, `user/probe/`, `qemu-ci` |
| Cap CDT / revoke | `core/src/caps.rs`, `core/src/demo.rs`, `docs/{SECURITY,ROADMAP,YEAR2_PLAN}.md` |
| aarch64 thin HAL | `boot/aarch64/`, `kernel/src/arch/aarch64/`, `Makefile`, `qemu-aarch64-ci` |
| aarch64 EL0 userspace | `kernel/src/arch/aarch64/`, `kernel/src/{task,syscall,elfload,mm/paging}.rs`, `user/init/`, `core/src/{aspace,sysnr}.rs`, `qemu-aarch64-ci` |
| Multiboot mmap | `core/src/mmap.rs`, `kernel/src/mm/{mod,frame}.rs`, `boot/x86_64/trampoline.S` |
| OperatorKernelHandle | `core/src/opkernel.rs`, `core/src/{caps,demo}.rs`, `docs/{CUT,FABRIC,ROADMAP}.md` |
| SparsifiedCollective | `core/src/sparsify.rs`, `core/src/{opkernel,demo}.rs`, `docs/{CUT,FABRIC,ROADMAP}.md` |
| Hardware fence/timeline | `core/src/fence.rs`, `drivers/src/{fakecp,softnpu}.rs`, `docs/{ACCEL,ARCHITECTURE,ROADMAP}.md` |
| AffinityLaplacian n≤32 | `core/src/{laplacian,cut,sched}.rs`, `docs/{CUT,ROADMAP,YEAR2_PLAN}.md` |
| Virtio path B (golden MMIO) | `drivers/src/{mmio,virtio_accel,softnpu}.rs`, `docs/{ACCEL,ROADMAP,YEAR2_PLAN}.md` |
| RISC-V PLIC SoftNPU doorbell | `kernel/src/arch/riscv64/{plic,idt}.rs`, `kernel/src/{world,task}.rs`, `Makefile` |
| TypedWindow stub (Exploration E) | `core/src/{window,iommu,cut,demo}.rs`, `docs/WINDOW.md` |

The Soft SMMU / Soft-CP track asked not to open `kernel/src/arch/` PRs.
That gate opened after AccelDevice (PR #8). SMP smoke is the first
arch PR on the revised track. CDT landed as a small `aether-core`
slice. OperatorKernelHandle and SparsifiedCollective are the same
kind of slice (caps + Hodge, no new syscall). The fence/timeline
cut is `aether-core` + driver retire (`retire_into`); no new
syscall. Laplacian-in-sched is the same kind of `aether-core` slice
(no new syscall, AccelDevice frozen). Path B is a docs + host-test
slice on the existing BAR (no new syscall, no QEMU device). Still
do not open CXL **product** PRs here. Exploration E (typed-window stub)
is the honest exception: host tests only, no QEMU CXL, no calendar claim.

---

## SpecForge criteria (aspirational appendix)

Original SpecForge half-year acceptance, file touch map, sequencing
risks, and PR order. **Not the active schedule.** KEEP / KILL / DEFER
above override what Kernel actually sequences. Criteria below are
**not done** unless verified on the branch tip.

### Y1H1 — Soft isolation + real accel path

**Done when:**

1. Soft SMMU: `IommuMap` programs non-identity IOVA + `stream_id`; map
   without Memory+MAP still refuses; host + QEMU test shows wrong-stream
   DMA fault/reject (software model OK).
2. Bank QoS credits: `PartitionProfile` credits enforced on submit/arena;
   over-credit → refuse; EventRing logs credit drain.
3. Real virtio-accel path: either (A) QEMU `-device`/`virtio-mmio` that
   DMA-reads the published BAR layout in [ACCEL.md](ACCEL.md) with
   SoftNPU behind it, **or** (B) documented "in-kernel BAR is canonical
   demo" + golden MMIO trace test — pick A if effort fits; B is the
   honest fallback. **Landed as path B** (ADR + frozen BAR + golden
   trace on SoftNPU submit/complete). Path A later landed as
   optional (`qemu/`; stock QEMU stays B).
4. Partner sketch enrichment: `PartnerNpuStub` fills opcode/dtype/route
   from real `AccelJobDesc` fields; [ACCEL.md](ACCEL.md) "how to plug CP"
   updated; still labeled sketch, not partnership.

### Y1H2 — Multi-CPU + HAL maturity

**Done when:**

1. Production HAL for 1–2 CPs: second `AccelDevice` impl beyond SoftNPU
   (can still be software) with distinct backend id; both pass
   probe/submit/poll/map contract tests.
2. SMP: `smp_start_aps` does INIT-SIPI (or QEMU multi-`-smp` bring-up);
   per-CPU `gs`/tick; work-steal driven by ≥2 harts; `make qemu-smp`
   smoke. **Landed** as a QEMU `-smp 2` slice (APs kernel-only; not a
   product scheduler).
3. ELF userspace maturity: per-task PML4 (or documented subset) +
   SMEP/SMAP on x86; `/init` still static ELF; optional second user
   binary; no PIE required. **Landed** as the documented subset
   (per-user CR3, USER-local 2 MiB windows, `/probe`). Higher-half
   (`ffffffff80000000+PA`) plus a boot-time KASLR slide (dual-map),
   PIE `.rela.dyn` + unused-alias unmap, and a KPTI user-CR3 subset
   (trampoline; identity DMA on kernel CR3) landed as later
   documented subsets.

### Y2H1 — Package scale + revoke

**Done when:**

1. Multi-chiplet: affinity graph spans ≥2 chiplets; scheduler refuses
   cross-cut without BIND; AffinityLaplacian used in sched placement
   (not only Cut enum n≤8) for at least an n≤32 host-tested case.
   **Landed** as a prototype (`two_chiplet_mesh` n=16/32, Fiedler
   median-cut, `bind_laplacian_cut`). Not GiFt-Placer. Enumeration
   stays at n≤8. ChipletFleet / chiplet-local steal is **KILL** as a
   calendar item (thin host stub only; not a partner ask).
2. CXL region objects: `MemorySpace::CxlRegion` is a real typed place
   with pin/map path; coherent remote load still refused without
   UNIFIED; QEMU stub window OK — no claim of real CXL.mem.
3. Cap CDT/revoke: revoke parent empties descendants across tables;
   host tests lock it; [SECURITY.md](SECURITY.md) gap table updated.
   **Landed** as a small derivation tree (`parent` + `(tenant, generation)`, `revoke_in`
   of named tables). Property tests lock mint → derive → `revoke_in`;
   formal caps stay killed as a calendar item. No `SYS_REVOKE`. Kernel
   World is still one shared `CapTable`. The Laplacian item above is
   **landed** as a prototype; CXL is **not** done. Exploration E
   (`TypedWindow`) is an honest stub, not this criterion.

### Y2H2 — Bring-up + ecosystem docs

**Done when:**

1. Bring-up playbook: `docs/BRINGUP.md` — QEMU/SoftNPU path, how a
   partner maps CP registers into AccelDevice, failure modes, STUB
   list. **Not** the Soft SMMU software-table kit
   ([bringup/BRINGUP.md](bringup/BRINGUP.md)), which is an optional M2
   leave-behind and is not this Y2H2 climax.
2. Open AccelDevice ecosystem docs: [ACCEL.md](ACCEL.md) +
   [DILIGENCE.md](DILIGENCE.md) refreshed; sample third-party sketch
   crate or `drivers/` template; `site/` roadmap section matches this
   plan (no vendor logos as partners).

### File touch map (expected)

| Milestone | Primary touches |
| --- | --- |
| Y1H1 | `core/src/iommu.rs`, `core/src/partition.rs`, `core/src/fence.rs`, `drivers/src/{mmio,virtio_accel,softnpu,partner}.rs`, `docs/ACCEL.md`, tests under `core/` |
| Y1H2 | `hal/`, `drivers/` (2nd CP), `kernel/src/arch/{x86_64,irq}.rs`, `kernel/src/{mm,task,elfload}.rs`, `boot/x86_64/`, `user/`, `Makefile`, `docs/ARCHITECTURE.md` |
| Y2H1 | `core/src/{laplacian,cut,sched,caps,space}.rs`, `docs/{CUT,SECURITY,ABI}.md` |
| Y2H2 | `docs/BRINGUP.md` (new), `docs/{ACCEL,DILIGENCE,ROADMAP}.md`, `site/` copy only — do not fork marketing claims |

Cross-cutting: this file, ROADMAP status rows, CI (`.github/workflows/ci.yml`)
for new qemu/smp targets.

### Sequencing risks

1. **ABI freeze:** Syscall 0–8 frozen. New surface = new numbers or caps
   bits only after [ABI.md](ABI.md) amendment in the same PR. Do not
   reshape `UserAccelJob` wire without a version bump in [ACCEL.md](ACCEL.md).
2. **Virtqueue BAR layout:** [ACCEL.md](ACCEL.md) BAR offsets are a
   de-facto ABI for SoftNPU ↔ future QEMU device. **Frozen** by the
   path-B golden MMIO trace. Any change needs a dual SoftNPU + doc +
   golden-trace update. Path A later must consume these offsets.
3. **SMP vs userspace:** INIT-SIPI + per-CPU state will thrash
   `arch/x86_64` and `task.rs` the same as PML4 work — sequence Y1H2 as
   **SMP first (kernel threads)** then **per-task PML4**, or one owner
   for both to avoid conflict.
4. **Laplacian in sched:** n≤32 Fiedler placement + `bind_laplacian_cut`
   landed as a prototype. BIND / CrossCut semantics are frozen; do not
   grow `MAX_VERTS` past 32 without a wider mask type. Still not a
   production eigensolve. ChipletFleet is KILL as a calendar item.
5. **Cap CDT:** Touches every mint/derive path. The small revoke
   slice is landed behind host + boot-demo tests; still land any
   later CXL/multi-chiplet demos on that API, not a new tree.
6. **RISC-V / aarch64 temptation:** RISC-V U-mode `/init` + PLIC
   SoftNPU doorbell and aarch64 EL0 `/init` landed as subsets; do
   not block Y1 on virtio-mmio or GICv3. Neither port is a second
   kernel.

### PR order (SpecForge original)

1. `docs/YEAR2_PLAN.md` (this leave-behind) + ROADMAP pointer
2. Y1H1 iommu `stream_id` + credit refuse tests
3. Y1H1 virtio path decision (A vs B) as an explicit ADR in [ACCEL.md](ACCEL.md)
4. Y1H2 SMP smoke
5. Y1H2 PML4
6. Y2H1 CDT → CXL place → laplacian-in-sched
7. Y2H2 docs/site only
