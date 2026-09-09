# Six-month plan (Falsifier revision)

Leave-behind for Kernel tracking. Falsifier revision 2026-09-07, after
PR #37 (site) and PRs #38–#41 (`IreeShapedCp`, CDT props, chiplet
affinity stub, PJRT/IREE shim) landed on main. Soft-CP XQueue landed
as **M4 (PR #47)**. Soft SMMU bring-up kit landed (PR #48). **M3
SID-at-submit is landed** (Host1x-shaped SET_SID; SID sticks on the
XQueue). SoftChipletSync scoped timelines **landed** after M3+M4
(PR #51). SoftGreenCtx SM/WQ partitions **landed** (Month 5 digest 1).
SoftCCT (Month 5 digest 3) **landed**. Site progress through PR #52.
Month 5 digest 2 (SoftCmdFirewall) is **landed** — see
[MONTH5_PLAN.md](MONTH5_PLAN.md).

**This calendar is closed (M1–M4 + SoftChipletSync + SoftCCT).**
[MONTH5_PLAN.md](MONTH5_PLAN.md) is the closed Month 5 record
(SoftGreenCtx, SoftCmdFirewall, SoftCCT, and SoftSFI digest 4 landed).
The **next calendar** is [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md)
(Sep 2026 → Sep 2028; SoftNoI-IS is in-flight / landing this PR as
an H2 2026 exploration, not marked Done; PASID/SVA / OperatorInject
stay explorations, not marked Done). SoftSFI is a toy
Soft-CP bytecode sandbox. SpecForge OS-completeness theater is not
the schedule. SpectraScout Soft-CP items are software models, not
fork / POSIX / CXL.mem / ChipletFleet.

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
  `CpCmd` (landed M1 packet is `IreeHalCmd`) over inventing a second
  IR.

## Reality check vs main (post PRs #37–#40)

The [YEAR2_PLAN.md](YEAR2_PLAN.md) Falsifier ACTIVE track is **complete
as research slices**. Year-1 + hardening already landed. Do **not**
re-schedule any of the following as new milestones:

| Landed | Honest reading |
| --- | --- |
| Soft SMMU | STE→CD→S1/S2 + ATS invalidate; not hardware |
| Soft-CP | `backend = 3`, packed `CpCmd` + SET_SID-at-submit + two software XQueues (queue-boundary) + SoftGreenCtx SM/WQ + SoftCmdFirewall copy-then-validate + IRQ/fence |
| **IreeShapedCp (PR #38)** | `backend = 4`, frozen `IreeHalCmd` from public IREE HAL nouns; **this is M1**; not a signed vendor |
| Path A | optional QEMU `aether-accel`; stock QEMU stays B |
| Cap CDT / revoke | small parent/child + `revoke_in`; no `SYS_REVOKE` |
| CDT property tests (PR #39) | host tests, not proofs (exploration D stub) |
| Chiplet affinity stub (PR #40) | `ChipletTaskScope`; ChipletFleet stays KILL |
| `SYS_CLONE` / `SYS_MMAP` | additive 10 / 11; not Linux clone / POSIX mmap |
| In-kernel ramfs + x86 virtio-blk seed | not a user `open`/`read` |
| RISC-V PLIC SoftNPU doorbell | path B BAR; not virtio-mmio |
| aarch64 EL0 `/init` | documented subset; not GICv3 |
| SMP smoke / per-task PML4 | QEMU `-smp 2`; x86 CR3 + USER 2 MiB |
| Site refresh | PR #37; research leave-behind, not a vendor pitch |

SoftNPU path-B opcodes stay Aether-native `Nop` / `MatMul` / `Wave`.
`PartnerNpuStub` (`backend = 2`) is a leftover no-op. Host ABI nouns in
`core/src/abi.rs` are types only. The M2 PJRT/IREE shim is
`host/aether-pjrt` (frozen `IreeHalCmd` → `IreeShapedCp`).

## Execution spine (this is the calendar)

**M1–M4 are done.** SoftChipletSync scoped timelines and SoftCCT
elision are **landed**. SoftGreenCtx (Month 5 digest 1) is **landed**.
SoftCmdFirewall (digest 2) is **landed**. Month 5 is **not** a
second half-year of this file — see [MONTH5_PLAN.md](MONTH5_PLAN.md)
(four digests landed; SoftNoI-IS in-flight / landing this PR). Optional / conditional work in *this* file is
leave-behind, not a pillar. Site-as-milestone stays killed (PR #46 /
#50 / #52 were
progress refreshes, not a marketing climax).

### M1 — partner-shaped opcode table (**landed, PR #38**)

`IreeShapedCp` (`backend = 4`) packs a frozen `IreeHalCmd` whose
fields are cited from public IREE HAL headers. Research opcodes — not
a vendor-as-partner claim. SoftNPU path B and Soft-CP (`backend = 3`)
unchanged. `PartnerNpuStub` stays a no-op.

Do **not** re-schedule this as a new milestone. M2 still submits
through this device.

**Done when (met on main):**

1. A distinct `AccelInfo.backend` (or a documented Soft-CP packet
   revision) packs a frozen little-endian command image. Field names
   and offsets are copied from one cited public source (IREE HAL
   command-buffer / device / buffer / executable / semaphore, or one
   other public ISA). Not an invented NVIDIA opcode list.
2. Host tests: `probe` / `submit` / `poll` / `map`; packet round-trip;
   refuse map/submit without Memory+MAP; wrong-stream is a software
   fault. Soft SMMU IOVA is not identity.
3. [ACCEL.md](ACCEL.md) ADR: source citation, packet layout, “research
   opcodes / not a partnership.” SoftNPU path B and `backend = 3`
   remain the stock demo unless this *is* the Soft-CP packet revision.
4. `PartnerNpuStub` stays a labeled no-op. No FLOP numbers. No new
   syscall.

### NOW–M2 — PJRT/IREE-shaped host shim (**landed, PR #41**)

A host-side shim that speaks **Device / MemorySpace / Buffer /
Executable / Event**, packs the frozen `IreeHalCmd` image (magic
`0xAE7E1EE1`, 96-byte LE, `backend = 4`, `ssid = 2`), and submits
through `IreeShapedCp`. Decode keys off the DISPATCH bit; Nop is
`categories = 0` (function ignored); v1 pack emits 0 or DISPATCH only
(`TRANSFER` alone is Fault). `workgroup_count` is AccelJobDesc `m,n,k`
(shape stand-in, not compiler tiles). Binding lengths are dtype-aware
byte spans. Executable is `0x0001EE00` only. SoftNPU remains the
`make qemu` path-B demo.

This is the **primary partner story**. Exploration A merged here.

Not `GetPjRtApi`. Not `iree_hal_driver_t`. Not in-kernel graph IR. Not
a vendor compiler integration.

**Done when (met on main):**

1. A host crate or module exposes those five nouns and lowers a submit
   onto `AccelJobDesc` → `IreeShapedCp` (`backend = 4`). SoftNPU
   virtqueue and/or Soft-CP may be extra backends. `PartnerNpuStub`
   is unused.
2. Host tests: create/probe device; allocate typed buffers
   (`HOST` / `DEVICE_HBM` / `TILE_SRAM` at minimum); bind an opaque
   Executable handle (kernel does not parse the blob); submit; wait on
   Event / the existing CP-shaped timeline. Mixed spaces stay
   non-unified unless `CapRights::UNIFIED` was granted.
3. Docs name the public PJRT / IREE HAL vocabulary and state the
   non-claims. Kernel syscalls and `UserAccelJob` are unchanged.
   `make qemu` still retires path-B SoftNPU.

### Optional M2 leave-behind — Soft SMMU bring-up kit (**landed, this cut**)

Docs plus dump/replay scripts tied to `AccelDevice::map` / Soft-CP SID
bind, so a silicon team can watch STE→CD→S1/S2 + ATS invalidate on the
same pins M1 uses (`ssid = 1` Soft-CP, `ssid = 2` IreeShapedCp).

**Not** a Soft-SMMU redo. **Not** a half-year pillar. Software tables
only.

**Done when (met on this branch):** a short how-to
([bringup/BRINGUP.md](bringup/BRINGUP.md)) + a replay of one
map / translate / abort-until-bound / wrong-stream / ATS sequence.
`scripts/smmu_{dump,replay}.py` + host test `smmu_bringup::replay_jsonl`.

### M3 — Soft-CP SID-at-submit (Host1x-shaped) (**landed**)

Program / validate `StreamId` at the Soft-CP doorbell, not only at
`map` / `bind_stream`. Host1x-shaped: the submit path names a stream
the way a channel CD / SID would, and a mismatch with the pinned CD
is a software fault.

M4 XQueue sticks the SID on the queue (`stamp_queue_sid` / first-submit
inherit, or privileged SET_SID on an empty queue). `CP_FLAG_SET_SID`
is the job-head stamp. Soft SMMU `resolve_submit` refuses until armed.

**Not** a Host1x driver. **Not** a NVIDIA channel. **Not** hardware
SMMU SID programming. SoftNPU path B stays. `IreeShapedCp` stamps
SET_SID on its single mailbox.

**Done when (met):**

1. `submit_xqueue` / `AccelDevice::submit` takes / checks a SID at
   doorbell time against the bound STE+CD. Map-only bind is no longer
   the only stream gate.
2. Host tests: SID-at-submit hit; unbound / wrong-SSID / wrong-STE
   refuse; existing map/cap refuse unchanged. No new syscall.
3. [ACCEL.md](ACCEL.md) names the Host1x shape and the non-claim.

### M4 — Soft-CP XQueue (XSched-shaped) (**landed, PR #47**)

Two software execution queues on Soft-CP (`create_xqueue` /
`submit_xqueue` / `suspend_xqueue` / `resume_xqueue`). Preemption is
**queue-boundary** only: `suspend` refuses the next packed `CpCmd`; a
command already inside `service()` runs to completion. SID sticks to
the queue (`stamp_queue_sid` / first-submit inherit). M3 SET_SID is
that doorbell stamp.

XSched (OSDI’25) is **inspiration** for an open queue object — not an
LD_PRELOAD CUDA/HIP shim, not a silicon queueing unit. Path B SoftNPU
/ `make qemu` unchanged. `IreeShapedCp` stays a single mailbox.
`CpCmd` layout unchanged.

**Done when (met on main):**

1. Soft-CP exposes more than one software queue without a second IR.
2. Host tests: two queues submit/poll; suspend A while B progresses
   (blast-radius); wrong-SID still Fault; stamp-hook inherit then
   refuse override. Existing single-queue tests still pass.
3. Docs: XSched-shaped software queues, not a product scheduler.

### SoftChipletSync scoped timelines (**landed**)

Chiplet-local fence domains on the existing seq / wait / complete model.
Scopes `{wave, CU, chiplet, package}`. Chiplet-local signal is free;
package-scope costs a hierarchical fence (last worker per participating
chiplet). SoftCCT (below) is the elision layer on this object.

Fleet hierarchical counters are **inspiration** — not a port, not a
Vulkan timeline product, not UCIe. Distinct from ChipletFleet
**placement** (`ChipletTaskScope`, still KILL-as-calendar). Host tests
measure fence **counts**. Latency wins need a multi-chiplet sim —
single-die QEMU / host numbers are not partner proof.

SID-at-submit + XQueue stay. Path B SoftNPU / `make qemu` unchanged.
`CpCmd` / `IreeHalCmd` layouts unchanged. No new syscall.

**Done when (met):**

1. Soft-CP (and IreeShapedCp mailbox) scoped timelines + one
   producer/consumer across two fake chiplets.
2. Host tests: package-scope fence count ≪ naive global fence;
   producer/consumer across two fake chiplets.
3. Docs name Fleet as inspiration only; non-claims above.

### SoftCCT elision on SoftChipletSync (**landed**)

Soft-CP buffer labels + last-writer chiplet (CPElide MICRO’24-shaped).
SoftChipletSync issues a package-scope fence **only** when SoftCCT says
a cross-chiplet hazard. Same-chiplet consume on a ≥2-chiplet package
elides. Single-chiplet CCT is a no-op.

CPElide is **inspiration** — not a full coherence protocol, not a
Vulkan / ROCm product, not silicon. Host tests measure fence **counts**
(CCT ≪ broadcast). An incorrect-elision test (ignore writer chiplet)
must fail. Path B SoftNPU / `make qemu` unchanged. No new syscall.

**Done when (met):**

1. Soft-CP / IreeShapedCp labeled producer (chiplet0) / consumer
   (chiplet1) consult SoftCCT.
2. Host tests: package-fence count with CCT ≪ broadcast-fence baseline
   on ≥2 fake chiplets; single-chiplet is a no-op; incorrect elision
   fails.
3. Docs name CPElide as inspiration only; non-claims above.

### SoftGreenCtx SM/WQ partitions (**landed**)

Soft-CP partitions a fake SM / work-queue pool (canonical 70/30).
XQueues bind to a `SoftGreenCtx`. CUDA Green Contexts and DetShare
(arXiv:2603.15042; no public repo) are **inspiration** — not a CUDA
driver, not a DetShare port, not HW MIG, not a BAR firewall.

Host tests co-run memcpy-like kernels and report BW interference vs
an unpartitioned baseline (normalized integer units, **not** FLOPs).
One migrate-to-yield moves queue A onto the larger slice at a queue
boundary; Soft-SMMU SID is unchanged. Residual shared-HBM tax stays
on so the 70% slice is still below solo.

SID-at-submit + XQueue + SoftChipletSync stay. Path B SoftNPU /
`make qemu` unchanged. `CpCmd` layout unchanged. `IreeShapedCp` stays
a single mailbox. No new syscall.

**Done when (met):**

1. Soft-CP `AccelInfo` advertises SM/WQ budget; two XQueues bind to
   70/30 SoftGreenCtx partitions.
2. Host tests: partitioned memcpy BW vs unpartitioned; migrate-to-yield
   without SID change; residual tax (not MIG).
3. Docs name Green Contexts / DetShare as inspiration only; non-claims
   above. Kernel serial `[greenctx]`.

### Conditional only — guest PCI path-A bind

**Host IOVA proof landed** (`drivers/src/path_a.rs`, `make accel-test`).
A kernel `VirtioAccelMmio` that talks PCI BAR0 is **not** required
for that proof. Path B remains canonical. The QEMU device model
already landed (`qemu/aether_accel.c`).

Do not schedule a guest PCI bind as M3. Do not rebuild QEMU in CI.

## Kill / hard defer

### KILL as calendar

These are **not** Kernel clock items. Some may exist as honest stubs;
none get an M-number.

| Item | Why it stays killed |
| --- | --- |
| Fork-shaped clone | `SYS_CLONE` shares aspace; a new PML4/`fork` is POSIX theater |
| User `open`/`read` POSIX surface | Ramfs is kernel-internal; no `SYS_OPEN` / `SYS_READ` |
| Soft-SMMU redo | STE→CD→S1/S2 already landed; the optional kit dumps those tables |
| CXL productization / typed-window as milestone | `MemorySpace::CxlRegion` stays a typed place; no CXL.mem claim |
| ChipletFleet as milestone | n≤32 Fiedler placement already landed; not Fleet marketing |
| Formal seL4-style caps as milestone | Small CDT landed; proofs are a non-claim |
| Site-as-milestone | PR #37 is the leave-behind; no marketing climax. A later progress refresh is OK; it is not a calendar item |
| `PartnerNpuStub` theater without opcodes | No-op sketch; M1 is a real packet or it does not ship |

### HARD DEFER until they serve the partner spine

Parked. Pull only when M3 (or the host shim) needs them.

| Item | Gate |
| --- | --- |
| Per-task `CapTable` | World still shares one table; isolate only if the shim has two tenants that must not alias slots |
| `SYS_REVOKE` | Internal `revoke` / `revoke_in` exist; a user syscall is not the partner ask |
| RISC-V virtio-mmio SoftNPU | PLIC software doorbell on path B is enough until a `-device` is required |
| aarch64 GIC SoftNPU IRQ | EL0 `/init` drains on timer/kthread; GICv3 is not the shim |
| MicroPerceptron interop | Secondary to PJRT/IREE; do not build a second compiler story. Doorbell sketch (`examples/accel-client`) is not this slice |

## Exploration digests (not milestones)

User-requested thin PRs. They may land as **honest stubs** without
becoming calendar. Prefer merging A into M1–M2.

| | Slice | Honest bound |
| --- | --- | --- |
| **A** | PJRT/IREE host shim | **Merged into M2 (PR #41).** Frozen `IreeHalCmd` → `IreeShapedCp`. Not a plugin. |
| **B** | Multi-tenant blast-radius demo | One-week diligence clip: two tenants, CrossCut + wrong-SID refuse. Not a second ring-3 World. |
| **C** | Chiplet affinity placement | **Landed as stub (PR #40).** `ChipletTaskScope`. Not ChipletFleet marketing. |
| **D** | Mint/derive/revoke property tests | **Landed as stub (PR #39).** `caps_props.rs`. Not proofs, not a seL4 CNode. |
| **E** | CXL.mem typed-window stub | Inspiration only. Typed place / window sketch. Not productization, not Y2H1 CXL objects. |

If A had landed as a shim over SoftNPU / Soft-CP (`backend` 1 / 3)
only, M2 would still be open. PR #41 packs `IreeHalCmd` and submits
through `IreeShapedCp`; SoftNPU stays the qemu demo.

## SpectraScout leftovers (after SoftChipletSync + SoftCCT + SoftGreenCtx)

M3 SID-at-submit, M4 XQueue, SoftChipletSync, SoftCCT, and SoftGreenCtx
are **landed**. Still software models, still no vendor claim.
Sequencing moved to [MONTH5_PLAN.md](MONTH5_PLAN.md):

1. **SoftChipletSync scoped timelines** (**landed**). Chiplet-local
   fence domains; Fleet inspiration only. Not UCIe sync.
2. **SoftCCT** (**landed**, Month 5 digest 3). Last-writer chiplet per
   buffer label; package fence only on a cross-chiplet hazard.
   CPElide inspiration only. Not a coherence protocol, not Vulkan /
   ROCm. Single-chiplet is a no-op.
3. **SoftGreenCtx SM/WQ partitions** (**landed**, Month 5 digest 1).
   Fake 70/30 SM/WQ pool; XQueue bind; memcpy interference vs
   unpartitioned; migrate-to-yield without SID change. Not HW MIG.
4. **PASID / SVA** — **parked leftover** (per-AccelDevice PASID;
   bind process VA ↔ Soft-SMMU SSID; unmap → SSID TLB invalidate).
   Software only. Not zero-copy SVA without the invalidate path.
5. **FlowHodgeQuota.** Already landed as admit/refuse. DMA class
   headers stay killed as theater. SoftNoI consumes a software
   `FlowClass` tag on `AccelJobDesc` instead.
6. **OperatorInject deepen** — **parked leftover** (Soft-CP)
   resident worker + versioned ops). Distinct from landed
   `OperatorKernelHandle` Hodge inject. Not NVRTC/CUDA.

Month 5 remaining (not leftovers): none of the four digests.
SoftGreenCtx, SoftCmdFirewall, SoftCCT, and SoftSFI (**digest 4**;
GPU-AToLL-shaped toy ISA; `atomic_add` later SID-proved; tensor / heap
`Unmodeled`) are
**landed**. SoftNoI-IS is **in-flight / landing this PR** (H2 2026
exploration; not a Month 5 digest; not marked Done).
See [MONTH5_PLAN.md](MONTH5_PLAN.md).

**Skip:** SMMUv3 emulator, UCIe PHY. Hardware SMMU still needs partner
silicon; UCIe stays transport. Full kill / exploration menu lives in
the Month 5 plan.

## Relationship to YEAR2_PLAN

[YEAR2_PLAN.md](YEAR2_PLAN.md) holds the 2026-09-06 Falsifier ACTIVE
track (Soft SMMU, Soft-CP, SMP, PML4, CDT, three-ISA `/init`, path B/A,
hardening) plus the SpecForge Y1H1–Y2H2 appendix.

That ACTIVE track is **done as research slices through PR #37**. This
file is the closed M1–M4 Kernel calendar (it replaced YEAR2_PLAN’s
ACTIVE track). The **next calendar** is
[TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md). The SpecForge appendix stays
aspirational — including bank QoS EventRing theater, CXL objects, and
a Y2 bring-up climax.

[ROADMAP.md](ROADMAP.md) points at [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md)
for what to sequence next. This file remains the closed M1–M4
record. SoftChipletSync, SoftCCT, SoftGreenCtx, and SoftCmdFirewall
are landed. SoftSFI (digest 4) is **landed**. SoftNoI-IS is
**in-flight / landing this PR**. Suggested next cuts
in ROADMAP that are not parked leftovers remain **technical leftovers**.

## Kernel PR order (this calendar)

1. `docs/SIX_MONTH_PLAN.md` + ROADMAP / YEAR2_PLAN pointers — **landed**
2. M1 partner-shaped opcode `AccelDevice` — **landed** as `IreeShapedCp`
   (PR #38)
3. M2 PJRT/IREE host shim submitting through `IreeShapedCp` (merge
   exploration A) — **landed** as `host/aether-pjrt` (PR #41)
4. M4 Soft-CP XQueue (XSched-shaped) — **landed** (PR #47; ahead of M3)
5. Optional Soft-SMMU bring-up kit (docs + dump/replay) — **landed** (PR #48)
6. M3 Soft-CP SID-at-submit (Host1x-shaped) — **landed**
7. SoftChipletSync scoped timelines — **landed**
8. SoftGreenCtx (Month 5 digest 1) — **landed** (PR #54)
9. SoftCCT elision on SoftChipletSync — **landed** (PR #57)
10. Conditional path-A guest PCI bind — Soft-SMMU IOVA on path-A DMA
    **landed as a host proof** (`PathABar` / `make accel-test`). Kernel
    PCI BAR0 bind still not required. See
    [MONTH5_PLAN.md](MONTH5_PLAN.md) / [ACCEL.md](ACCEL.md).
11. Month 5 remaining — [MONTH5_PLAN.md](MONTH5_PLAN.md)
    (SoftSFI digest 4 **landed**; SoftNoI-IS in-flight / this PR;
    PASID/SVA and OperatorInject parked)

Do not open calendar PRs for fork, POSIX `open`/`read`, CXL
productization, ChipletFleet, formal caps, site-as-milestone, or
PartnerNpuStub paint. A later site progress refresh is not a
milestone.

### File touch map

| Step | Primary touches |
| --- | --- |
| M1 opcode device | `hal/`, `drivers/` (new backend or Soft-CP packet), `docs/ACCEL.md`, host tests |
| M2 host shim | host crate or `core/src/abi.rs` expansion, `docs/ABI.md`, host tests; submit via M1 `AccelDevice` |
| Optional SMMU kit | `docs/bringup/`, `scripts/smmu_*.py`, `core/src/{iommu,smmu_bringup}.rs`; dump/replay against `IommuMap` |
| M3 SID-at-submit | `drivers/src/fakecp.rs`, `docs/ACCEL.md`, host tests |
| M4 XQueue | `drivers/src/fakecp.rs`, `hal/` (`n_queues`) — **landed PR #47** |
| SoftChipletSync | `core/src/{fence,chipsync}.rs`, Soft-CP / IreeShapedCp retire, host tests — **landed** |
| SoftGreenCtx | `core/src/greenctx.rs`, Soft-CP / HAL `sm_wq_budget`, host tests — **landed** |
| SoftCmdFirewall | `drivers/src/firewall.rs`, Soft-CP `submit_xqueue` / `submit_cmdbuf`, host tests — **landed** |
| SoftCCT | `core/src/chipsync.rs` (`SoftCct`), Soft-CP buffer labels, host tests — **landed** |
| SoftSFI | `core/src/softsfi.rs`, `drivers/src/softsfi.rs` `submit_sfi` / skip-verify, host tests — **landed** |
| SoftNoI-IS | `core/src/noi.rs`, SoftChipletSync advertisement, Soft-CP `submit_xqueue_noi`, host tests — **in-flight / this PR** |
| Conditional path A | host `PathABar` IOVA / wrong-SID proof; CI still does not rebuild QEMU |
| Month 5 digests | [MONTH5_PLAN.md](MONTH5_PLAN.md) file-touch map (SoftGreenCtx / SoftCmdFirewall / SoftCCT / SoftSFI) |

Cross-cutting: this file, ROADMAP status pointer, YEAR2_PLAN status
line, [MONTH5_PLAN.md](MONTH5_PLAN.md). CI only if a new host-test
target appears. No new syscall.

## What we will not claim

- That M1 is a signed vendor ISA or that a public header citation is a
  partnership
- That the M2 shim is a PJRT plugin, an IREE driver, or XLA integration
- That M3 is a Host1x driver or that M4 (PR #47) is a silicon XQueue
  or an XSched LD_PRELOAD shim
- Benchmarks, FLOPs, tape-out, or seL4 proofs
- That Soft SMMU / Soft-CP / path A became hardware
- That RISC-V or aarch64 is a product-class second kernel
- That SoftChipletSync is a Vulkan timeline product, UCIe sync, a
  coherence protocol, ChipletFleet placement, or a multi-chiplet
  latency result from single-die host tests
- That SoftGreenCtx is HW MIG, a BAR firewall, a CUDA Green Context
  driver, a DetShare port, silicon SM isolation, or a FLOP / partner
  bandwidth result
- That SoftCmdFirewall is a Tegra Host1x driver, confidential GPU,
  HBM encryption, GPU-CC HMAC, or NVIDIA SEC2
- That SoftCCT is a full coherence protocol, CPElide silicon, or a
  Vulkan / ROCm product
- That SoftNoI-IS synthesizes NoI topology, is UniCNet, or is
  already Done on this closed calendar
- An OS-completeness M3–M4 clock (fork, POSIX, CXL.mem, ChipletFleet,
  SMMUv3 emulator, UCIe PHY). SpectraScout Soft-CP M3–M4 + SoftChipletSync
  + SoftGreenCtx + SoftCCT is the software-model track; that theater is not.
