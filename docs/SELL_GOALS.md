# Sell goals (this quarter)

What a silicon or compiler team can **run** this quarter. Not a
kernel feature. Not a partnership announcement. Research prototype.

**The ask:** bring your opcode table. Fill
[DESIGN_WIN.md](DESIGN_WIN.md) against frozen `IreeHalCmd`. A filled
worksheet, or a written no with reasons, is a good outcome. The IREE
HAL stand-in ([design-win/iree-hal-standin.md](design-win/iree-hal-standin.md))
is a research mapping, **not** a partner.

Near-term calendar: [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md)
(Sep 2026 → Mar 2027). Horizon: [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md).
Week 1 script: [WEEK1_CALL.md](WEEK1_CALL.md). Eight minutes:
[PITCH.md](PITCH.md).

Path B is canonical. Soft SMMU is software. Frozen `IreeHalCmd`.
No fake NVIDIA. No FLOPs. No tape-out.

## What a partner gets this quarter

Clone the tree. Rustc 1.83+ on the host. **No QEMU rebuild** for the
call. Stock `make qemu` stays the path-B guest demo if they want it.

| Demo | Command | What they see |
| --- | --- | --- |
| Diligence clip | `make diligence-demo` | Two tenants. CrossCut + wrong-SID refuse. Frozen `IreeHalCmd` submit + wait. Event create/record/wait + fence **counts**. SoftCCT package ≪ broadcast. SoftCmdFirewall mutation-during-validate fails. SoftGreenCtx 70/30 + interference vs unpartitioned. Honest close. |
| Named attacks | `make red-team` | wrong-SID / SoftCmdFirewall / SoftSFI-OOB / SoftNoI-IS / PASID-stale **refused**. Fabric-class admit/refuse. `ATOMIC_ADD` accept/reject. `[softsfi] heap=refused`. |
| Frozen packet | `make partner-hello` | 96-byte `IreeHalCmd` → `IreeShapedCp`. Magic `0xAE7E1EE1`. 2×2 I32 matmul. Unknown executable refused. |
| Doorbell (second caller) | `cargo run -p aether-accel-client` | Same 96-byte image. Not a Makefile target. Not a MicroPerceptron port. |
| MP-shaped thin consumer | `make mp-shim` | `host/aether-mp-shim` (PR #83). Same frozen image. memcpy / matmul / wave. Inspiration name only. Secondary to PJRT. Not a port. |
| PJRT Add/Relu | `cargo test -p aether-pjrt` | Additive `Add=3` / `Relu=4` on the frozen `IreeHalCmd` (`function` 2 / 3; PR #84). Offsets unchanged. Not `GetPjRtApi`. |
| Worksheet | `make design-win-check` / `make design-win-standin` | Blank they fill, or the IREE HAL research stand-in (not a partner). TRANSFER-only refused. |
| Path-A IOVA | `make accel-test` | Soft-SMMU IOVA on the optional BAR; wrong SID aborts. Stock `make qemu` stays B. Do not rebuild QEMU. |
| Guest Path B | `make qemu` | In-kernel SoftNPU BAR. Optional. Not required for the call. |

Captured stdout (if cargo is cold):
[pitch/diligence-demo.log](pitch/diligence-demo.log),
[pitch/red-team.log](pitch/red-team.log).

## Proofs we can show live

Walk this order. Host clips. No new kernel feature. No invented
Makefile targets.

| Proof | Point at |
| --- | --- |
| Isolation | `make red-team` — named attacks refused |
| Diligence narrative | `make diligence-demo` — blast / pjrt / event / softcct / firewall / greenctx |
| Partner hello | `make partner-hello` — frozen packet, no QEMU |
| MP-shaped thin consumer | `make mp-shim` — same frozen `IreeHalCmd`; inspiration name only |
| PJRT Add/Relu | `cargo test -p aether-pjrt` — same frozen packet; `function` 2 / 3; PR #84 |
| Path-A IOVA | `make accel-test` — Soft-SMMU IOVA / wrong-SID on the optional BAR |
| Event wait | `make diligence-demo` — grep `[event] SoftChipletSync create/record/wait` |
| Event fence counts | `make diligence-demo` — grep `[event] fence counts chiplet-local vs package` |
| SoftCCT vs broadcast | `make diligence-demo` — grep `[softcct] package fences=` |
| SoftGreenCtx interference | `make diligence-demo` — grep `[greenctx] interference partitioned 70/30 vs unpartitioned` |
| Fabric-class admit | `make red-team` — grep `[redteam] fabric-class admit/refuse` |
| SoftSFI `ATOMIC_ADD` | `make red-team` — grep `[redteam] ATOMIC_ADD accept/reject` |
| SoftSFI heap refuse | `make red-team` — grep `[softsfi] heap=refused` |

How to plug a CP: [ACCEL.md](ACCEL.md) + `aether_hal::AccelDevice`.
Isolation invariants: [SECURITY.md](SECURITY.md), [BLAST.md](BLAST.md).
What is stubbed: [DILIGENCE.md](DILIGENCE.md).

## Non-claims

We will **not** claim:

- An NVIDIA partnership — or any booked ASIC / compiler bring-up.
  `PartnerNpuStub` is a labeled no-op.
- FLOPs, benchmarks vs Linux / seL4 / CUDA / any NPU SDK.
- Tape-out, a foundry date, or manufacturing readiness. 2028 is a
  signed opcode list **or** a research ABI freeze.
- Hardware SMMU. Soft SMMU is software. A real device can DMA past it.
- That path A is the demo. Path B (stock `make qemu`) is canonical.
- A PJRT plugin (`GetPjRtApi`), an IREE HAL driver, or XLA.
- MIG-class isolation, confidential GPU, ARM SVA / CUDA UVA, UCIe,
  a coherence protocol, or seL4 proofs.
- That [DESIGN_WIN.md](DESIGN_WIN.md) is a signed contract, or that
  this file is a design win in hand.
- That `host/aether-mp-shim` is a MicroPerceptron port, a vendor, or
  a second compiler story.

## Six-month sell milestones (demos / docs, not OS theater)

These are **call artifacts**. They are not fork, POSIX, Soft-SMMU
redo, CXL, ChipletFleet, seL4, SMMUv3 emu, UCIe PHY, PartnerNpuStub
paint, or site-as-milestone.

| When | Sell slice | Honest bound |
| --- | --- | --- |
| **M0 now (Sep)** | Sell pack + call pack live. Capture feedback. Fill DESIGN_WIN from a **real** table when one appears. | Pack is already in tree. Do not invent a vendor table. |
| **M1–M2 (Oct–Nov)** | **Done.** Thin MP-shaped consumer (PR #83; `make mp-shim`). SoftSFI heap refuse (PR #80; named `Unmodeled`, not a bump allocator). PJRT Add/Relu more ops (PR #84; same frozen packet, `function` 2 / 3). | Same packet. Dual ACCEL.md + `ireecp` + host pack/unpack if the image moves. |
| **M3–M4 (Dec–Jan)** | **Done.** SoftGreenCtx interference clip (`[greenctx] interference partitioned 70/30 vs unpartitioned`). SoftCCT / Event fence-**count** polish (`[softcct] package fences=` + `[event] fence counts`). CapTable **skipped** (no shim-tenant slot alias; gate still closed). | Not HW MIG. Not a latency claim from single-die numbers. |
| **M5–M6 (Feb–Mar)** | Opcode-table v2 **or** freeze-v1 checkpoint. Diligence pack refresh. One port (RISC-V virtio-mmio **or** aarch64 GIC) **only if** path B’s doorbell fails a partner ask. | 2028 language unchanged: signed list or freeze research ABI. Not tape-out. |

M0 does not need a kernel PR. Later months land only if they stay
demos or docs on the frozen packet. Calendar:
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md).
