# Six-month forward (Sep 2026 → Mar 2027)

Leave-behind for Kernel tracking. Filed 2026-09-10 after H2 2026
leftovers landed on main.

**This is the closed Sep 2026 → Mar 2027 Kernel record** (M0–M6
landed). Do not re-schedule it. **Next-12-month industry calendar:**
[YEAR_AHEAD.md](YEAR_AHEAD.md) (Sep 2026 → Sep 2027).
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
| **SoftGreenCtx interference clip (M3)** | Diligence `[greenctx] interference partitioned 70/30 vs unpartitioned` with integer `bw_milli` / `interference_milli`. Existing 70/30 needle stays. Not HW MIG, not FLOPs, not a BAR firewall |
| **SoftCCT / Event fence counts (M4)** | Diligence `[softcct] package fences=` (1 vs 10) + `[event] fence counts chiplet-local vs package`. Host tests measure counts. Not latency, not Vulkan, not UCIe |
| **CapTable (M3–M4 gate)** | **Skipped.** PJRT / MP shims do not mint World `CPtr` slots. No alias. No `SYS_REVOKE`. Gate still closed |
| **freeze-v1 checkpoint (M5–M6)** | Research `IreeHalCmd` v1 stays. No real partner table in-tree. Do not invent a vendor. Port **skipped**. Diligence pack refreshed. Not tape-out |

SoftNPU path-B opcodes stay Aether-native. `PartnerNpuStub`
(`backend = 2`) stays a labeled no-op. Path B is canonical. Soft
SMMU is software. Frozen `IreeHalCmd`. No fake NVIDIA / FLOPs /
tape-out.

## Execution spine (this is the calendar)

```text
M0 now (Sep)     sell pack + call pack live; capture feedback; DESIGN_WIN from real tables
M1–M2 (Oct–Nov)  Done — heap refuse #80, MP consumer #83, PJRT Add/Relu #84
M3–M4 (Dec–Jan)  Done — GreenCtx interference clip + SoftCCT/Event fence counts; CapTable skipped (no alias)
M5–M6 (Feb–Mar)  Done — freeze-v1 (research IreeHalCmd v1 stays); diligence refresh; port skipped
```

2028 handoff language does **not** move: a signed opcode list from a
real partner CP, or freeze the research ABI. M5–M6 **named** that
fork (freeze-v1). It is not tape-out.

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

The two host clips **landed**. CapTable **skipped** (gate still
closed). Do not open a new ISA. Do not invent a CNode.

1. **SoftGreenCtx interference leave-behind** (**Done**, PR #85).
   The 70/30 SM/WQ partition already landed. Diligence now prints a
   stable `[greenctx] interference partitioned 70/30 vs unpartitioned`
   line with integer `bw_milli` / `interference_milli` from
   `MemcpyReport` / `GreenCtxReport`. Existing
   `[greenctx] SM/WQ pool split 70/30` needle stays. Not HW MIG, not
   FLOPs, not a BAR firewall. Residual shared-HBM tax stays.
2. **SoftCCT / Event polish** (**Done**, PR #85). Package-scope ≪
   broadcast stays the proof (`cct=1` vs `broadcast=10` on the
   multi-chiplet clip; single-chiplet is a no-op). Diligence prints
   `[softcct] package fences=` plus
   `[event] fence counts chiplet-local vs package`. Event
   create / record / wait still sits on SoftChipletSync; polish is
   honest **counts**, not a new IR. Not CUDA EventRecord. Frozen
   `IreeHalCmd` offsets unchanged. Single-die QEMU / host numbers are
   still not partner latency proof. Not Vulkan, not UCIe, not a
   coherence protocol.
3. **Per-task `CapTable` — skipped.** Two shim tenants do **not**
   alias slots. Kernel World (`kernel/src/world.rs`) still owns one
   `CapTable` for `TenantId(1)`. `aether-pjrt` and `aether-mp-shim`
   pin through a host Soft-SMMU walk; they do not mint World `CPtr`
   slots and do not share a table. No alias → no isolate. No
   `SYS_REVOKE`. Gate stays closed. Do not invent a CNode. No new
   syscall.

**Done when:** met. Diligence names the GreenCtx interference clip;
CCT/Event host tests still measure fence **counts**; CapTable skipped
(no alias). No new syscall. Path B / `make qemu` unchanged.

### M5–M6 — Feb–Mar 2027

**Done (this PR).** Decision checkpoint. Not a foundry date. Not
tape-out. `PartnerNpuStub` stays a labeled no-op.

**Fork named: research `IreeHalCmd` v1 stays.** There is no real
partner opcode table in-tree. [DESIGN_WIN.md](DESIGN_WIN.md) is still
a blank worksheet. The filled sample is the IREE HAL **research
stand-in**
([design-win/iree-hal-standin.md](design-win/iree-hal-standin.md)),
explicitly not a partner. Path B doorbell (PLIC UART-THRE on RISC-V;
CNTV/kthread drain on aarch64) has **not** failed a partner ask.

Therefore:

1. **freeze-v1 checkpoint.** Research `IreeHalCmd` v1 stays until a
   real partner table forces a dual update of [ACCEL.md](ACCEL.md) +
   `drivers/src/ireecp.rs` + host pack/unpack. Do **not** invent a
   vendor table. Do **not** dual-update the packet to look busy.
   Magic `0xAE7E1EE1`, 96-byte LE, `backend = 4`, executable
   `0x0001EE00`, TRANSFER reserved — unchanged. Freeze proof:
   `make design-win-standin` (`examples/design-win-check` re-packs
   sentinels through `IreeHalCmd::to_le_bytes` and asserts ACCEL.md
   offsets). 2028 H2 language does **not** move: signed opcode list
   **or** freeze the research ABI. This month **names** the fork. It
   does not tape out, does not claim a foundry date, does not retire
   `PartnerNpuStub` as anything but a labeled no-op.
2. **Diligence pack refresh.** [DILIGENCE.md](DILIGENCE.md) +
   [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) +
   [SELL_GOALS.md](SELL_GOALS.md) list the demos we can still run
   (see those files). Still not a partnership announcement. Still
   not a booked bring-up.
3. **Port skipped.** RISC-V virtio-mmio / aarch64 GIC stay hard
   deferred. Gate unchanged: only if path B doorbell fails a partner
   ask. Do not schedule both. Neither is a product-class second
   kernel.

**Done when:** met. freeze-v1 written; diligence pack matches the
demos we can still run; port skipped (no partner doorbell-fail ask).
No new syscall. Path B / `make qemu` unchanged. `IreeHalCmd`
offsets unchanged.

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
| [YEAR_AHEAD.md](YEAR_AHEAD.md) | **Next-12-month industry calendar** Sep 2026 → Sep 2027 |
| **This file** | **Closed** Sep 2026 → Mar 2027 record (M0–M6 landed) |
| [SELL_GOALS.md](SELL_GOALS.md) | Partner-facing demos / ask / non-claims for this quarter |
| [ROADMAP.md](ROADMAP.md) | Landed status, stubs, technical leftovers. Points at YEAR_AHEAD for the next twelve months |

Do not re-schedule Soft SMMU / Soft-CP / SMP / PML4 / `IreeShapedCp`
/ XQueue / SID-at-submit / SoftChipletSync / Month 5 digests 1–4 /
H2 2026 leftovers / Event / fabric-class / `ATOMIC_ADD` / SoftSFI
heap refuse / path-A IOVA / the sell set / MP-shaped thin consumer
(PR #83) / PJRT Add/Relu more ops (PR #84) / SoftGreenCtx interference
clip / SoftCCT/Event fence-count polish / M5–M6 freeze-v1
checkpoint / diligence refresh. Port stays skipped (no doorbell-fail
ask). CapTable stays gated (no alias). Do not sequence against the
SpecForge Y1H1–Y2H2 appendix.

[ROADMAP.md](ROADMAP.md) suggested-next-cuts that are not in
[YEAR_AHEAD.md](YEAR_AHEAD.md) remain **technical leftovers**, not
calendar. M0–M6 in this file stay closed.

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
4. M3–M4 — **landed:** SoftGreenCtx interference clip + SoftCCT/Event
   fence-count polish (PR #85). CapTable **skipped** (no shim-tenant
   slot alias; gate still closed). Do not re-schedule the clips.
5. M5–M6 — **landed (this PR):** freeze-v1 (research `IreeHalCmd` v1
   stays until a real partner table forces a dual update); diligence
   pack refresh; port **skipped** (path B doorbell has not failed a
   partner ask). Do not invent a vendor table. Do not re-schedule.

Site refreshes may land beside any of the above. They are not
numbered.

### File touch map (when a slice is pulled)

| Slice | Primary touches |
| --- | --- |
| Thin MP consumer (**landed**, PR #83) | `host/aether-mp-shim`; frozen `IreeHalCmd`; doorbell or Soft-CP. Doorbell sketch stays `examples/accel-client` |
| SoftSFI heap refuse (**landed**, PR #80) | `core/src/softsfi.rs`, `drivers/src/softsfi.rs`; `SoftOp::Heap` → `Unmodeled`; `[softsfi] heap=refused` |
| PJRT more ops (**landed**, PR #84) | `Add` / `Relu` on frozen `IreeHalCmd` (`function` 2 / 3). `host/aether-pjrt`, [HOST.md](HOST.md), [ACCEL.md](ACCEL.md), `drivers/src/ireecp.rs`. Offsets unchanged |
| SoftGreenCtx leave-behind (**landed**, PR #85) | [DILIGENCE.md](DILIGENCE.md), `examples/diligence-demo/`; `[greenctx] interference partitioned 70/30 vs unpartitioned`. Host tests already in `core/src/greenctx.rs` |
| SoftCCT / Event polish (**landed**, PR #85) | `core/src/chipsync.rs`, `host/aether-pjrt` `EventFenceCounts` / `cct_vs_broadcast`; `[softcct] package fences=` + `[event] fence counts`. [HOST.md](HOST.md) |
| Per-task CapTable (**skipped**, no alias) | `core/src/caps.rs`, kernel World; gate still closed. `SYS_REVOKE` only with unbind/FLR demo |
| Opcode v2 / freeze checkpoint | **Done this PR (freeze-v1).** Written note in this file + [ACCEL.md](ACCEL.md) ADR pointer. Packet untouched. Dual ACCEL.md + `ireecp` + host pack/unpack only if a real partner table arrives |
| Diligence refresh | **Done this PR.** [DILIGENCE.md](DILIGENCE.md), [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md), [SELL_GOALS.md](SELL_GOALS.md) |
| One port (if needed) | **Skipped.** No partner ask that path B doorbell failed. Gate stays. |

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
