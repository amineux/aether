# Two-year plan (Sep 2026 → Sep 2028)

Leave-behind for Kernel tracking. Filed 2026-09-08 after Year-1 +
hardening, closed M1–M4, SoftChipletSync, the Soft SMMU bring-up kit,
and Month 5 digests 1–4 landed on main.

**This is the Kernel calendar** for Sep 2026 → Sep 2028.
[MONTH5_PLAN.md](MONTH5_PLAN.md) is the closed Month 5 record (four
digests landed). [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) is the closed
M1–M4 record. [YEAR2_PLAN.md](YEAR2_PLAN.md) stays the 2026-09-06
Falsifier ACTIVE track (done through PR #37) plus the SpecForge
appendix (aspirational only). Do not sequence new work against those
closed files.

Research prototype. Path B is canonical. Soft SMMU is software. No
fake NVIDIA partnership, no FLOP benchmarks, no tape-out claim.

## Non-negotiables

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
  `CpCmd` / frozen `IreeHalCmd` over inventing a second IR.
- Site progress refreshes are **not** milestones.

## Reality on main (through 2026-09-07)

Do **not** re-schedule any of the following as new milestones. Do
**not** rewrite the landed-PR record in
[SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) or
[MONTH5_PLAN.md](MONTH5_PLAN.md).

| Landed | Honest reading |
| --- | --- |
| Year-1 + hardening | Soft SMMU, Soft-CP, SMP, PML4, CDT, three-ISA `/init`, path B/A — research slices, not a product kernel |
| **M1 IreeShapedCp (PR #38)** | `backend = 4`, frozen `IreeHalCmd`; research opcodes, not a signed vendor |
| **M2 PJRT/IREE shim (PR #41)** | `host/aether-pjrt` packs `IreeHalCmd` → `IreeShapedCp`. Not `GetPjRtApi` |
| **M3 SID-at-submit (PR #49)** | Host1x-shaped SET_SID; SID sticks on the XQueue. Not a Tegra driver |
| **M4 XQueue (PR #47)** | Two software queues; queue-boundary suspend/resume. Not silicon, not XSched LD_PRELOAD |
| **SoftChipletSync (PR #51)** | Scoped `{wave, CU, chiplet, package}`. Fence **counts** only. Not UCIe, not Vulkan |
| Soft SMMU bring-up kit (PR #48) | Dump/replay of STE→CD→S1/S2 + ATS. Not a Soft-SMMU redo. This is the silicon handoff |
| MONTH5_PLAN (PR #53) | Closed Month 5 calendar. Four exploration digests, not one pillar |
| **SoftGreenCtx (PR #54)** | Fake 70/30 SM/WQ partitions. Not HW MIG |
| **SoftCmdFirewall (PR #55)** | Copy-then-validate Soft-CP submit. Not confidential GPU |
| **SoftSFI (PR #56)** | Toy Soft-CP load/store/add/dma + SFI verifier. Not NVVM. Atomics / tensor / heap refused |
| **SoftCCT (PR #57)** | Last-writer chiplet elision on SoftChipletSync. Not a coherence protocol |
| Explorations A–E | A merged into M2; B blast-radius clip; C `ChipletTaskScope` stub; D CDT props; E `TypedWindow` stub |
| Site through PR #52 / #58 | Research leave-behind / progress refresh. **Not** a calendar item |

SoftNPU path-B opcodes stay Aether-native. `PartnerNpuStub`
(`backend = 2`) stays a labeled no-op.

**In flight / landing this PR as exploration — do not mark Done until merge:**

- SoftNoI-IS admit (this PR)
- PASID/SVA software bind + invalidate
- OperatorInject hot-add

Those three sit in H2 2026 below. They are not Month 5 digests.
SoftNoI-IS is in-flight on this branch; do not treat it as Done on
main until this PR merges. PASID/SVA and OperatorInject stay parked.

## H2 2026 (now → Dec 2026) — finish partner-HAL leftovers

Finish the parked partner-HAL leftovers as **explorations**, not as
M5–M8 numbers and not as a second half-year of
[MONTH5_PLAN.md](MONTH5_PLAN.md).

```text
SoftNoI-IS (exploration)  →  PASID/SVA bind + invalidate  →  OperatorInject hot-add
optional: MicroPerceptron consumer of frozen IreeHalCmd (secondary to PJRT)
gated: Guest PCI path A  — only if Soft-SMMU IOVA demo needs BAR
site progress refreshes — not milestones
```

### SoftNoI-IS admit (exploration)

Interference Score admit policy (PARL / NoI inspiration).
SoftChipletSync fabric IS estimate; refuse when `IS > budget`.
Slice: solo vs concurrent → IS; refuse `IS > 1.5` (or the named
budget). **Admit control, not topology synthesis.** IS path landed
in PR #60.

This PR adds a thin software fabric-class tag
(`AccelJobDesc.flow`, from collective type: allreduce/tree,
ring-exchange/curl, persistent/harmonic). SoftNoI uses the tag as
an admit input: Curl needs reserved ring capacity; host tests show
class changes admit/refuse, not a renamed IS. Not a vendor header.
Not FlowHodgeQuota DMA-header theater.

**Done when:** host tests show solo vs concurrent IS; refuse
`IS > budget`; class tag changes admit (Curl ring); docs name PARL /
NoI as inspiration only. No FLOPs. No “optimal NoI.” No new
syscall. Path B / `make qemu` unchanged.

### PASID / SVA software bind + invalidate

Per-`AccelDevice` PASID space. Bind process VA ↔ Soft-SMMU SSID;
Soft-CP DMA uses VA; host unmap → SSID TLB invalidate; stale
translate faults. Software only.

**Overclaim watch:** no “zero-copy SVA” / “unified VA” without the
unmap → invalidate path in the same PR.

**Done when:** bind mm↔ssid; Soft-CP DMA via VA; unmap→SSID TLB
invalidate; stale translate faults; docs non-claims (not ARM SVA,
not PCIe PASID/PRI, not CUDA UVA). No new syscall.

### OperatorInject hot-add

Soft-CP resident worker + versioned injectable ops (`memcpy` /
`saxpy` + hot-add third) without Soft-CP restart. SID still at
submit. Own bytecode / IR only — not NVRTC / CUDA.

Distinct from landed `OperatorKernelHandle` Hodge inject.

**Done when:** hot-add a third op without restart; SID-at-submit
unchanged; docs non-claims. No new syscall.

### Optional — MicroPerceptron consumer of frozen `IreeHalCmd`

Secondary to the landed PJRT/IREE shim. Consume the frozen packet
and/or path-A BAR. **Not** a second compiler story. **Not** a
plugin. Skip if it would fork the opcode table.

A thin doorbell client (`examples/accel-client`, crate
`aether-accel-client`) already proves a **second caller** of the same
96-byte image through `IreeShapedCp` (allocate, MatMul-shaped / Nop,
wait; Soft SMMU map + SID stamp required). That is a **research sketch**,
not this MicroPerceptron slice and not a new device model. MicroPerceptron
remains later and optional.

### Gated — guest PCI path A

A kernel `VirtioAccelMmio` that talks PCI BAR0 **only if** the
Soft-SMMU IOVA demo needs BAR DMA. Path B remains canonical. Do
not rebuild QEMU in CI. The QEMU device model already landed
(`qemu/aether_accel.c`).

### Site progress refreshes

OK as leave-behinds. **Not** milestones. PR #37 / #46 / #50 / #52
/ #58 stay progress refreshes.

## 2027 H1 — deepen isolation + compiler contract

Deepen what already shipped. Do not open a new ISA. Do not port
the kernel.

- **PJRT shim more ops / Event timeline polish**
  (`host/aether-pjrt`). Still not a PJRT plugin. Still not
  `GetPjRtApi` / `iree_hal_driver_t`. Still not in-kernel graph IR.
- **SoftSFI widen** — more memory side-effects on the toy ISA,
  still honest TODOs. Atomics / tensor / heap stay refused until
  they are modeled. Still not NVVM. Still not “safe multi-tenant
  kernels.”
- **Per-task `CapTable`** — **only if** two shim tenants alias
  slots on the shared World table. World still shares one table
  today. Isolate those tenants; do not invent a CNode.
- **`SYS_REVOKE`** — **only if** the same PR demos
  revoke → `unbind_stream` / FLR. Internal `revoke` / `revoke_in`
  already exist. Additive syscall only with that demo.
- **Blast-radius diligence clips** as needed (two-tenant refuse
  that earns a new line). Not a second ring-3 World.

Path B / Soft SMMU software / ABI 0–11 stay. No FLOPs.

## 2027 H2 — second consumers + ports if needed

Second consumers of the **frozen** packet. Ports stay hard-defer
unless path B’s doorbell is no longer enough.

- **MicroPerceptron / virtio-accel second consumer** on frozen
  `IreeHalCmd`. Same packet as M1/M2. Not a second compiler story.
  The doorbell client (`examples/accel-client`) is a research sketch
  of that shape; a full MicroPerceptron port stays later and optional.
- **RISC-V virtio-mmio SoftNPU *or* aarch64 GIC SoftNPU IRQ** —
  pick **one** if the path B doorbell (PLIC UART-THRE on RISC-V;
  CNTV/kthread drain on aarch64) is no longer enough. Hard defer
  otherwise. Do not schedule both. Neither port is a product-class
  second kernel.
- **Opcode table v2** — only with a **dual** update of
  [ACCEL.md](ACCEL.md) **and** `drivers/src/ireecp.rs` **and** host
  pack/unpack (`host/aether-pjrt`). A one-sided bump is a break.
  Research opcodes until 2028 H2 says otherwise.

## 2028 H1 — partner bring-up leave-behind

The silicon handoff is already in tree. Maintain it. Do not
invent a hardware SMMU milestone.

- **Soft SMMU bring-up kit stays the silicon handoff** (PR #48;
  [bringup/BRINGUP.md](bringup/BRINGUP.md) +
  `scripts/smmu_{dump,replay}.py`). Already landed; keep dump/replay
  honest against STE→CD→S1/S2 + ATS. Not a Soft-SMMU redo.
- **Hardware SMMU only with partner silicon** — program a real
  SID / PT walk on an IOMMU. That is **not** a software milestone
  and is **not** this calendar. QEMU does not emulate an SMMU for
  this path.
- **Diligence pack refresh** ([DILIGENCE.md](DILIGENCE.md) +
  [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md)) for a real design-win
  conversation. Still not a partnership announcement. Still not a
  booked bring-up.

## 2028 H2 — stop or partner

Either a **signed opcode list** replaces research `IreeHalCmd`, or
**freeze the research ABI** and stop inventing partners.

- Signed list: cited from a real partner CP; dual-update ACCEL.md
  + `ireecp` + host pack/unpack in the same PR. That is a partner
  event, not a Kernel invention.
- Freeze: `IreeHalCmd` v1 (or the v2 that already dual-updated)
  is the last research packet. No further opcode theater.
- **No manufacturing climax. No tape-out claim.** SpecForge Y2H2
  bring-up-as-climax stays killed.
- `PartnerNpuStub` stays a labeled no-op unless a signed list
  retires it.

## Kill forever as calendar

These are **not** Kernel clock items for 2026–2028. Some exist as
honest stubs; none get a half-year. Do not open calendar PRs for
them.

| Item | Why it stays killed |
| --- | --- |
| Fork-shaped clone | `SYS_CLONE` shares aspace; a new PML4/`fork` is POSIX theater |
| User `open`/`read` POSIX surface | Ramfs is kernel-internal; no `SYS_OPEN` / `SYS_READ` |
| Soft-SMMU redo | STE→CD→S1/S2 + ATS + bring-up kit already landed |
| CXL productization | `MemorySpace::CxlRegion` stays a typed place; no CXL.mem claim |
| ChipletFleet as milestone | n≤32 Fiedler + `ChipletTaskScope` stub; not Fleet marketing |
| Formal seL4-style caps / proofs | Small CDT landed; proofs are a non-claim |
| SMMUv3 emulator | Software tables only; partner silicon for hardware SMMU |
| UCIe PHY | Transport. SoftChipletSync is not a coherence protocol |
| `PartnerNpuStub` paint | No-op sketch; M1 already shipped a real packet |
| Site-as-milestone | PR #37 / #46 / #50 / #52 / #58 are leave-behinds; not a climax |

**Skip** (same list, said once): fork, POSIX open/read, Soft-SMMU
redo, CXL productization, ChipletFleet-as-milestone, formal seL4
proofs, SMMUv3 emulator, UCIe PHY, PartnerNpuStub paint,
site-as-milestone.

## Relationship to prior calendars

| File | Role |
| --- | --- |
| [YEAR2_PLAN.md](YEAR2_PLAN.md) | Historical Falsifier ACTIVE track through PR #37 + SpecForge appendix (aspirational) |
| [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) | Closed M1–M4 + SoftChipletSync record |
| [MONTH5_PLAN.md](MONTH5_PLAN.md) | Closed Month 5 record (SoftGreenCtx / SoftCmdFirewall / SoftCCT / SoftSFI) |
| **This file** | Kernel calendar Sep 2026 → Sep 2028 |
| [ROADMAP.md](ROADMAP.md) | Landed status, stubs, technical leftovers. Points here for what to sequence next |

Do not re-schedule Soft SMMU / Soft-CP / SMP / PML4 / `IreeShapedCp`
/ XQueue / SID-at-submit / SoftChipletSync / Month 5 digests 1–4.
Do not sequence against the SpecForge Y1H1–Y2H2 appendix.

[ROADMAP.md](ROADMAP.md) suggested-next-cuts that are not in H2 2026
or a later half-year below remain **technical leftovers**, not
calendar.

## Kernel PR order (this horizon)

1. This file + pointers from [ROADMAP.md](ROADMAP.md),
   [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md), and
   [YEAR2_PLAN.md](YEAR2_PLAN.md) — **this cut**
2. H2 2026 explorations (do not mark Done until they land):
   SoftNoI-IS (this PR), PASID/SVA, OperatorInject
3. Optional H2 2026: MicroPerceptron consumer; guest PCI path A
   **only if** Soft-SMMU IOVA needs BAR. Doorbell sketch
   (`examples/accel-client`) is a second `IreeHalCmd` caller, not
   MicroPerceptron.
4. 2027 H1 deepen (PJRT polish, SoftSFI widen, gated CapTable /
   `SYS_REVOKE`, blast-radius clips)
5. 2027 H2 second consumer; **one** port if path B doorbell fails;
   opcode v2 only with dual ACCEL.md + ireecp + host pack/unpack
6. 2028 H1 maintain bring-up kit; diligence refresh; hardware SMMU
   is not a software PR
7. 2028 H2 signed opcode list **or** freeze research ABI

Site refreshes may land beside any of the above. They are not
numbered.

### File touch map (when a slice is pulled)

| Slice | Primary touches |
| --- | --- |
| SoftNoI-IS | `core/src/noi.rs` + SoftChipletSync advertisement + `drivers/src/noi.rs` XQueue admit + class tag, host tests |
| PASID / SVA | `core/src/iommu.rs`, `drivers/src/fakecp.rs` |
| OperatorInject | `drivers/src/fakecp.rs` resident worker |
| MicroPerceptron | host crate or virtio-accel consumer of frozen `IreeHalCmd` (still later / optional; doorbell sketch is `examples/accel-client`) |
| Path-A guest bind | guest `VirtioAccelMmio` only; CI still does not rebuild QEMU |
| PJRT polish | `host/aether-pjrt`, [HOST.md](HOST.md) |
| SoftSFI widen | `core/src/softsfi.rs`, `drivers/src/softsfi.rs`; honest TODOs |
| Per-task CapTable | `core/src/caps.rs`, kernel World; `SYS_REVOKE` only with unbind/FLR demo |
| Blast-radius clip | `core/src/blast.rs` / host tests; extend, do not rebuild |
| Opcode table v2 | [ACCEL.md](ACCEL.md) **and** `drivers/src/ireecp.rs` **and** host pack/unpack |
| One port (if needed) | `kernel/src/arch/riscv64/` virtio-mmio **or** `kernel/src/arch/aarch64/` GIC IRQ — not both |
| Diligence refresh | [DILIGENCE.md](DILIGENCE.md), [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) |

Cross-cutting: this file, ROADMAP / SIX_MONTH_PLAN / YEAR2_PLAN
status pointers. CI only if a new host-test target appears. ABI
0–11 frozen. Path B canonical.

## What we will not claim

- That SoftNoI-IS, PASID/SVA, or OperatorInject are Done on this
  tip (SoftNoI-IS is in-flight / landing this PR)
- That PASID/SVA is ARM SVA, PCIe PASID/PRI, hardware ATS, or a
  CUDA unified virtual address space
- Zero-copy / unified VA without the unmap → SSID TLB invalidate
  path
- That OperatorInject is NVRTC, CUDA, or a vendor compiler
- That SoftNoI-IS synthesizes topology
- That the PJRT shim is a plugin, an IREE driver, or XLA
  integration
- That SoftSFI (widened or not) is a verified multi-tenant GPU
- That a 2027 port is a product-class second kernel
- That hardware SMMU, tape-out, or a manufacturing climax is a
  software milestone
- That a research `IreeHalCmd` is a signed vendor ISA
- That M1–M4, SoftChipletSync, the Soft SMMU kit, or Month 5
  digests became hardware
- Benchmarks, FLOPs, seL4 proofs, or a fake NVIDIA partnership
- An OS-completeness clock (fork, POSIX, CXL.mem, ChipletFleet,
  SMMUv3 emulator, UCIe PHY, site-as-milestone)
