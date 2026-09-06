# Year 2 plan

Leave-behind for Kernel tracking. SpecForge draft 2026-09-06; filed on
main via PR.

## Non-negotiables

- Research prototype. SoftNPU + stock/custom QEMU is the demo path.
- No fake NVIDIA partnership, no FLOP benchmarks, no tape-out / readiness
  claims.
- [`docs/ABI.md`](ABI.md) syscall 0–8 stay frozen; additive only
  (`exit=9` already additive).
- Prefer extending `aether_hal::AccelDevice` + `AccelJobDesc` over
  inventing a second IR.

## Milestone acceptance

These milestones are **not done**. Soft SMMU, bank QoS credits, a real
virtio-accel QEMU path, SMP, per-task PML4, multi-chiplet placement,
CXL region objects, and cap CDT/revoke remain in progress or stub on
the current tree (see [ROADMAP.md](ROADMAP.md)).

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
   per-CPU `gs`/tick; work-steal driven by ≥2 harts; `make qemu` SMP
   smoke.
3. ELF userspace maturity: per-task PML4 (or documented subset) +
   SMEP/SMAP on x86; `/init` still static ELF; optional second user
   binary; no PIE required.

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

### Y2H2 — Bring-up + ecosystem docs

**Done when:**

1. Bring-up playbook: `docs/BRINGUP.md` — QEMU/SoftNPU path, how a
   partner maps CP registers into AccelDevice, failure modes, STUB
   list.
2. Open AccelDevice ecosystem docs: [ACCEL.md](ACCEL.md) +
   [DILIGENCE.md](DILIGENCE.md) refreshed; sample third-party sketch
   crate or `drivers/` template; `site/` roadmap section matches this
   plan (no vendor logos as partners).

## File touch map (expected)

| Milestone | Primary touches |
| --- | --- |
| Y1H1 | `core/src/iommu.rs`, `core/src/partition.rs`, `core/src/fence.rs`, `drivers/src/{mmio,virtio_accel,softnpu,partner}.rs`, `docs/ACCEL.md`, tests under `core/` |
| Y1H2 | `hal/`, `drivers/` (2nd CP), `kernel/src/arch/{x86_64,irq}.rs`, `kernel/src/{mm,task,elfload}.rs`, `boot/x86_64/`, `user/`, `Makefile`, `docs/ARCHITECTURE.md` |
| Y2H1 | `core/src/{laplacian,cut,sched,caps,space}.rs`, `docs/{CUT,SECURITY,ABI}.md` |
| Y2H2 | `docs/BRINGUP.md` (new), `docs/{ACCEL,DILIGENCE,ROADMAP}.md`, `site/` copy only — do not fork marketing claims |

Cross-cutting: this file, ROADMAP status rows, CI (`.github/workflows/ci.yml`)
for new qemu/smp targets.

## Sequencing risks

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
5. **Cap CDT:** Touches every mint/derive path; land behind tests
   before CXL/multi-chiplet demos that mint many caps.
6. **RISC-V temptation:** Do not block Y1 on `sret` userspace; keep a
   thin HAL. aarch64 after the x86 ABI is stable.

## PR order

1. `docs/YEAR2_PLAN.md` (this leave-behind) + ROADMAP pointer
2. Y1H1 iommu `stream_id` + credit refuse tests
3. Y1H1 virtio path decision (A vs B) as an explicit ADR in [ACCEL.md](ACCEL.md)
4. Y1H2 SMP smoke
5. Y1H2 PML4
6. Y2H1 CDT → CXL place → laplacian-in-sched
7. Y2H2 docs/site only
