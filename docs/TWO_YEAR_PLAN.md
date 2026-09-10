# Two-year plan (Sep 2026 → Sep 2028)

Leave-behind for Kernel tracking. Filed 2026-09-08 after Year-1 +
hardening, closed M1–M4, SoftChipletSync, the Soft SMMU bring-up kit,
and Month 5 digests 1–4 landed on main. Status pointer refreshed
2026-09-10: H2 2026 leftovers **landed**.

**This is the Kernel horizon** for Sep 2026 → Sep 2028.
**What to sequence next:** [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md)
(Sep 2026 → Mar 2027). Partner-facing demos:
[SELL_GOALS.md](SELL_GOALS.md). [MONTH5_PLAN.md](MONTH5_PLAN.md) is
the closed Month 5 record (four digests landed).
[SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) is the closed M1–M4 record.
[YEAR2_PLAN.md](YEAR2_PLAN.md) stays the 2026-09-06 Falsifier ACTIVE
track (done through PR #37) plus the SpecForge appendix (aspirational
only). Do not sequence new work against those closed files.

Research prototype. Path B is canonical. Soft SMMU is software.
Frozen `IreeHalCmd`. No fake NVIDIA partnership, no FLOP benchmarks,
no tape-out claim.

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

## Reality on main (through 2026-09-10)

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
| **SoftSFI (PR #56)** | Toy Soft-CP load/store/add/dma + SFI verifier. Later: SID-proved `atomic_add`. Heap/alloc named refuse. Not NVVM. Tensor `Unmodeled` |
| **SoftCCT (PR #57)** | Last-writer chiplet elision on SoftChipletSync. Not a coherence protocol |
| **SoftNoI-IS (PR #60)** | Fake shared NoI; refuse `IS > 1.5`. Admit control, not topology synth |
| **OperatorInject (PR #61)** | Resident worker + hot-add scale. Not NVRTC/CUDA |
| **PASID / SVA (PR #62)** | Bind mm↔SSID; unmap→SSID TLB; stale ATC fault. Not ARM SVA, not CUDA UVA |
| Sell set (PRs #65–#72, #77) | Diligence / red-team / partner-hello / DESIGN_WIN / Week 1 call pack. Live |
| **SoftSFI `ATOMIC_ADD` (PR #73)** | SID-proved toy fetch-add. Sequential RMW, not a hardware atomic. Tensor stays `Unmodeled` |
| **SoftSFI heap refuse (PR #80)** | Named `SoftOp::Heap` → `SfiError::Unmodeled`. `[softsfi] heap=refused`. Not a bump allocator |
| **Event create/record/wait (PR #74)** | PJRT Event on existing SoftChipletSync fences. Not `GetPjRtApi` |
| **Fabric-class SoftNoI (PR #75)** | Software `FlowClass` tag (Curl ring reserve). Not a vendor header |
| **Path-A IOVA (PR #78)** | Soft-SMMU IOVA on path-A BAR DMA; wrong SID aborts. Kernel PCI bind still optional |
| **MP-shaped thin consumer (PR #83)** | `host/aether-mp-shim`. Same frozen `IreeHalCmd`. Inspiration name only. Secondary to PJRT. Not a port |
| **PJRT Add/Relu more ops (PR #84)** | SoftNPU `Add=3` / `Relu=4` through `aether-pjrt` as IREE HAL `function` 2 / 3. Same frozen packet. Offsets unchanged |
| Explorations A–E | A merged into M2; B blast-radius clip; C `ChipletTaskScope` stub; D CDT props; E `TypedWindow` stub |
| Site through PR #79 | Research leave-behind / progress refresh. **Not** a calendar item |

SoftNPU path-B opcodes stay Aether-native. `PartnerNpuStub`
(`backend = 2`) stays a labeled no-op.

H2 2026 leftovers are **Done** (SoftNoI #60, PASID #62,
OperatorInject #61, Event #74, fabric-class #75, SoftSFI
`ATOMIC_ADD` #73, SoftSFI heap refuse #80, path-A IOVA #78, sell
set #65–#72 / #77). Thin MP-shaped consumer **landed** (PR #83).
PJRT Add/Relu more ops **landed** (PR #84).
Do not re-schedule them. Near-term sequencing is
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md).

## H2 2026 (closed) — partner-HAL leftovers **landed**

The parked partner-HAL leftovers landed as **explorations**, not as
M5–M8 numbers and not as a second half-year of
[MONTH5_PLAN.md](MONTH5_PLAN.md). Do not re-open this half-year.

```text
SoftNoI-IS (PR #60)  →  OperatorInject (PR #61)  →  PASID/SVA (PR #62)
Event (PR #74)  ·  fabric-class (PR #75)  ·  ATOMIC_ADD (PR #73)  ·  path-A IOVA (PR #78)
sell set (#65–#72, #77) live
optional leftover: MicroPerceptron consumer of frozen IreeHalCmd — **landed** as thin sketch (PR #83; `host/aether-mp-shim`)
gated leftover: Guest PCI path A  — Soft-SMMU IOVA host proof landed; kernel BAR0 bind still optional
site progress refreshes — not milestones
```

### SoftNoI-IS admit (**landed**, PR #60 + fabric-class #75)

Interference Score admit policy (PARL / NoI inspiration).
SoftChipletSync fabric IS estimate; refuse when `IS > budget`.
Slice: solo vs concurrent → IS; refuse `IS > 1.5` (or the named
budget). **Admit control, not topology synthesis.** IS path landed
in PR #60. Software fabric-class tag (`AccelJobDesc.flow`) landed
in PR #75: Curl needs reserved ring capacity; class changes
admit/refuse. Not a vendor header. Not FlowHodgeQuota DMA-header
theater.

**Done when (met):** host tests show solo vs concurrent IS; refuse
`IS > budget`; class tag changes admit (Curl ring); docs name PARL /
NoI as inspiration only. No FLOPs. No “optimal NoI.” No new
syscall. Path B / `make qemu` unchanged.

### PASID / SVA software bind + invalidate (**landed**, PR #62)

Per-`AccelDevice` PASID space. Bind process VA ↔ Soft-SMMU SSID;
Soft-CP DMA uses VA; host unmap → SSID TLB invalidate; stale
translate faults. Software only.

**Overclaim watch:** no “zero-copy SVA” / “unified VA” without the
unmap → invalidate path. That pairing landed in the same PR.

**Done when (met):** bind mm↔ssid; Soft-CP DMA via VA; unmap→SSID TLB
invalidate; stale translate faults; docs non-claims (not ARM SVA,
not PCIe PASID/PRI, not CUDA UVA). No new syscall.

### OperatorInject hot-add (**landed**, PR #61)

Soft-CP resident worker + versioned injectable ops (`memcpy` /
`saxpy` + hot-add third) without Soft-CP restart. SID still at
submit. Own bytecode / IR only — not NVRTC / CUDA.

Distinct from landed `OperatorKernelHandle` Hodge inject.

**Done when (met):** hot-add a third op without restart; SID-at-submit
unchanged; docs non-claims. No new syscall.

### Optional — MicroPerceptron consumer of frozen `IreeHalCmd`

**Landed** as PR #83 (`host/aether-mp-shim`). Recorded on
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) **M1–M2**.
Secondary to the landed PJRT/IREE shim. Consume the frozen packet
and/or path-A BAR. **Not** a second compiler story. **Not** a
plugin. Skip if it would fork the opcode table.

**Thin research sketch landed:** `host/aether-mp-shim` (`aether-mp-shim`)
submits the same frozen 96-byte `IreeHalCmd` through `IreeShapedCp`
(doorbell) or Soft-CP (SoftCmdFirewall still applies). Opcode surface
is memcpy (host copy; v1 `TRANSFER` stays reserved) / matmul / wave.
Bad executable and unbound SID are refused. MicroPerceptron is an
**inspiration name only** — not a port, not a vendor.

A thin doorbell client (`examples/accel-client`, crate
`aether-accel-client`) already proves a **second caller** of the same
image. Thin MP-shaped consumer **landed** as PR #83
(`host/aether-mp-shim`). A full MicroPerceptron / virtio-accel port
stays later and optional. Do not fork the opcode table.

### Gated — guest PCI path A

**Host IOVA proof landed** (PR #78; `drivers/src/path_a.rs`,
`make accel-test` / `make qemu-accel`). The path-A job wire carries
Soft-SMMU IOVAs; DMA walks ssid 4; wrong SID aborts. A kernel
`VirtioAccelMmio` that talks PCI BAR0 is **not** required for that
digest. Path B remains canonical. Do not rebuild QEMU in CI. The
QEMU device model already landed (`qemu/aether_accel.c`).

### Site progress refreshes

OK as leave-behinds. **Not** milestones. PR #37 / #46 / #50 / #52
/ #58 / #76 / #79 stay progress refreshes.

## 2027 H1 — deepen isolation + compiler contract

Near-term sequencing for Sep 2026 → Mar 2027 lives in
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md). Do not open a new ISA.
Do not port the kernel.

- **PJRT Event timeline polish** (**landed**, PR #74)
  (`host/aether-pjrt` Event create / record / wait on the existing
  CP-shaped `Timeline` and SoftChipletSync chiplet/package scopes).
  Still not a PJRT plugin. Still not `GetPjRtApi` / XLA /
  `iree_hal_driver_t`. Still not in-kernel graph IR. `IreeHalCmd`
  offsets frozen; TRANSFER stays reserved.
- **PJRT shim more ops** (**landed**, PR #84) — `Add` / `Relu`
  pack into the same frozen `IreeHalCmd` (`function` 2 / 3). SoftNPU
  AccelOp table is additive (`Add=3`, `Relu=4`). Not a second IR.
  Offsets / magic `0xAE7E1EE1` / executable `0x0001EE00` / TRANSFER
  reserved / `workgroup_count` as shape (not tiles) stay. Still not
  `GetPjRtApi`. Path B unchanged.
- **SoftSFI widen** — `atomic_add` **landed** (PR #73; SID
  `base+bound`; in-range accept, cross-tenant `Oob`). Sequential toy
  RMW, not a hardware atomic. Tensor stays `Unmodeled`. Heap/alloc
  named refuse **landed** (PR #80; `SoftOp::Heap` →
  `SfiError::Unmodeled`; `[softsfi] heap=refused`) — not a bump
  allocator. Still not NVVM. Still not “safe multi-tenant
  kernels.”
- **Per-task `CapTable`** — **only if** two shim tenants alias
  slots on the shared World table. World still shares one table
  today. Isolate those tenants; do not invent a CNode. Forward
  M3–M4 gate.
- **`SYS_REVOKE`** — **only if** the same PR demos
  revoke → `unbind_stream` / FLR. Internal `revoke` / `revoke_in`
  already exist. Additive syscall only with that demo.
- **Blast-radius diligence clips** as needed (two-tenant refuse
  that earns a new line). SoftGreenCtx interference leave-behind is
  forward M3–M4. Not a second ring-3 World.

Path B / Soft SMMU software / ABI 0–11 stay. No FLOPs.

## 2027 H2 — second consumers + ports if needed

Pulled forward where the partner ask is already live. See
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) M1–M2 (**landed:** heap
#80, MP #83, PJRT Add/Relu #84) and M5–M6 (opcode v2 **or**
freeze checkpoint; **one**
port **only if** path B’s doorbell fails a partner ask).

- **MicroPerceptron / virtio-accel second consumer** on frozen
  `IreeHalCmd`. Same packet as M1/M2. Not a second compiler story.
  Thin research sketch **landed** as PR #83 (`host/aether-mp-shim`;
  inspiration name only; secondary to PJRT). The doorbell client
  (`examples/accel-client`) is another caller of that image. A full
  MicroPerceptron port stays later and optional.
- **RISC-V virtio-mmio SoftNPU *or* aarch64 GIC SoftNPU IRQ** —
  pick **one** if the path B doorbell (PLIC UART-THRE on RISC-V;
  CNTV/kthread drain on aarch64) fails a partner ask. Hard defer
  otherwise. Do not schedule both. Neither port is a product-class
  second kernel. Forward M5–M6 gate.
- **Opcode table v2** — only with a **dual** update of
  [ACCEL.md](ACCEL.md) **and** `drivers/src/ireecp.rs` **and** host
  pack/unpack (`host/aether-pjrt`). A one-sided bump is a break.
  Research opcodes until 2028 H2 says otherwise. M5–M6 is the
  **checkpoint** (v2 or freeze-v1), not tape-out.

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
  [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) +
  [SELL_GOALS.md](SELL_GOALS.md)) for a real design-win
  conversation. First refresh is forward M5–M6. Still not a
  partnership announcement. Still not a booked bring-up.

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
| Site-as-milestone | PR #37 / #46 / #50 / #52 / #58 / #76 / #79 are leave-behinds; not a climax |

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
| [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) | **Near-term calendar** Sep 2026 → Mar 2027 |
| [SELL_GOALS.md](SELL_GOALS.md) | Partner-facing demos / ask / non-claims for this quarter |
| **This file** | Kernel horizon Sep 2026 → Sep 2028. H2 2026 leftovers **landed** |
| [ROADMAP.md](ROADMAP.md) | Landed status, stubs, technical leftovers. Points at SIX_MONTH_FORWARD for what to sequence next |

Do not re-schedule Soft SMMU / Soft-CP / SMP / PML4 / `IreeShapedCp`
/ XQueue / SID-at-submit / SoftChipletSync / Month 5 digests 1–4.
Do not sequence against the SpecForge Y1H1–Y2H2 appendix.

[ROADMAP.md](ROADMAP.md) suggested-next-cuts that are not in
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) M0–M6 or a later
half-year below remain **technical leftovers**, not calendar.

## Kernel PR order (this horizon)

1. This file + pointers from [ROADMAP.md](ROADMAP.md),
   [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md), and
   [YEAR2_PLAN.md](YEAR2_PLAN.md) — **landed**
2. H2 2026 explorations — **landed:** SoftNoI-IS (PR #60),
   OperatorInject (PR #61), PASID/SVA (PR #62), Event (PR #74),
   fabric-class (PR #75), SoftSFI `ATOMIC_ADD` (PR #73), SoftSFI
   heap refuse (PR #80), path-A IOVA (PR #78), sell set (#65–#72,
   #77)
3. Thin MP-shaped `IreeHalCmd` consumer — **landed** (PR #83;
   `host/aether-mp-shim`). PJRT Add/Relu more ops — **landed**
   (PR #84). Near-term remainder (Sep 2026 → Mar 2027) is
   [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) +
   [SELL_GOALS.md](SELL_GOALS.md)
4. 2027 H1 leftovers not pulled forward: gated `SYS_REVOKE` with
   unbind/FLR demo; blast-radius clips as needed
5. 2027 H2 remainder: anything M5–M6 did not take; **one** port
   only if path B doorbell fails; opcode v2 only with dual
   ACCEL.md + ireecp + host pack/unpack
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
| MicroPerceptron | thin sketch **landed** (PR #83; `host/aether-mp-shim`; inspiration name only; secondary to PJRT). Full virtio-accel port still later / optional. Doorbell sketch is `examples/accel-client` |
| Path-A guest bind | host `PathABar` IOVA / wrong-SID **landed**; kernel PCI bind still optional. CI still does not rebuild QEMU |
| PJRT polish | `host/aether-pjrt`, [HOST.md](HOST.md) |
| PJRT more ops | **landed** (PR #84; `Add` / `Relu`). SoftNPU `AccelOp` + `ireecp` function map + `host/aether-pjrt`; dual [ACCEL.md](ACCEL.md) / [HOST.md](HOST.md). Offsets frozen |
| SoftSFI widen | `core/src/softsfi.rs`, `drivers/src/softsfi.rs`; `atomic_add` modeled (PR #73); tensor `Unmodeled`; heap named refuse **landed** (PR #80) |
| Per-task CapTable | `core/src/caps.rs`, kernel World; `SYS_REVOKE` only with unbind/FLR demo |
| Blast-radius clip | `core/src/blast.rs` / host tests; extend, do not rebuild |
| Opcode table v2 | [ACCEL.md](ACCEL.md) **and** `drivers/src/ireecp.rs` **and** host pack/unpack |
| One port (if needed) | `kernel/src/arch/riscv64/` virtio-mmio **or** `kernel/src/arch/aarch64/` GIC IRQ — not both |
| Diligence refresh | [DILIGENCE.md](DILIGENCE.md), [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) |

Cross-cutting: this file, [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md),
[SELL_GOALS.md](SELL_GOALS.md), ROADMAP / SIX_MONTH_PLAN / YEAR2_PLAN
status pointers. CI only if a new host-test target appears. ABI
0–11 frozen. Path B canonical.

## What we will not claim

- That SoftNoI-IS, PASID/SVA, or OperatorInject became hardware,
  ARM SVA / PCIe PASID/PRI, NVRTC, or topology synthesis
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
