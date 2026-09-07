# Month 5 plan (Falsifier revision)

Leave-behind for Kernel tracking. Filed after M1–M4 + SoftChipletSync
landed on main (PRs #38, #41, #47, #49, #51) plus the Soft SMMU
bring-up kit (#48) and site progress through #52.

**This is the next calendar.** [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md)
closed M1–M4. SpecForge OS-completeness theater is still not the
schedule. Pick **one** primary spine. Optional digests are not
pillars. The exploration menu below is a direction list, not a
half-year clock.

## Reality (what already landed)

Do **not** re-schedule any of the following as new milestones.

| Landed | Honest reading |
| --- | --- |
| **M1 IreeShapedCp (PR #38)** | `backend = 4`, frozen `IreeHalCmd`; research opcodes, not a signed vendor |
| **M2 PJRT/IREE shim (PR #41)** | `host/aether-pjrt` packs `IreeHalCmd` → `IreeShapedCp`. Not `GetPjRtApi` |
| **M3 SID-at-submit (PR #49)** | Host1x-shaped SET_SID; SID sticks on the XQueue. Not a Tegra driver |
| **M4 XQueue (PR #47)** | Two software queues; queue-boundary suspend/resume. Not silicon, not XSched LD_PRELOAD |
| **SoftChipletSync (PR #51)** | Scoped `{wave, CU, chiplet, package}` + optional CCT. Fence **counts** only |
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

## Pick-one primary (the calendar)

Month 5 is **one spine**. Do not run both as pillars. Digests below
may land beside the chosen spine if their gates fire; they do not
replace it.

### 1. PASID / SVA — recommended M5

Per-`AccelDevice` PASID space. Bind a process VA range to a Soft-SMMU
SSID (software CD). Soft-CP DMA names **VA**, not a host-pinned IOVA
the caller already resolved. Host unmap of that VA **must** invalidate
the SSID TLB (software ATC / `InvCmd::{Ats,Tlbi,CfgCd}`). A stale
translate after unmap is a **fault**, not a silent hit.

This is the leftover SpectraScout isolation item. Soft SMMU already
walks STE→CD→S1/S2 and ATS-invalidates a software ATC. SID-at-submit
already arms a stream at the doorbell. PASID/SVA adds the
**process-ASID** noun: one AccelDevice, many client address spaces,
each bound to its own SSID.

**Software only.** Not ARM SVA, not PCIe PASID/PRI, not ATS hardware,
not a CUDA unified VAS, not zero-copy without an invalidation path.

```text
AccelDevice  →  PASID space (software; per-device, not global)
process VA   ↔  Soft-SMMU SSID (CD)
Soft-CP DMA  →  translate(VA, pasid/ssid) via STE→CD→S1→S2
host unmap   →  InvCmd on that SSID (ATC + S1 drop)
stale VA     →  NotMapped / StreamAbort  (never a cached PA)
```

**Overclaim watch.** Do not write “zero-copy SVA” or “unified VA”
unless the unmap → SSID TLB invalidate path is in the same PR and
host-tested. A bind without invalidate is IOVA-with-extra-nouns.

**Done when:**

1. Host tests bind a process mm (guest VA range + Memory+MAP) to a
   Soft-SMMU SSID / PASID on one `AccelDevice`.
2. Soft-CP DMA issues through **VA** on that binding (resolve walks
   STE→CD→S1→S2; IOVA is not identity; SID-at-submit still required).
3. Host unmap of the VA range issues an SSID-scoped invalidate
   (`InvCmd` ATS/TLBI/CfgCd). A subsequent translate of the stale VA
   is `NotMapped` / `StreamAbort` — not a cached ATC hit.
4. Docs ([ACCEL.md](ACCEL.md), this file) name the non-claims: software
   PASID, not hardware SVA, not zero-copy without invalidate, not a
   CUDA UVA. No new syscall. Path B SoftNPU / `make qemu` unchanged.
   `IreeHalCmd` / `CpCmd` layouts unchanged unless ACCEL.md + both
   packers update together.

### 2. Alternate — OperatorInject deepen

Pick this **only if** PASID/SVA is explicitly not wanted this month.

Soft-CP already has a packed `CpCmd` surface, SET_SID-at-submit, and
two XQueues. OperatorInject deepen is a **resident worker** on that
CP: versioned injectable ops (`memcpy`, `saxpy`, plus a third op
hot-added) that land **without restarting Soft-CP**. SID still stamps
at submit. Own bytecode / IR only.

**Not** `OperatorKernelHandle` Hodge inject (that already landed).
**Not** an NVRTC / CUDA demo. **Not** a vendor compiler. **Not** a
second packet format unless [ACCEL.md](ACCEL.md) versions it.

**Done when:**

1. Soft-CP keeps a resident worker; `memcpy` and `saxpy` are
   injectable versioned ops on the existing `CpCmd` / XQueue path.
2. A third op hot-adds while the CP stays up (no device recreate).
   SID-at-submit still refuses unbound / wrong-SSID.
3. Host tests: inject → submit → poll; hot-add third; restart is
   **not** required; wrong-SID still Fault. Docs: own IR, not NVRTC.
   No new syscall.

## Optional Month 5 digests (not pillars)

Thin PRs. They may land beside the primary. They do **not** get an
M-number. If the gate is closed, leave them killed.

| Digest | Gate | Honest bound |
| --- | --- | --- |
| **FlowHodgeQuota deepen** | Shim injects fabric **class headers** (Gradient / Curl / Harmonic) on Soft-CP DMA | Admit/refuse counters under overload. Already-landed quota without class headers stays killed as theater |
| **MicroPerceptron interop** | Secondary to PJRT; consume frozen `IreeHalCmd` and/or path-A BAR | Not a second compiler story. Not a plugin |
| **Guest PCI path A bind** | Soft-SMMU IOVA demo **needs** BAR DMA | Kernel `VirtioAccelMmio` talks PCI BAR0. Path B stays canonical. Do not rebuild QEMU in CI |
| **Per-task CapTable** | Two shim tenants **alias slots** on the shared World table | Isolate those tenants. Additive `SYS_REVOKE` only if the same PR demos revoke → `unbind_stream` / FLR |
| **Blast-radius deepen** | XQueue freeze + SID-at-submit already cover the clip | One more two-tenant refuse (wrong PASID / stale VA) if PASID is the spine. Not a second ring-3 World |

## Exploration menu (directions, not calendar)

Kernel / user may pull these later. **None of these is Month 5
unless it is the chosen primary or a gated digest above.**

### Partner / HAL

- PJRT shim: more ops / Event timeline polish (`host/aether-pjrt`).
  Still not `GetPjRtApi` / `iree_hal_driver_t`.
- MicroPerceptron / virtio-accel consumer (same frozen `IreeHalCmd`
  or path-A BAR; secondary to PJRT).
- Frozen opcode table **v2** — only with a dual update of
  [ACCEL.md](ACCEL.md) **and** `drivers/src/ireecp.rs`. A one-sided
  bump is a break.

### Isolation / Soft SMMU

- PASID / SVA (recommended M5 spine; see above).
- Guest PCI path-A IOVA proof (digest; BAR DMA only).
- Per-task CapTable + additive `SYS_REVOKE` **only if**
  revoke → `unbind_stream` / FLR is the demo. Internal `revoke` /
  `revoke_in` already exist.

### Soft-CP / sched

- OperatorInject mega-kernel lite (alternate M5 spine).
- XQueue mid-op pretends — honest levels only (queue-boundary is
  what landed; do not claim intra-`service()` preempt).
- SoftChipletSync CCT deepen / multi-chiplet **sim** metrics.
  Single-die QEMU / host numbers are still not partner proof.

### Fabric / spectral

- FlowHodgeQuota class tags on Soft-CP DMA (digest; headers required).
- AffinityLaplacian / SpectralCut diligence clips. Not GiFt-Placer.
  ChipletFleet stays killed as calendar.

### Ports (hard defer unless the spine needs them)

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
2. **One** of: PASID/SVA (recommended) **or** OperatorInject deepen
3. Gated digests only if their gate is open (same PR or a thin
   follow-up; not a second spine)

Do not open calendar PRs for the killed list. A later site progress
refresh is not a milestone.

### File touch map

| Step | Primary touches |
| --- | --- |
| PASID / SVA | `core/src/iommu.rs` (PASID↔SSID bind, unmap→invalidate), `drivers/src/fakecp.rs` (DMA via VA), host tests, [ACCEL.md](ACCEL.md) |
| OperatorInject (alt) | `drivers/src/fakecp.rs` (resident worker + versioned ops), [ACCEL.md](ACCEL.md) if the packet versions, host tests |
| FlowHodgeQuota digest | `core/src/hodge.rs`, Soft-CP DMA header inject, counters; only with class headers |
| MicroPerceptron digest | host crate or virtio-accel consumer of frozen `IreeHalCmd` |
| Path-A guest bind | guest `VirtioAccelMmio` only; CI still does not rebuild QEMU |
| Per-task CapTable | `core/src/caps.rs`, kernel World; `SYS_REVOKE` only with unbind/FLR demo |
| Blast-radius digest | `core/src/blast.rs` / host tests; extend, do not rebuild |

Cross-cutting: this file, SIX_MONTH_PLAN leftover pointer, ROADMAP
status pointer. CI only if a new host-test target appears. ABI 0–11
frozen. Path B canonical.

## Relationship to SIX_MONTH_PLAN / ROADMAP

[SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) is the closed M1–M4 calendar
(plus SoftChipletSync). This file replaces its leftover list as the
Kernel clock.

[YEAR2_PLAN.md](YEAR2_PLAN.md) still holds the 2026-09-06 Falsifier
ACTIVE track (done through PR #37) and the SpecForge appendix
(aspirational). Do not sequence Month 5 against either.

[ROADMAP.md](ROADMAP.md) points here for what to sequence next.
Suggested next cuts in ROADMAP that are not this spine remain
**technical leftovers**.

## What we will not claim

- That PASID/SVA is ARM SVA, PCIe PASID/PRI, hardware ATS, or a
  CUDA unified virtual address space
- Zero-copy / unified VA without the unmap → SSID TLB invalidate path
- That OperatorInject is NVRTC, CUDA, or a vendor compiler
- That M1–M4, SoftChipletSync, or the Soft SMMU kit became hardware
- That a digest is a pillar, or that the exploration menu is a calendar
- Benchmarks, FLOPs, tape-out, seL4 proofs, or a signed vendor ISA
- An OS-completeness Month 5 (fork, POSIX, CXL.mem, ChipletFleet,
  SMMUv3 emulator, UCIe PHY, site-as-milestone)
