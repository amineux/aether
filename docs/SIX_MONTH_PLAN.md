# Six-month plan (Falsifier revision)

Leave-behind for Kernel tracking. Falsifier revision 2026-09-07, after
PR #37 (site) and PRs #38–#40 (`IreeShapedCp`, CDT props, chiplet
affinity stub) landed on main. Filed on main via PR.

**This is the next calendar.** SpecForge OS-completeness theater is not
the schedule. There is no M3–M4 clock.

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
| Soft-CP | `backend = 3`, packed `CpCmd` + Soft SMMU SID + IRQ/fence |
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

Only M1 and M2 are the spine. M1 is **landed**. Optional / conditional
work is leave-behind, not a half-year pillar.

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

**Done when (met on this branch):**

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

### Optional M2 leave-behind — Soft SMMU bring-up kit

Docs plus dump/replay scripts tied to `AccelDevice::map`, so a silicon
team can watch STE→CD→S1/S2 on the same pins M1 uses.

**Not** a Soft-SMMU redo. **Not** a half-year pillar. Skip if M1–M2
already make the walk visible in host tests.

**Done when (if taken):** a short how-to + a replay of one map/translate
/ abort-until-bound / wrong-stream sequence. Software table only.

### Conditional only — guest PCI path-A bind

A kernel `VirtioAccelMmio` that talks PCI BAR0 **only if** it is needed
to prove Soft-SMMU IOVA on **path-A** DMA. Path B remains canonical.
The QEMU device model already landed (`qemu/aether_accel.c`).

Do not schedule this as M2. Do not rebuild QEMU in CI.

## Kill / hard defer

### KILL as calendar

These are **not** Kernel clock items. Some may exist as honest stubs;
none get an M-number.

| Item | Why it stays killed |
| --- | --- |
| Fork-shaped clone | `SYS_CLONE` shares aspace; a new PML4/`fork` is POSIX theater |
| User `open`/`read` POSIX surface | Ramfs is kernel-internal; no `SYS_OPEN` / `SYS_READ` |
| Soft-SMMU redo | STE→CD→S1/S2 already landed; deepen only as the optional kit |
| CXL productization / typed-window as milestone | `MemorySpace::CxlRegion` stays a typed place; no CXL.mem claim |
| ChipletFleet as milestone | n≤32 Fiedler placement already landed; not Fleet marketing |
| Formal seL4-style caps as milestone | Small CDT landed; proofs are a non-claim |
| Site-as-milestone | PR #37 is the leave-behind; no marketing climax |
| `PartnerNpuStub` theater without opcodes | No-op sketch; M1 is a real packet or it does not ship |

### HARD DEFER until they serve the partner spine

Parked. Pull only when M1–M2 need them.

| Item | Gate |
| --- | --- |
| Per-task `CapTable` | World still shares one table; isolate only if the shim has two tenants that must not alias slots |
| `SYS_REVOKE` | Internal `revoke` / `revoke_in` exist; a user syscall is not the partner ask |
| RISC-V virtio-mmio SoftNPU | PLIC software doorbell on path B is enough until a `-device` is required |
| aarch64 GIC SoftNPU IRQ | EL0 `/init` drains on timer/kthread; GICv3 is not the shim |
| MicroPerceptron interop | Secondary to PJRT/IREE; do not build a second compiler story |

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

## Post–M2 backlog (SpectraScout order)

**Park. Do not schedule as M3–M4 theater.** After opcodes + PJRT, this
is the preferred bite order — still software models, still no vendor
claim:

1. **Soft-CP SID-at-submit (Host1x-shaped).** Program / validate
   `StreamId` at doorbell, not only at map. Not a Host1x driver.
2. **Soft-CP XQueue.** Extra software execution queues on the CP
   mailbox. Not a silicon queueing unit.
3. **SoftChipletSync scoped timelines.** Chiplet-local fence domains
   on the existing seq/wait/complete model. Not UCIe sync.
4. **Optional PASID / SVA.** Process-ASID on Soft-SMMU CDs if the host
   shim needs per-client VAS. Software only.
5. **FlowHodgeQuota.** Already landed as admit/refuse. Further quota
   theater stays killed; only pull if the shim injects fabric headers.
6. **OperatorInject.** `OperatorKernelHandle` inject exists. Touch it
   only after Soft-CP (M1 path) is the stable submit surface.

**Skip:** SMMUv3 emulator, UCIe PHY. Hardware SMMU still needs partner
silicon; UCIe stays transport.

## Relationship to YEAR2_PLAN

[YEAR2_PLAN.md](YEAR2_PLAN.md) holds the 2026-09-06 Falsifier ACTIVE
track (Soft SMMU, Soft-CP, SMP, PML4, CDT, three-ISA `/init`, path B/A,
hardening) plus the SpecForge Y1H1–Y2H2 appendix.

That ACTIVE track is **done as research slices through PR #37**. This
file replaces it as the Kernel calendar. The SpecForge appendix stays
aspirational — including bank QoS EventRing theater, CXL objects, and
a Y2 bring-up climax.

[ROADMAP.md](ROADMAP.md) points here for what to sequence next.
Suggested next cuts in ROADMAP remain **technical leftovers**, not
M3–M4.

## Kernel PR order (this calendar)

1. `docs/SIX_MONTH_PLAN.md` + ROADMAP / YEAR2_PLAN pointers (**this
   cut**)
2. M1 partner-shaped opcode `AccelDevice` — **landed** as `IreeShapedCp`
   (PR #38)
3. M2 PJRT/IREE host shim submitting through `IreeShapedCp` (merge
   exploration A) — **landed** as `host/aether-pjrt` (PR #41)
4. Optional Soft-SMMU bring-up kit (docs + dump/replay)
5. Conditional path-A guest PCI bind — **only if** Soft-SMMU IOVA must
   be shown on path-A DMA

Do not open calendar PRs for fork, POSIX `open`/`read`, CXL
productization, ChipletFleet, formal caps, site refresh, or
PartnerNpuStub paint.

### File touch map

| Step | Primary touches |
| --- | --- |
| M1 opcode device | `hal/`, `drivers/` (new backend or Soft-CP packet), `docs/ACCEL.md`, host tests |
| M2 host shim | host crate or `core/src/abi.rs` expansion, `docs/ABI.md`, host tests; submit via M1 `AccelDevice` |
| Optional SMMU kit | `docs/` + scripts; dump/replay against `core/src/iommu.rs` |
| Conditional path A | guest `VirtioAccelMmio` only; CI still does not rebuild QEMU |

Cross-cutting: this file, ROADMAP status pointer, YEAR2_PLAN status
line. CI only if a new host-test target appears. No new syscall.

## What we will not claim

- That M1 is a signed vendor ISA or that a public header citation is a
  partnership
- That the M2 shim is a PJRT plugin, an IREE driver, or XLA integration
- Benchmarks, FLOPs, tape-out, or seL4 proofs
- That Soft SMMU / Soft-CP / path A became hardware
- That RISC-V or aarch64 is a product-class second kernel
- An M3–M4 OS-completeness clock (fork, POSIX, CXL.mem, ChipletFleet,
  SMMUv3 emulator, UCIe PHY)
