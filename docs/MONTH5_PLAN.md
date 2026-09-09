# Month 5 plan (Falsifier revision)

Leave-behind for Kernel tracking. Filed after M1–M4 + SoftChipletSync
landed on main (PRs #38, #41, #47, #49, #51) plus the Soft SMMU
bring-up kit (#48) and site progress through #52.

**This calendar is closed** (four digests landed). The **next
calendar** is [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) (Sep 2026 → Sep
2028). [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) closed M1–M4.
SoftGreenCtx (digest 1) is **landed**. SpecForge OS-completeness
theater is still not the schedule. Month 5 is **four SpectraScout
exploration digests** — not one pillar, not a half-year of
OS-completeness. SoftGreenCtx and SoftCmdFirewall are **landed**.
SoftCCT (digest 3) is **landed**. SoftSFI (digest 4) is **landed**.
PASID/SVA, OperatorInject, and SoftNoI-IS are H2 2026 explorations
on the two-year plan — **not** marked Done here. SoftNoI-IS is
**in-flight / landing this PR** (admit control, not a fifth digest).
The exploration menu below is a direction list; do not re-schedule
the four landed digests.

## Reality (what already landed)

Do **not** re-schedule any of the following as new milestones.

| Landed | Honest reading |
| --- | --- |
| **M1 IreeShapedCp (PR #38)** | `backend = 4`, frozen `IreeHalCmd`; research opcodes, not a signed vendor |
| **M2 PJRT/IREE shim (PR #41)** | `host/aether-pjrt` packs `IreeHalCmd` → `IreeShapedCp`. Not `GetPjRtApi` |
| **M3 SID-at-submit (PR #49)** | Host1x-shaped SET_SID; SID sticks on the XQueue. Not a Tegra driver |
| **M4 XQueue (PR #47)** | Two software queues; queue-boundary suspend/resume. Not silicon, not XSched LD_PRELOAD |
| **SoftChipletSync (PR #51)** | Scoped `{wave, CU, chiplet, package}` + optional CCT. Fence **counts** only |
| **SoftGreenCtx (PR #54)** | Fake 70/30 SM/WQ partitions; XQueue bind; memcpy BW vs unpartitioned; migrate-to-yield without SID change. Not MIG |
| **SoftCmdFirewall (PR #55)** | Copy-then-validate Soft-CP submit. Host1x lesson. Not confidential GPU |
| **SoftCCT (PR #57)** | Last-writer chiplet elision on SoftChipletSync. Package fence ≪ broadcast; incorrect elision fails. Not a coherence protocol |
| Soft SMMU bring-up kit (PR #48) | Dump/replay of STE→CD→S1/S2 + ATS. Not a Soft-SMMU redo |
| Explorations A–E | A merged into M2; B blast-radius clip; C `ChipletTaskScope` stub; D CDT props; E `TypedWindow` stub |
| Site through PR #52 | Research leave-behind / progress refresh. **Not** a calendar item |

Year-1 + hardening (Soft SMMU, Soft-CP, SMP, PML4, CDT, three-ISA
`/init`, path B/A) stay done as research slices. SoftNPU path-B
opcodes stay Aether-native. `PartnerNpuStub` (`backend = 2`) stays a
labeled no-op.

## Non-negotiables

Falsifier constraints do not relax for Month 5.

- Research prototype. SoftNPU + stock QEMU (in-kernel BAR) is the
  canonical demo (**path B**). A custom QEMU `-device` (**path A**) is
  optional (`qemu/`, `make qemu-accel`); stock `make qemu` stays B.
- Soft SMMU is **software** (`IommuMap` STE→CD→S1/S2). Not hardware
  SMMU, not an SMMUv3 emulator.
- No fake NVIDIA partnership, no FLOP benchmarks, no tape-out /
  readiness claims.
- [`docs/ABI.md`](ABI.md) syscall **0–11 stay frozen**; additive only.
  Do not reshape `UserAccelJob` without a version bump in
  [ACCEL.md](ACCEL.md).
- Prefer extending `aether_hal::AccelDevice` + `AccelJobDesc` /
  `CpCmd` / `IreeHalCmd` over inventing a second IR.
- **No OS-completeness theater as M5.** Fork, POSIX `open`/`read`,
  CXL.mem productization, ChipletFleet, formal seL4 caps, and
  site-as-milestone are not this month.
- The four digests are **not one spine**. Do not market them as a
  product isolation story or a CUDA/Host1x/MIG port.

## Month 5 calendar — four exploration digests

Ship in this order. Each is a thin PR with host tests and docs
non-claims. They are **not** a single pillar and **not** M5–M8
numbers. Skip a digest only if its honest slice is already met on
main (SoftCCT: see below).

```text
SoftGreenCtx (landed)  →  SoftCmdFirewall (landed)  →  SoftCCT (landed)  →  SoftSFI (landed)
SoftNoI-IS in-flight (this PR; H2 2026 — not a fifth digest)
PASID / SVA  and  OperatorInject deepen  parked leftovers
```

### 1. SoftGreenCtx (**landed**)

SM / work-queue **partitions** on Soft-CP. Inspiration: CUDA Green
Contexts / DetShare — not a CUDA driver, not MIG-class isolation.

An XQueue binds a `SoftGreenCtx` (software partition of a fake SM
pool). SID-at-submit is unchanged on migrate. A 70/30 split of the
fake pool, two XQueues, shows bandwidth interference versus the
unpartitioned case. `migrate-to-yield` is queue-boundary (same
honesty as M4): a command already inside `service()` runs to
completion.

**Done when (met):**

1. Soft-CP exposes a software `SoftGreenCtx` (partition of a fake SM
   / WQ pool). An XQueue binds one; SID sticks / inherits as today.
2. Host tests: 70/30 split; two XQueues; partitioned BW interference
   versus unpartitioned; migrate-to-yield at a queue boundary. SID
   unchanged across migrate.
3. Docs: Green Context / DetShare inspiration only. **Not** MIG.
   **Not** hardware SM partitioning. No new syscall. `CpCmd` layout
   unchanged.

### 2. SoftCmdFirewall (**landed**, PR #55)

Copy-then-validate submit. Inspiration: Host1x “don’t execute the
caller’s live buffer” — not a Tegra driver, not a confidential GPU.

Submit copies the command buffer, validates opcodes / relocs / SID /
caps on the **copy**, then enqueues. A mutation-during-validate race
fails without the firewall and passes with it.

**Done when (met on this branch):**

1. Soft-CP (and IreeShapedCp mailbox if it shares the path) copies
   the cmdbuf before validate + enqueue. Validation covers opcodes,
   relocs, SID, and the Memory+MAP / submit cap walk.
2. Host tests: mutation-during-validate sneaks without the
   firewall and is ignored with it; existing wrong-SID / unbound refuse
   unchanged.
3. Docs: Host1x lesson, software only. **Not** confidential compute.
   **Not** a silicon command parser. No new syscall.

### 3. SoftCCT (**landed**)

Chiplet Coherence Table on the **already-landed** SoftChipletSync
(CPElide inspiration). Soft-CP buffer labels + last-writer chiplet.
SoftChipletSync issues a package-scope fence **only** on a
cross-chiplet hazard. Same-chiplet consume on ≥2 fake chiplets
elides. Single-chiplet CCT is a no-op.

This digest **deepens** PR #51 (scoped timelines + optional CCT);
it is not a second CCT object.

- chiplet0 → chiplet1 **labeled** buffer producer/consumer
- package-fence count ≪ broadcast baseline
- **incorrect** elision (elide whenever a label is known) **fails**

Do not claim a latency win from single-die QEMU / host numbers.
Multi-chiplet **sim** metrics stay in the exploration menu.

**Done when (met):**

1. Labeled-buffer chiplet0→1 path is explicit (`SoftCct` +
   `submit_scoped` write/read labels).
2. Host tests: package-fence ≪ broadcast; incorrect elision is
   no-elide / PackageFence; single-chiplet is a no-op. Existing
   CCT tests still pass.
3. Docs name CPElide as inspiration only. **Not** a coherence
   directory, **Not** UCIe, **Not** ChipletFleet placement, **Not**
   a Vulkan / ROCm product. Fence **counts** only.

### 4. SoftSFI (**landed**)

Soft-CP bytecode **memory sandbox**. Inspiration: GPU-AToLL-shaped
verifier — not a full safe multi-tenant kernel claim.

A toy ISA (`load` / `store` / `add` / `dma`) verifier accepts
load/store/dma whose `base+bound` stays inside the SID-mapped
range and rejects OOB. Digest 4 left atomics / tensor / heap
**refused**. Two tenants: SFI + SID together (skip-verify fault
injection still traps; in-range SID-A must not touch SID-B pins).

**Later (SoftSFI widen):** `atomic_add` is a SID-proved toy fetch-add
(in-range accept, cross-tenant `Oob`). Sequential RMW, not a hardware
atomic. Tensor / heap stay `Unmodeled`. Still not “safe multi-tenant
kernels.”

**Contract** (`aether_core::softsfi` + Soft-CP `submit_sfi`):

1. Soft-CP (own bytecode only) runs a bounds verifier on
   load/store/dma/`atomic_add` against the submit SID’s Soft-SMMU
   IOVA window.
2. Host tests: in-bounds accept; OOB reject; SID-proved `atomic_add`
   (in-range accept, cross-tenant `Oob`); two tenants SFI+SID
   (A cannot load/store/RMW B’s pin; skip-verify does not cross-read).
   Existing SET_SID refuse unchanged.
3. Docs: GPU-AToLL pattern, software sandbox. **Not** a verified
   multi-tenant GPU, **Not** NVRTC, **Not** confidential GPU, **not**
   “safe multi-tenant kernels.” No new syscall. Own IR only.

Primary touches: `core/src/softsfi.rs` (ISA + verifier),
`drivers/src/softsfi.rs` (Soft-CP submit/inject; keep `fakecp.rs`
thin so SoftGreenCtx / SoftCmdFirewall can land beside this).

## Parked leftovers (not this month)

These stay documented so they are not lost. They are **not** the
Month 5 clock.

### PASID / SVA

Per-`AccelDevice` PASID space. Bind process VA ↔ Soft-SMMU SSID;
Soft-CP DMA uses VA; host unmap → SSID TLB invalidate; stale
translate faults. Software only.

**Overclaim watch** if pulled later: no “zero-copy SVA” / “unified
VA” without the unmap → invalidate path in the same PR.

**Done when (if pulled):** bind mm↔ssid; Soft-CP DMA via VA;
unmap→SSID TLB invalidate; stale translate faults; docs non-claims
(not ARM SVA, not PCIe PASID/PRI, not CUDA UVA). No new syscall.

### OperatorInject deepen

Soft-CP resident worker + versioned injectable ops (`memcpy` /
`saxpy` + hot-add `scale`) without Soft-CP restart. SID still at
submit. SoftCmdFirewall copy-then-validate admits the packed image.
Own bytecode / IR only — not NVRTC / CUDA.

Distinct from landed `OperatorKernelHandle` Hodge inject.
GPUOS / Mirage MPK inspiration only. Not a full LLM compiler, not
NVIDIA. H2 2026 leftover on [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) —
**not** marked Done.

### SoftNoI-IS (in-flight / landing this PR)

Interference Score admit policy (PARL / NoI inspiration).
SoftChipletSync fabric IS estimate; refuse when `IS > budget`.
Solo vs concurrent → IS; refuse `IS > 1.5`. Two Soft-CP tenants on
a shared fake NoI. **Admit control, not topology synthesis, not
UniCNet.** H2 2026 exploration on [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md).
Not a fifth Month 5 digest. Do not mark Done until this PR merges.

**Done when (this PR; not marked Done here):**

1. SoftChipletSync advertises a per-tenant IS estimate on a fake
   shared NoI. Soft-CP / XQueue admit refuses when projected IS >
   budget.
2. Host tests: solo vs concurrent throughput → IS; policy refuse
   when IS > 1.5; demo logs admit/refuse.
3. Docs: PARL/NoI inspiration. **Not** optimal NoI design. **Not**
   UniCNet. Path B SoftNPU / `make qemu` unchanged. No new syscall.

## Optional gated digests (still not pillars)

Thin PRs. They may land beside the four if their gate fires. They
do **not** get an M-number. If the gate is closed, leave them
killed.

| Digest | Gate | Honest bound |
| --- | --- | --- |
| **FlowHodgeQuota deepen** | Shim injects fabric **class headers** on Soft-CP DMA | **Stays killed as theater.** Already-landed Hodge quota without DMA headers. The interesting path is a thin software tag on `AccelJobDesc.flow` feeding SoftNoI admit (Curl ring reserve + admit/refuse counters) — not this digest, not a vendor header |
| **MicroPerceptron interop** | Secondary to PJRT; consume frozen `IreeHalCmd` and/or path-A BAR | Not a second compiler story. Not a plugin |
| **Guest PCI path A bind** | Soft-SMMU IOVA demo **needs** BAR DMA | **Host proof landed** (`PathABar` + `make accel-test`). Kernel `VirtioAccelMmio` PCI BAR0 still not required. Path B stays canonical. Do not rebuild QEMU in CI |
| **Per-task CapTable** | Two shim tenants **alias slots** on the shared World table | Isolate those tenants. Additive `SYS_REVOKE` only if the same PR demos revoke → `unbind_stream` / FLR |
| **Blast-radius deepen** | XQueue freeze + SID-at-submit already cover the clip | One more two-tenant refuse (wrong SoftSFI / SoftGreenCtx) if it earns a new line. Not a second ring-3 World |

## Exploration menu (directions, not calendar)

Kernel / user may pull these later. SpectraScout filed five M5–6
bets as a **menu** (suggested order below). A later redirect put
the first four on the Month 5 clock; they are still **not one
spine**. SoftNoI-IS is **in-flight / landing this PR** (H2 2026;
not marked Done here). PASID/SVA, OperatorInject, and FlowHodgeQuota
stay parked / gated. Skip shipped work (SoftChipletSync CCT in
PR #51 is a deepen, not a re-landing).

Suggested digest order (menu):

```text
SoftGreenCtx  →  SoftCmdFirewall  →  SoftCCT  →  SoftSFI  →  SoftNoI-IS
```

### SpectraScout M5–6 bets

| Bet | Status | Slice | Honest bound |
| --- | --- | --- | --- |
| **SoftGreenCtx** | **Landed** | 70/30 fake SM pool; two XQueues bind a `SoftGreenCtx`; BW interference vs unpartitioned; migrate-to-yield (queue-boundary); SID unchanged on migrate | CUDA Green Contexts / DetShare inspiration. **Not MIG.** |
| **SoftCmdFirewall** | **Landed** (Month 5 digest 2) | Copy cmdbuf → validate opcodes / relocs / SID / caps → enqueue. Mutation-during-validate sneaks without the firewall, ignored with it | Host1x lesson. **Not confidential GPU.** |
| **SoftCCT** | **Landed** (Month 5 digest 3) | chiplet0→1 labeled buffer; package-fence ≪ broadcast; incorrect elision fails; single-chiplet no-op | CPElide last-writer table. **Not UCIe.** Not a coherence protocol |
| **SoftSFI** | **Landed** (digest 4) | Toy ISA: accept in-bounds load/store/`atomic_add` in the SID range; reject OOB / cross-tenant; two tenants SFI+SID. Tensor / heap `Unmodeled` | GPU-AToLL pattern. **Not** a full safe multi-tenant kernel claim |
| **SoftNoI-IS** | **In-flight** (this PR; H2 2026) | SoftChipletSync fabric IS estimate; solo vs concurrent → IS; refuse `IS > 1.5` (or budget) | PARL / NoI inspiration. **Admit control, not topology synth.** Not UniCNet. Not a Month 5 digest |
| PASID / SVA | **Parked leftover** | per-AccelDevice PASID; bind VA↔SSID; unmap→invalidate; stale fault | Software only. No zero-copy SVA without invalidate |
| OperatorInject | **Parked leftover** | Resident worker + memcpy/saxpy + hot-add scale; no Soft-CP restart | Own IR. **Not NVRTC/CUDA**. Not a full LLM compiler |
| FlowHodgeQuota | Gated digest | **Killed as theater** (no DMA class headers). SoftNoI consumes a software `FlowClass` tag instead |

### Partner / HAL

- PJRT Event timeline polish (**landed**): `host/aether-pjrt` Event
  create / record / wait on existing fences / SoftChipletSync. Still
  not `GetPjRtApi` / XLA / `iree_hal_driver_t`. TRANSFER stays reserved.
- PJRT shim more ops — still open (Nop / MatMul / Wave only).
- MicroPerceptron / virtio-accel consumer (same frozen `IreeHalCmd`
  or path-A BAR; secondary to PJRT). Remains later / optional. The
  doorbell sketch (`examples/accel-client`) is a second caller of the
  packet, not this port.
- Frozen opcode table **v2** — only with a dual update of
  [ACCEL.md](ACCEL.md) **and** `drivers/src/ireecp.rs`. A one-sided
  bump is a break.

### Isolation / Soft SMMU

- PASID / SVA (parked leftover; see above).
- Guest PCI path-A IOVA proof (**landed** as host contract; BAR DMA
  initiator + Soft SMMU ssid 4 + wrong-SID abort). Kernel PCI bind
  still optional / not required.
- Per-task CapTable + additive `SYS_REVOKE` **only if**
  revoke → `unbind_stream` / FLR is the demo. Internal `revoke` /
  `revoke_in` already exist.
- SoftSFI (Month 5 digest 4) — **landed**. SoftCmdFirewall (**landed**).

### Soft-CP / sched

- SoftGreenCtx (**landed**).
- OperatorInject mega-kernel lite (parked leftover).
- XQueue mid-op pretends — honest levels only (queue-boundary is
  what landed; do not claim intra-`service()` preempt).
- SoftCCT deepen / multi-chiplet **sim** metrics (digest 3 is the
  labeled-buffer slice; sim metrics stay later). Single-die QEMU /
  host numbers are still not partner proof.

### Fabric / spectral

- FlowHodgeQuota class tags on Soft-CP DMA (gated; headers required —
  **killed**). SoftNoI uses a software enum on `AccelJobDesc.flow`.
- SoftNoI-IS (**in-flight / this PR**; admit control, not topology synth).
- AffinityLaplacian / SpectralCut diligence clips. Not GiFt-Placer.
  ChipletFleet stays killed as calendar.

### Ports (hard defer unless a digest needs them)

- RISC-V virtio-mmio SoftNPU (PLIC software doorbell is enough).
- aarch64 GIC SoftNPU IRQ (EL0 `/init` drains on timer/kthread).

## Explicitly killed / skip

These do **not** get a Month 5 PR. Some exist as honest stubs.

| Item | Why it stays killed |
| --- | --- |
| Fork-shaped clone | `SYS_CLONE` shares aspace; a new PML4/`fork` is POSIX theater |
| User `open`/`read` POSIX surface | Ramfs is kernel-internal; no `SYS_OPEN` / `SYS_READ` |
| Soft-SMMU redo | STE→CD→S1/S2 + ATS + bring-up kit already landed |
| CXL productization | `MemorySpace::CxlRegion` stays a typed place; no CXL.mem claim |
| ChipletFleet as calendar | n≤32 Fiedler + `ChipletTaskScope` stub; not Fleet marketing |
| Formal seL4-style caps | Small CDT landed; proofs are a non-claim |
| Site-as-milestone | PR #37 / #46 / #50 / #52 are leave-behinds; not a climax |
| SMMUv3 emulator | Software tables only; partner silicon for hardware SMMU |
| UCIe PHY | Transport. SoftChipletSync is not a coherence protocol |
| `PartnerNpuStub` paint | No-op sketch; M1 already shipped a real packet |
| Fake NVIDIA / FLOPs / tape-out | Standing non-claim |

**Skip** (same list, said once): fork, POSIX open/read, Soft-SMMU
redo, CXL productization, ChipletFleet calendar, formal seL4 caps,
site-as-milestone, SMMUv3 emulator, UCIe PHY, PartnerNpuStub paint,
fake NVIDIA / FLOPs / tape-out.

## Kernel PR order (this month)

1. This file + pointers from [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md)
   and [ROADMAP.md](ROADMAP.md) — **this cut**
2. SoftGreenCtx — **landed** (PR #54)
3. SoftCmdFirewall — **landed** (PR #55)
4. SoftCCT — **landed** (PR #57)
5. SoftSFI — **landed** (toy ISA + SID sandbox; not NVVM)

Do not open calendar PRs for the killed list or the remaining
parked leftovers (PASID/SVA, OperatorInject) unless the user
redirects. SoftNoI-IS is the H2 2026 pull landing this PR — see
[TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md); not marked Done here. A later
site progress refresh is not a milestone.

### File touch map

| Step | Primary touches |
| --- | --- |
| SoftGreenCtx (**landed**) | `drivers/src/fakecp.rs` (partition + XQueue bind), host tests, [ACCEL.md](ACCEL.md) |
| SoftCmdFirewall | `drivers/src/firewall.rs` + Soft-CP `submit_xqueue` / `submit_cmdbuf`, host tests — **landed** |
| SoftCCT | `core/src/chipsync.rs` (`SoftCct`), Soft-CP / IreeShapedCp `submit_scoped`, host tests — **landed** |
| SoftSFI | `core/src/softsfi.rs` + `drivers/src/softsfi.rs` (toy ISA + SID window). Keep `fakecp.rs` thin — **landed** |
| SoftNoI-IS (in-flight / this PR) | `core/src/noi.rs` + SoftChipletSync advertisement + `drivers/src/noi.rs` XQueue admit, host tests — not a Month 5 digest |
| PASID / SVA (parked) | `core/src/iommu.rs`, `drivers/src/fakecp.rs` — not this month |
| OperatorInject (parked) | `core/src/opinject.rs` + `drivers/src/opinject.rs` resident worker — H2 leftover on [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md); **not** marked Done |
| FlowHodgeQuota digest | stays killed (no DMA class headers). SoftNoI tag: `AccelJobDesc.flow` |
| MicroPerceptron digest | host crate or virtio-accel consumer of frozen `IreeHalCmd` |
| Path-A guest bind | host `PathABar` IOVA / wrong-SID **landed**; kernel PCI bind still optional. CI still does not rebuild QEMU |
| Per-task CapTable | `core/src/caps.rs`, kernel World; `SYS_REVOKE` only with unbind/FLR demo |
| Blast-radius digest | `core/src/blast.rs` / host tests; extend, do not rebuild |

Cross-cutting: this file, SIX_MONTH_PLAN leftover pointer, ROADMAP
status pointer. CI only if a new host-test target appears. ABI 0–11
frozen. Path B canonical.

## Relationship to SIX_MONTH_PLAN / ROADMAP

[SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) is the closed M1–M4 calendar
(plus SoftChipletSync). This file is the closed Month 5 record.

[TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) is the Kernel calendar
(Sep 2026 → Sep 2028). [YEAR2_PLAN.md](YEAR2_PLAN.md) still holds
the 2026-09-06 Falsifier ACTIVE track (done through PR #37) and the
SpecForge appendix (aspirational). Do not sequence new work against
this file or YEAR2_PLAN.

[ROADMAP.md](ROADMAP.md) points at [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md)
for what to sequence next. Suggested next cuts in ROADMAP that are
not H2 2026 leftovers remain **technical leftovers**.

## What we will not claim

- That SoftGreenCtx is MIG, CUDA Green Contexts, or hardware SM
  isolation
- That SoftCmdFirewall is a confidential GPU or a Host1x driver
- That SoftCCT is CPElide silicon, a coherence protocol, UCIe, or
  a multi-chiplet latency result from single-die host tests
- That SoftSFI is a verified multi-tenant GPU or a safe-kernel
  product
- That SoftNoI-IS synthesizes topology, is UniCNet, or is already
  Done on MONTH5 / main (it is in-flight / landing this PR)
- That PASID/SVA is ARM SVA, PCIe PASID/PRI, hardware ATS, or a
  CUDA unified virtual address space
- Zero-copy / unified VA without the unmap → SSID TLB invalidate path
- That OperatorInject is NVRTC, CUDA, or a vendor compiler
- That M1–M4, SoftChipletSync, or the Soft SMMU kit became hardware
- That four digests are one spine, or that the exploration menu is
  a calendar
- Benchmarks, FLOPs, tape-out, seL4 proofs, or a signed vendor ISA
- An OS-completeness Month 5 (fork, POSIX, CXL.mem, ChipletFleet,
  SMMUv3 emulator, UCIe PHY, site-as-milestone)
