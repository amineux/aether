# Year 2 plan

Leave-behind for Kernel tracking. SpecForge draft 2026-09-06; Falsifier
revision 2026-09-06. Filed on main via PR.

## Non-negotiables

- Research prototype. SoftNPU + stock/custom QEMU is the demo path.
- No fake NVIDIA partnership, no FLOP benchmarks, no tape-out / readiness
  claims.
- [`docs/ABI.md`](ABI.md) syscall 0–8 stay frozen; additive only
  (`exit=9` already additive).
- Prefer extending `aether_hal::AccelDevice` + `AccelJobDesc` over
  inventing a second IR.

## Falsifier revision — execution track

**This is the ACTIVE track.** SpecForge's half-year calendar
([appendix](#specforge-criteria-aspirational-appendix)) is aspirational
only — do not schedule Kernel work against it. Soft SMMU (PR #7) and
the SoftCommandProcessor AccelDevice (packed `CpCmd` + IRQ/fence) are
**done** as software models. SMP smoke (INIT-SIPI + per-CPU `gs` +
two-hart work-steal) is **done** as a QEMU `-smp 2` slice. Custom QEMU
virtio-accel and the other stubs remain open (see
[ROADMAP.md](ROADMAP.md)). Per-task PML4 + SMEP/SMAP is **done** as
an x86 documented subset (CR3 switch, task-local USER 2 MiB windows;
no higher-half / POSIX MM). A minimal cap CDT / revoke is **done** as
unscheduled Y2H1 security work (parent/child edges + `revoke_in`;
not a seL4 CNode, not a calendar milestone).

### KEEP / ACTIVE Y1

1. Soft SMMU: non-identity IOVA + `stream_id` on the `AccelDevice` map
   path (not a SoftMMU product claim). Map without Memory+MAP still
   refuses; wrong-stream DMA fault/reject is a software model.
2. One real-shaped `AccelDevice` path beyond SoftNPU that packs a
   **concrete command packet** + fence/IRQ complete (software model OK).
   Distinct backend id; probe/submit/poll/map contract tests. This is
   **not** `PartnerNpuStub` enrichment theater.
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
- CXL region objects — keep the typed `MemorySpace::CxlRegion` place
  only; no pin/map productization, no CXL.mem claim.
- Cap CDT as a *calendar* item. A small derivation-edge revoke landed
  as unscheduled Y2H1 security work; do not treat it as a SpecForge
  half-year clock, a seL4 clone, or a reason to pull CXL / Laplacian.
- Y2 manufacturing / bring-up playbook as a climax goal
  ([DILIGENCE.md](DILIGENCE.md) already exists).

### DEFER

After AccelDevice bites a real-shaped path — not before:

- Custom QEMU virtio-accel (`-device` / virtio-mmio DMA of the
  [ACCEL.md](ACCEL.md) BAR layout). In-kernel BAR + SoftNPU remains
  the honest demo.
- ELF beyond this subset (higher-half, PIE, ramfs). Per-task PML4 +
  SMEP/SMAP + optional `/probe` is landed.
- Laplacian expansion (n≤32 placement; AffinityLaplacian in sched).
- aarch64 (thin HAL after the x86 ABI is stable).

### PR order for Kernel (revised)

1. `docs/YEAR2_PLAN.md` with both SpecForge criteria + this Falsifier
   active track
2. Soft SMMU SIDs on the AccelDevice path
3. Second AccelDevice software CP with a real command packet (not
   `PartnerNpuStub` enrichment theater)
4. SMP smoke (INIT-SIPI, per-CPU `gs`, two-hart work-steal)
5. Per-task PML4 + SMEP/SMAP (documented x86 subset)
6. Minimal cap CDT / revoke (internal API + host/QEMU demo) — **this cut**
7. Optional virtio-accel / MicroPerceptron interop later

### Active file touch map

| Step | Primary touches |
| --- | --- |
| Soft SMMU SIDs | `core/src/iommu.rs`, `drivers/src/{mmio,softnpu}.rs`, tests under `core/` |
| Second software CP | `hal/`, `drivers/` (new backend, not partner-stub paint), `docs/ACCEL.md` |
| Cross-cutting | this file, ROADMAP status rows, CI only if a new host test target appears |
| SMP smoke | `kernel/src/arch/{irq,x86_64/{smp,cpu,apic,idt}}.rs`, `Makefile`, `qemu-smp-ci` |
| Per-task PML4 | `kernel/src/{mm,task,elfload}.rs`, `core/src/aspace.rs`, `user/probe/`, `qemu-ci` |
| Cap CDT / revoke | `core/src/caps.rs`, `core/src/demo.rs`, `docs/{SECURITY,ROADMAP,YEAR2_PLAN}.md` |

The Soft SMMU / Soft-CP track asked not to open `kernel/src/arch/` PRs.
That gate opened after AccelDevice (PR #8). SMP smoke is the first
arch PR on the revised track. CDT landed as a small `aether-core`
slice. Still do not open CXL PRs here.

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
   honest fallback.
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
   (per-user CR3, USER-local 2 MiB windows, `/probe`, no higher-half).

### Y2H1 — Package scale + revoke

**Done when:**

1. Multi-chiplet: affinity graph spans ≥2 chiplets; scheduler refuses
   cross-cut without BIND; AffinityLaplacian used in sched placement
   (not only Cut enum n≤8) for at least an n≤32 host-tested case.
2. CXL region objects: `MemorySpace::CxlRegion` is a real typed place
   with pin/map path; coherent remote load still refused without
   UNIFIED; QEMU stub window OK — no claim of real CXL.mem.
3. Cap CDT/revoke: revoke parent empties descendants across tables;
   host tests lock it; [SECURITY.md](SECURITY.md) gap table updated.
   **Landed** as a small derivation tree (`cdt`/`parent`, `revoke_in`
   of named tables). Not a seL4 CNode/MDB. No `SYS_REVOKE`. Kernel
   World is still one shared `CapTable`. CXL / Laplacian items above
   are **not** done.

### Y2H2 — Bring-up + ecosystem docs

**Done when:**

1. Bring-up playbook: `docs/BRINGUP.md` — QEMU/SoftNPU path, how a
   partner maps CP registers into AccelDevice, failure modes, STUB
   list.
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
   de-facto ABI for SoftNPU ↔ future QEMU device. Freeze layout before
   Y1H1 QEMU work; any change needs a dual SoftNPU+doc update — merge
   conflict magnet with the partner sketch.
3. **SMP vs userspace:** INIT-SIPI + per-CPU state will thrash
   `arch/x86_64` and `task.rs` the same as PML4 work — sequence Y1H2 as
   **SMP first (kernel threads)** then **per-task PML4**, or one owner
   for both to avoid conflict.
4. **Laplacian in sched:** AffinityLaplacian today is a prototype
   (n≤8). Wiring into sched before a stable placement API will churn
   `cut.rs`/`sched.rs` under Y2H1 — freeze `SpectralCut` BIND semantics
   in Y1H2.
5. **Cap CDT:** Touches every mint/derive path. The small revoke
   slice is landed behind host + boot-demo tests; still land any
   later CXL/multi-chiplet demos on that API, not a new tree.
6. **RISC-V temptation:** Do not block Y1 on `sret` userspace; keep a
   thin HAL. aarch64 after the x86 ABI is stable.

### PR order (SpecForge original)

1. `docs/YEAR2_PLAN.md` (this leave-behind) + ROADMAP pointer
2. Y1H1 iommu `stream_id` + credit refuse tests
3. Y1H1 virtio path decision (A vs B) as an explicit ADR in [ACCEL.md](ACCEL.md)
4. Y1H2 SMP smoke
5. Y1H2 PML4
6. Y2H1 CDT → CXL place → laplacian-in-sched
7. Y2H2 docs/site only
