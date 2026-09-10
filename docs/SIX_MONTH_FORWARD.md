# Six-month forward (Sep 2026 → Mar 2027)

Leave-behind for Kernel tracking. Filed 2026-09-10 after H2 2026
leftovers landed on main.

**This is the near-term Kernel calendar** for Sep 2026 → Mar 2027.
[TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) remains the Sep 2026 → Sep 2028
horizon. Closed records: [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md)
(M1–M4), [MONTH5_PLAN.md](MONTH5_PLAN.md) (four digests).
[SELL_GOALS.md](SELL_GOALS.md) is the partner-facing demo list for
this quarter. [YEAR2_PLAN.md](YEAR2_PLAN.md) stays the 2026-09-06
Falsifier ACTIVE track (done through PR #37) plus the SpecForge
appendix (aspirational only). Do not sequence new work against those
closed files.

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
- Sell milestones in this file are **demos and docs**, not an
  OS-completeness clock.

## Reality (update Done)

H2 2026 leftovers **largely landed**. Do **not** re-schedule any of
the following as new milestones. Do **not** rewrite the landed-PR
record in [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) or
[MONTH5_PLAN.md](MONTH5_PLAN.md).

| Landed | Honest reading |
| --- | --- |
| Year-1 + hardening | Soft SMMU, Soft-CP, SMP, PML4, CDT, three-ISA `/init`, path B/A — research slices, not a product kernel |
| **M1 IreeShapedCp (PR #38)** | `backend = 4`, frozen `IreeHalCmd`; research opcodes, not a signed vendor |
| **M2 PJRT/IREE shim (PR #41)** | `host/aether-pjrt` packs `IreeHalCmd` → `IreeShapedCp`. Not `GetPjRtApi` |
| **M3 SID-at-submit (PR #49)** | Host1x-shaped SET_SID; SID sticks on the XQueue. Not a Tegra driver |
| **M4 XQueue (PR #47)** | Two software queues; queue-boundary suspend/resume. Not silicon, not XSched LD_PRELOAD |
| **SoftChipletSync (PR #51)** | Scoped `{wave, CU, chiplet, package}`. Fence **counts** only. Not UCIe, not Vulkan |
| Soft SMMU bring-up kit (PR #48) | Dump/replay of STE→CD→S1/S2 + ATS. Not a Soft-SMMU redo |
| Month 5 digests 1–4 | SoftGreenCtx, SoftCmdFirewall, SoftCCT, SoftSFI. Not one pillar |
| **SoftNoI-IS (PR #60)** | Fake shared NoI; solo vs concurrent → IS; refuse `IS > 1.5`. Admit control, not topology synth |
| **OperatorInject (PR #61)** | Soft-CP resident worker + versioned memcpy/saxpy + hot-add scale. Not NVRTC/CUDA |
| **PASID / SVA (PR #62)** | Bind mm↔SSID; Soft-CP DMA via process VA; unmap→SSID TLB; stale ATC fault. Not ARM SVA, not CUDA UVA |
| Sell set (PRs #65–#72, #77) | Diligence / red-team / partner-hello / DESIGN_WIN / Week 1 call pack. Live. Not a partnership |
| **SoftSFI `ATOMIC_ADD` (PR #73)** | SID-proved toy fetch-add (in-range accept, cross-tenant `Oob`). Sequential RMW, not a hardware atomic. Tensor stays `Unmodeled` |
| **SoftSFI heap refuse (PR #80)** | Named `SoftOp::Heap` → `SfiError::Unmodeled`. `[softsfi] heap=refused`. Not a bump allocator. Tensor stays `Unmodeled` |
| **Event create/record/wait (PR #74)** | `host/aether-pjrt` Event on existing SoftChipletSync fences. Not `GetPjRtApi`, not XLA |
| **Fabric-class SoftNoI (PR #75)** | Software `FlowClass` tag at submit (Curl ring reserve). Not a vendor header |
| **Path-A IOVA (PR #78)** | Soft-SMMU IOVA on path-A BAR DMA; wrong SID aborts. Host proof. Kernel PCI BAR0 bind still optional |
| Site through PR #79 | Research leave-behind / progress refresh. **Not** a calendar item |
| **MP-shaped thin consumer (PR #83)** | `host/aether-mp-shim`. Same frozen 96-byte `IreeHalCmd`. memcpy / matmul / wave. Inspiration name only. Secondary to PJRT. Not a port |
| **PJRT Add/Relu more ops (PR #84)** | SoftNPU `Add=3` / `Relu=4` through `aether-pjrt` as IREE HAL `function` 2 / 3. Same frozen `IreeHalCmd`. Offsets / magic / executable / TRANSFER reserved stay. Not a second IR. Not `GetPjRtApi` |

SoftNPU path-B opcodes stay Aether-native. `PartnerNpuStub`
(`backend = 2`) stays a labeled no-op. Path B is canonical. Soft
SMMU is software. Frozen `IreeHalCmd`. No fake NVIDIA / FLOPs /
tape-out.

## Execution spine (this is the calendar)

```text
M0 now (Sep)     sell pack + call pack live; capture feedback; DESIGN_WIN from real tables
M1–M2 (Oct–Nov)  Done — heap refuse #80, MP consumer #83, PJRT Add/Relu #84
M3–M4 (Dec–Jan)  SoftGreenCtx diligence leave-behind; SoftCCT/Event fence-count polish; CapTable iff alias
M5–M6 (Feb–Mar)  opcode v2 or ABI-freeze checkpoint; diligence refresh; one port iff path B doorbell fails
```

2028 handoff language does **not** move: a signed opcode list from a
real partner CP, or freeze the research ABI. That decision is named
at M5–M6 as a **checkpoint**, not as tape-out.

### M0 — now (Sep 2026)

Sell pack + call pack are **live**. Capture partner feedback. Fill
[DESIGN_WIN.md](DESIGN_WIN.md) from **real** opcode tables when they
appear. Do not invent a vendor table in this tree.

**Done when:**

1. [SELL_GOALS.md](SELL_GOALS.md) names the runnable demos and the
   ask. Week 1 pack ([WEEK1_CALL.md](WEEK1_CALL.md)) and 8-minute
   script ([PITCH.md](PITCH.md)) stay the call surfaces.
2. Partner feedback is written down (filled DESIGN_WIN, or a written
   no with reasons). The IREE HAL stand-in
   ([design-win/iree-hal-standin.md](design-win/iree-hal-standin.md))
   is **not** a partner table.
3. No new kernel feature required for this month. Path B / Soft SMMU
   software / frozen `IreeHalCmd` unchanged.

### M1–M2 — Oct–Nov 2026

All three slices **landed**. None forked the opcode table. SoftSFI
heap refuse is **Done** (PR #80). The MicroPerceptron-shaped thin
consumer is **Done** (PR #83). PJRT Add/Relu more ops are **Done**
(PR #84) — do not re-schedule any of them.

1. **MicroPerceptron-shaped thin consumer** (**landed**, PR #83).
   `host/aether-mp-shim` submits the same frozen 96-byte `IreeHalCmd`
   through `IreeShapedCp` or Soft-CP (SoftCmdFirewall still applies).
   Opcode surface: memcpy (host copy; TRANSFER reserved) / matmul /
   wave. Same refuse rules (Soft SMMU map + SID stamp; unknown
   executable). Inspiration name only. Secondary to PJRT. **Not** a
   second compiler story. **Not** a plugin. **Not** a port. Full
   virtio-accel interop stays later. Doorbell sketch stays
   `examples/accel-client`.
2. **SoftSFI heap refuse** (**landed**, PR #80). Named `SoftOp::Heap`
   → `SfiError::Unmodeled`. `[softsfi] heap=refused`. Not a bump
   allocator. Tensor stays `Unmodeled`. Still not NVVM. Still not
   “safe multi-tenant kernels.”
3. **PJRT shim more ops** (**landed**, PR #84). `Add` / `Relu` pack
   into the same frozen `IreeHalCmd` (`function` 2 / 3). SoftNPU
   AccelOp table is additive (`Add=3`, `Relu=4`). Not a second IR.
   Offsets / magic `0xAE7E1EE1` / executable `0x0001EE00` / TRANSFER
   reserved stay. Still not `GetPjRtApi`. Further packet change is
   still a **dual** update of [ACCEL.md](ACCEL.md) **and**
   `drivers/src/ireecp.rs` **and** host pack/unpack in the same PR.

**Done when:** met. Heap refuse (PR #80), thin MP consumer (PR #83),
PJRT Add/Relu (PR #84). No new syscall. Path B / `make qemu`
unchanged. `IreeHalCmd` offsets unchanged.

### M3–M4 — Dec 2026 – Jan 2027

Deepen what already shipped. Do not open a new ISA.

1. **SoftGreenCtx interference leave-behind** for diligence. The
   70/30 SM/WQ partition already landed. This cut is a partner-readable
   clip (host stdout + [DILIGENCE.md](DILIGENCE.md) line), not HW MIG,
   not FLOPs, not a BAR firewall.
2. **SoftCCT / Event polish** for multi-chiplet **fence counts**.
   Package-scope ≪ broadcast stays the proof. Single-die QEMU / host
   numbers are still not partner latency proof. Event create / record /
   wait already sits on SoftChipletSync; polish is more honest counts,
   not a new IR.
3. **Per-task `CapTable` — only if** two shim tenants alias slots on
   the shared World table. World still shares one table today.
   Isolate those tenants. Do not invent a CNode. Additive
   `SYS_REVOKE` only if the same PR demos revoke → `unbind_stream` /
   FLR.

**Done when:** diligence names the GreenCtx clip; CCT/Event host tests
still measure fence **counts**; CapTable is either skipped (no alias)
or isolates the two tenants. No new syscall unless the CapTable gate
fires with the revoke→FLR demo.

### M5–M6 — Feb–Mar 2027

Decision checkpoint. Not a foundry date.

1. **Partner opcode table v2 or ABI freeze checkpoint.** Either a
   filled [DESIGN_WIN.md](DESIGN_WIN.md) from a real table forces a
   dual-update v2, or we write down “research `IreeHalCmd` v1 stays.”
   2028 handoff language is unchanged: signed opcode list **or**
   freeze the research ABI. This month **names** that fork. It does
   not tape out.
2. **Diligence pack refresh** ([DILIGENCE.md](DILIGENCE.md) +
   [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) + [SELL_GOALS.md](SELL_GOALS.md)).
   Still not a partnership announcement. Still not a booked bring-up.
3. **One port** — RISC-V virtio-mmio **or** aarch64 GIC SoftNPU IRQ —
   **only if** the path B doorbell (PLIC UART-THRE on RISC-V;
   CNTV/kthread drain on aarch64) fails a partner ask. Hard defer
   otherwise. Do not schedule both. Neither port is a product-class
   second kernel.

**Done when:** the checkpoint is written (v2 dual-update **or**
explicit freeze-v1); diligence pack matches the demos we can still
run; the port is either skipped or one doorbell path that a partner
actually asked for.

## Kill forever as calendar

These are **not** Kernel clock items for this half-year. Some exist
as honest stubs; none get an M-number. Do not open calendar PRs for
them.

| Item | Why it stays killed |
| --- | --- |
| Fork-shaped clone | `SYS_CLONE` shares aspace; a new PML4/`fork` is POSIX theater |
| User `open`/`read` POSIX surface | Ramfs is kernel-internal; no `SYS_OPEN` / `SYS_READ` |
| Soft-SMMU redo | STE→CD→S1/S2 + ATS + bring-up kit + path-A IOVA already landed |
| CXL productization | `MemorySpace::CxlRegion` stays a typed place; no CXL.mem claim |
| ChipletFleet as calendar | n≤32 Fiedler + `ChipletTaskScope` stub; not Fleet marketing |
| Formal seL4-style caps / proofs | Small CDT landed; proofs are a non-claim |
| SMMUv3 emulator | Software tables only; partner silicon for hardware SMMU |
| UCIe PHY | Transport. SoftChipletSync is not a coherence protocol |
| `PartnerNpuStub` paint | No-op sketch; M1 already shipped a real packet |
| Site-as-milestone | PR #37 / #46 / #50 / #52 / #58 / #76 / #79 are leave-behinds |
| FLOPs / tape-out | Standing non-claim. 2028 is an opcode list or an ABI freeze |

**Skip** (same list, said once): fork, POSIX open/read, Soft-SMMU
redo, CXL productization, ChipletFleet calendar, formal seL4,
SMMUv3 emu, UCIe PHY, PartnerNpuStub paint, site-as-milestone,
FLOPs/tape-out.

## Relationship to prior calendars

| File | Role |
| --- | --- |
| [YEAR2_PLAN.md](YEAR2_PLAN.md) | Historical Falsifier ACTIVE track through PR #37 + SpecForge appendix (aspirational) |
| [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) | Closed M1–M4 + SoftChipletSync record |
| [MONTH5_PLAN.md](MONTH5_PLAN.md) | Closed Month 5 record (SoftGreenCtx / SoftCmdFirewall / SoftCCT / SoftSFI) |
| [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) | Horizon Sep 2026 → Sep 2028. H2 2026 leftovers **landed** |
| **This file** | Near-term calendar Sep 2026 → Mar 2027 |
| [SELL_GOALS.md](SELL_GOALS.md) | Partner-facing demos / ask / non-claims for this quarter |
| [ROADMAP.md](ROADMAP.md) | Landed status, stubs, technical leftovers. Points here for what to sequence next |

Do not re-schedule Soft SMMU / Soft-CP / SMP / PML4 / `IreeShapedCp`
/ XQueue / SID-at-submit / SoftChipletSync / Month 5 digests 1–4 /
H2 2026 leftovers / Event / fabric-class / `ATOMIC_ADD` / SoftSFI
heap refuse / path-A IOVA / the sell set / MP-shaped thin consumer
(PR #83) / PJRT Add/Relu more ops (PR #84). Do not sequence against the SpecForge
Y1H1–Y2H2 appendix.

[ROADMAP.md](ROADMAP.md) suggested-next-cuts that are not in M0–M6
below remain **technical leftovers**, not calendar.

## Kernel PR order (this horizon)

1. This file + [SELL_GOALS.md](SELL_GOALS.md) + pointers from
   [ROADMAP.md](ROADMAP.md) and [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md)
   — **this cut**
2. M0 ongoing: capture feedback; fill DESIGN_WIN from a real table
   when one appears (no kernel PR required)
3. M1–M2 — **landed:** SoftSFI heap refuse (PR #80), thin MP
   consumer (PR #83), PJRT Add/Relu (PR #84). Do not re-schedule.
   Further packet change is still a dual ACCEL.md + `ireecp` + host
   pack/unpack
4. M3–M4: SoftGreenCtx diligence leave-behind; SoftCCT/Event
   fence-count polish; CapTable **only if** two shim tenants alias
   slots
5. M5–M6: opcode v2 **or** freeze-v1 checkpoint; diligence refresh;
   **one** port **only if** path B doorbell fails a partner ask

Site refreshes may land beside any of the above. They are not
numbered.

### File touch map (when a slice is pulled)

| Slice | Primary touches |
| --- | --- |
| Thin MP consumer (**landed**, PR #83) | `host/aether-mp-shim`; frozen `IreeHalCmd`; doorbell or Soft-CP. Doorbell sketch stays `examples/accel-client` |
| SoftSFI heap refuse (**landed**, PR #80) | `core/src/softsfi.rs`, `drivers/src/softsfi.rs`; `SoftOp::Heap` → `Unmodeled`; `[softsfi] heap=refused` |
| PJRT more ops (**landed**, PR #84) | `Add` / `Relu` on frozen `IreeHalCmd` (`function` 2 / 3). `host/aether-pjrt`, [HOST.md](HOST.md), [ACCEL.md](ACCEL.md), `drivers/src/ireecp.rs`. Offsets unchanged |
| SoftGreenCtx leave-behind | [DILIGENCE.md](DILIGENCE.md), `examples/diligence-demo/`, host tests already in `core/src/greenctx.rs` |
| SoftCCT / Event polish | `core/src/chipsync.rs`, `host/aether-pjrt`, [HOST.md](HOST.md) |
| Per-task CapTable (gated) | `core/src/caps.rs`, kernel World; `SYS_REVOKE` only with unbind/FLR demo |
| Opcode v2 / freeze checkpoint | [ACCEL.md](ACCEL.md) **and** `drivers/src/ireecp.rs` **and** host pack/unpack **or** a written freeze-v1 note in this file |
| Diligence refresh | [DILIGENCE.md](DILIGENCE.md), [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md), [SELL_GOALS.md](SELL_GOALS.md) |
| One port (if needed) | `kernel/src/arch/riscv64/` virtio-mmio **or** `kernel/src/arch/aarch64/` GIC IRQ — not both |

Cross-cutting: this file, ROADMAP / TWO_YEAR_PLAN status pointers,
[SELL_GOALS.md](SELL_GOALS.md). CI only if a new host-test target
appears. ABI 0–11 frozen. Path B canonical.

## What we will not claim

- That a filled DESIGN_WIN, a Week 1 call, or [SELL_GOALS.md](SELL_GOALS.md)
  is a signed vendor, a partnership, or a design win in hand
- That the PJRT shim is a plugin, an IREE driver, or XLA integration.
  `Add` / `Relu` (PR #84) are additive research ops on the frozen
  packet, not FLOPs and not a second IR
- That a MicroPerceptron-shaped / virtio-accel consumer is a second
  compiler story or a silicon virtio device
- That SoftSFI (widened or not) is a verified multi-tenant GPU
- That SoftGreenCtx is HW MIG, a BAR firewall, or a FLOP / partner
  bandwidth result
- That SoftCCT / Event polish is a multi-chiplet latency result from
  single-die host tests, a Vulkan timeline, or UCIe
- That a 2027 port is a product-class second kernel
- That hardware SMMU, tape-out, or a manufacturing climax is a
  software milestone
- That a research `IreeHalCmd` is a signed vendor ISA
- That M1–M4, SoftChipletSync, Month 5 digests, or H2 2026 leftovers
  became hardware
- Benchmarks, FLOPs, seL4 proofs, or a fake NVIDIA partnership
- An OS-completeness clock (fork, POSIX, CXL.mem, ChipletFleet,
  SMMUv3 emulator, UCIe PHY, site-as-milestone)
