# Year ahead (Sep 2026 → Sep 2027)

Industry calendar for a sales or partner call. Isolation is the
product (blast radius), not FLOPs. Research prototype.

**This is the next-twelve-month Kernel + sell calendar.**
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) is the **closed**
Sep 2026 → Mar 2027 record (M0–M6 landed; do not re-schedule).
[TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) stays the Sep 2026 → Sep 2028
horizon. 2028 language does **not** move: a signed opcode list from
a real partner CP, **or** freeze the research ABI. Not this year’s
climax. Not tape-out.

Live site: [https://amineux.github.io/aether/#sell](https://amineux.github.io/aether/#sell)
· [https://amineux.github.io/aether/#roadmap](https://amineux.github.io/aether/#roadmap).
One-page leave-behind: [SELL_PACK.md](SELL_PACK.md). Printable:
[pitch/partner-one-pager.md](pitch/partner-one-pager.md). This-quarter
demos: [SELL_GOALS.md](SELL_GOALS.md). Week 1 20-minute pack:
[WEEK1_CALL.md](WEEK1_CALL.md). Eight-minute script:
[PITCH.md](PITCH.md).

**Ask:** bring your opcode table.

---

## Honesty (read first)

Research prototype. Path B (stock `make qemu`, in-kernel SoftNPU BAR)
is canonical. Soft SMMU is **software**. Frozen `IreeHalCmd` v1
(magic `0xAE7E1EE1`, 96-byte LE, `backend = 4`, executable
`0x0001EE00`) — **freeze-v1 named in #86**. ABI syscalls **0–11**
stay frozen; additive only.

We will **not** claim: an NVIDIA partnership; FLOPs; tape-out or a
foundry date; hardware SMMU; HW MIG; confidential GPU; a PJRT plugin
/ `GetPjRtApi`; a signed vendor ISA; a booked ASIC bring-up; a
product kernel.

`PartnerNpuStub` is a labeled no-op. The IREE HAL stand-in is a
research mapping, **not** a partner. Site refresh is **not** a
milestone.

---

## Spine (Sep 2026 → Sep 2027)

```text
Now (Sep 2026)       sell pack live. Isolation is the product. freeze-v1.
2026 Q4 – 2027 Q1    SIX_MONTH_FORWARD M0–M6 already landed (do not re-schedule).
2027 H1 leftover     CapTable / SYS_REVOKE only if shim tenants alias.
                     Blast clips as needed. Guest PCI BAR0 still optional.
                     Path-A IOVA host proof already landed (#78).
2027 H2              wait for a real partner opcode table
                     (v2 only as dual ACCEL.md + ireecp + host pack/unpack).
                     One port (RISC-V virtio-mmio or aarch64 GIC) only if
                     path B doorbell fails a partner ask.
                     Full MicroPerceptron port stays optional / later.
Through Sep 2027     sell the demos we can run. Do not invent silicon.
2028 (horizon)       signed list or ABI freeze — TWO_YEAR_PLAN, not this
                     year’s climax.
```

| When | Status | What a partner should hear |
| --- | --- | --- |
| **Now (Sep 2026)** | **Landed** | Sell pack live. Isolation is the product. Research `IreeHalCmd` v1 is frozen (freeze-v1). |
| **2026 Q4 – 2027 Q1** | **Landed** | [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) M0–M6. Heap refuse, MP shim, PJRT Add/Relu, GreenCtx interference, SoftCCT/Event fence counts, freeze-v1, diligence refresh. Port skipped. CapTable skipped (no alias). Do not re-schedule. |
| **2027 H1 leftover** | **Gated** | Per-task `CapTable` / `SYS_REVOKE` **only if** two shim tenants alias World `CPtr` slots. Blast-radius clips as needed. Guest PCI BAR0 bind still optional. Path-A IOVA host proof already landed (PR #78). |
| **2027 H2** | **Gated** | Wait for a **real** partner opcode table. Packet v2 only as a dual update of [ACCEL.md](ACCEL.md) + `drivers/src/ireecp.rs` + host pack/unpack. One port only if path B doorbell fails a partner ask. Full MicroPerceptron / virtio-accel port stays optional / later. |
| **Through Sep 2027** | **Sell** | Run the demos we can already run. Do not invent silicon. |
| **2028** | **Horizon** | Signed opcode list **or** freeze the research ABI. Pointed from [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md). Not this year’s climax. Not tape-out. |

---

## What a partner can run today

Host clips first. rustc 1.83+. No QEMU rebuild. Commands are
`Makefile` targets (`make help`). POSIX `/bin/sh` safe.

```bash
git clone https://github.com/amineux/aether.git
cd aether
make diligence-demo
make red-team
make partner-hello
make mp-shim
make design-win-standin
```

| Beat | Command | Grep needle |
| --- | --- | --- |
| Isolation | `make red-team` | `[redteam] attack=… result=refused` |
| Packet | `make partner-hello` | frozen `IreeHalCmd` → `IreeShapedCp`; bad exec refused |
| Third consumer | `make mp-shim` | MicroPerceptron-**shaped** thin consumer (PR #83). Inspiration name only. Not a port. |
| Wait | `make diligence-demo` | `[event] SoftChipletSync create/record/wait` |
| Event counts | `make diligence-demo` | `[event] fence counts chiplet-local vs package` |
| SoftCCT | `make diligence-demo` | `[softcct] package fences=` |
| GreenCtx interference | `make diligence-demo` | `[greenctx] interference partitioned 70/30 vs unpartitioned` |
| Admit class | `make red-team` | `[redteam] fabric-class admit/refuse` |
| Sandbox hole | `make red-team` | `[redteam] ATOMIC_ADD accept/reject` + `[softsfi] heap=refused` |
| Freeze-v1 proof | `make design-win-standin` | IREE HAL research stand-in — **not a partner**. Packet offsets stay. |
| Path-A IOVA | `make accel-test` | Soft-SMMU IOVA / wrong-SID. **No QEMU rebuild.** |
| Guest (optional) | `make qemu` | Path-B SoftNPU on stock QEMU |

Doorbell second consumer of the **same** frozen packet (not a
Makefile target): `cargo run -p aether-accel-client`.

Walk on the call: isolation → packet → wait → admit class → sandbox
hole. Script: [WEEK1_CALL.md](WEEK1_CALL.md). Site: `#sell`.

---

## Now — sell what landed

M0–M6 on [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) are **Done**.
Do not re-open them as new milestones.

- Sell pack + Week 1 call pack are live.
- SoftSFI heap refuse (PR #80): `[softsfi] heap=refused`. Not a bump
  allocator. Tensor stays `Unmodeled`.
- Thin MP-shaped consumer (PR #83): `make mp-shim`. Inspiration name
  only. Secondary to PJRT. Not a port.
- PJRT `Add` / `Relu` (PR #84) on the same frozen packet. Not FLOPs.
  Not `GetPjRtApi`.
- SoftGreenCtx interference clip (PR #85):
  `[greenctx] interference partitioned 70/30 vs unpartitioned`.
  Not HW MIG.
- SoftCCT / Event fence-count polish (PR #85):
  `[softcct] package fences=` + `[event] fence counts`. Not latency.
- **freeze-v1** (PR #86): research `IreeHalCmd` v1 stays. Magic
  `0xAE7E1EE1`, 96-byte LE, `backend = 4`, executable `0x0001EE00`.
  Freeze proof: `make design-win-standin`. Port **skipped** (path B
  doorbell has not failed a partner ask). CapTable **skipped** (no
  shim-tenant slot alias).

Path-A IOVA host proof already landed (PR #78). Kernel PCI BAR0 bind
is still optional. Stock `make qemu` stays path B.

---

## 2027 H1 leftover — gated only

Do **not** invent a CNode. Do **not** open a new ISA.

- **Per-task `CapTable` / `SYS_REVOKE`** — only if two shim tenants
  actually alias World `CPtr` slots. Today they do not.
  `aether-pjrt` / `aether-mp-shim` pin through a host Soft-SMMU walk;
  they do not mint World caps. Gate still closed.
- **Blast-radius clips** as needed (a two-tenant refuse that earns a
  new line). GreenCtx interference and SoftCCT/Event counts already
  landed.
- **Guest PCI BAR0 bind** stays optional. The host IOVA proof is
  enough for the call (`make accel-test`).

---

## 2027 H2 — wait for a real table

Do **not** invent a vendor opcode list to look busy.

- **Opcode table v2** only when a real partner brings a table, and
  only as a dual update of [ACCEL.md](ACCEL.md) **and**
  `drivers/src/ireecp.rs` **and** host pack/unpack in the same PR.
  A one-sided bump is a break.
- **One port** (RISC-V virtio-mmio **or** aarch64 GIC SoftNPU IRQ) —
  only if path B doorbell later fails a partner ask. Do not schedule
  both. Neither is a product-class second kernel. Gate stays.
- **Full MicroPerceptron / virtio-accel port** stays optional /
  later. The thin sketch (`make mp-shim`) already exists.

Until that table arrives, sell the demos above. Research v1 stays.

---

## Through Sep 2027 — sell, do not invent silicon

Every first meeting runs the host clips. Collect a filled
[DESIGN_WIN.md](DESIGN_WIN.md) or a written no. Leave the frozen
packet. Offer path-A IOVA as optional proof. Hold the 2028 stop
condition.

Hardware SMMU stays partner silicon. No FLOPs. No tape-out. No
booked ASIC bring-up.

---

## 2028 (horizon, not this year’s climax)

Pointed from [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md). Either a **signed
opcode list** from a real partner command processor replaces research
`IreeHalCmd`, or we **freeze the research ABI** and stop inventing
partners. Not a foundry date. Not tape-out. Not manufacturing.

---

## The ask

> Bring your opcode table. Fill DESIGN_WIN.md — opcode names, SID
> budget, memory spaces, queue count, event/fence scope — against
> frozen `IreeHalCmd`. If the HAL contract matches the chip, that is
> the conversation. If it does not, a written no with reasons is a
> good outcome.

Not a logo. Not an NDA draft in this meeting. Not NVIDIA.

---

## What we will not do this year

Same kill list as [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) /
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md). None of these get a
calendar slot in Sep 2026 → Sep 2027.

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
| Site-as-milestone | Progress refresh is a leave-behind, not a climax |
| FLOPs / tape-out | Standing non-claim. 2028 is an opcode list or an ABI freeze |
| Invented vendor table | Wait for a real partner CP. freeze-v1 stays until then |
| Product kernel / booked bring-up | Research prototype. Path B. Soft SMMU is software |

**Skip** (same list, said once): fork, POSIX open/read, Soft-SMMU
redo, CXL productization, ChipletFleet calendar, formal seL4,
SMMUv3 emu, UCIe PHY, PartnerNpuStub paint, site-as-milestone,
FLOPs/tape-out, invented vendor table, product kernel.

---

## Relationship to the other calendars

| File | Role |
| --- | --- |
| [YEAR2_PLAN.md](YEAR2_PLAN.md) | Historical Falsifier ACTIVE track through PR #37 + SpecForge appendix (aspirational). Do not fork. |
| [SIX_MONTH_PLAN.md](SIX_MONTH_PLAN.md) | Closed M1–M4 + SoftChipletSync record |
| [MONTH5_PLAN.md](MONTH5_PLAN.md) | Closed Month 5 record (four digests) |
| [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) | **Closed** Sep 2026 → Mar 2027 record (M0–M6 landed) |
| [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) | Horizon Sep 2026 → Sep 2028. 2028 is signed list or ABI freeze |
| [SELL_PACK.md](SELL_PACK.md) / [SELL_GOALS.md](SELL_GOALS.md) | What you get today / this-quarter demos |
| [WEEK1_CALL.md](WEEK1_CALL.md) / [PITCH.md](PITCH.md) | 20-minute pack / 8-minute script |
| [ROADMAP.md](ROADMAP.md) | Landed status, stubs, technical leftovers |
| **This file** | Next-12-month industry calendar Sep 2026 → Sep 2027 |
| Site | [https://amineux.github.io/aether/#sell](https://amineux.github.io/aether/#sell) · [https://amineux.github.io/aether/#roadmap](https://amineux.github.io/aether/#roadmap) |

---

## Non-claims (again, because this is a sell)

We will **not** claim:

- An NVIDIA partnership — or any booked ASIC / compiler bring-up
- FLOPs, or a benchmark vs Linux / seL4 / CUDA / any NPU SDK
- Tape-out, a foundry date, or manufacturing readiness
- Hardware SMMU (Soft SMMU is software)
- HW MIG (SoftGreenCtx is a fake 70/30 SM/WQ partition)
- Confidential GPU (SoftCmdFirewall is copy-then-validate)
- A PJRT plugin / `GetPjRtApi` / IREE driver / XLA
- A signed vendor ISA (research `IreeHalCmd` v1 is frozen, not signed)
- That `host/aether-mp-shim` is a MicroPerceptron port
- That a 2027 port is a product-class second kernel
- That this file, a filled DESIGN_WIN, or the live site is a design
  win in hand
