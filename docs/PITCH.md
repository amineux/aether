# 8-minute pitch (presenter script)

A founder-readable walkthrough for a live call. Not a kernel feature.
Not a press quote. Research prototype.

**What this is.** Interfaces and refuse invariants we would show an
AI-chip OS team. Isolation is the product (blast radius), not FLOPs.

**What this is not.** A NVIDIA partnership, a FLOP number, a tape-out,
hardware SMMU, MIG-class isolation, or confidential GPU. Those words
do not appear as claims below.

Longer technical session: [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md)
(60–90 min script; that meeting has not happened). One-page sell
leave-behind: [SELL_PACK.md](SELL_PACK.md) (printable:
[pitch/partner-one-pager.md](pitch/partner-one-pager.md); site
`#sell`). Diligence pack: [DILIGENCE.md](DILIGENCE.md). Call
worksheet: [DESIGN_WIN.md](DESIGN_WIN.md). Week 1 20-minute pack:
[WEEK1_CALL.md](WEEK1_CALL.md). Calendar:
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) (Sep 2026 → Mar 2027);
horizon [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md). Serial lines you can point at:
[pitch/transcript.txt](pitch/transcript.txt) (copied from in-tree
prints / golden greps). Captured host runs:
[pitch/diligence-demo.log](pitch/diligence-demo.log),
[pitch/red-team.log](pitch/red-team.log).

**Proof commands that exist** (`Makefile` `help`):
`make diligence-demo`, `make red-team`, `make design-win-check`,
`make design-win-standin`, `make partner-hello`, `make mp-shim`,
`make accel-test`, `make qemu`.
[`docs/PARTNER.md`](PARTNER.md) is the partner landing page.
Doorbell second consumer (not a Makefile target):
`cargo run -p aether-accel-client`.

Post-#73 / #74 / #75 / #77 / #78 / #80 / #83 / #84: SoftSFI
`ATOMIC_ADD` is SID-proved; heap/alloc is a named refuse
(`[softsfi] heap=refused`); Event create/record/wait sits on
existing SoftChipletSync fences; fabric-class tags feed SoftNoI
admit; Week 1 pack walks those slices; path-A Soft-SMMU IOVA is a
host proof (`make accel-test`); `make mp-shim` is a
MicroPerceptron-shaped thin consumer (not a port); PJRT Add / Relu
are extra research opcodes on the frozen packet (not FLOPs).
SoftNoI-IS **landed** (PR #60) — not in flight.

**What to show next** (same tree): isolation (`make red-team`) →
packet (`make partner-hello` / doorbell) → wait (Event line in
`make diligence-demo`) → admit class (fabric-class line in
`make red-team`) → sandbox hole (`ATOMIC_ADD` + `[softsfi] heap=refused`
in `make red-team`).

---

## Clock

| Min | Beat | One line |
| --- | --- | --- |
| 0:00–1:30 | Problem | Multi-tenant accelerator packages need a fabric kernel, not a bigger Linux driver. |
| 1:30–3:00 | What ships | Frozen `IreeHalCmd` + PJRT-shaped host nouns + Soft SMMU SID-at-submit. |
| 3:00–5:00 | Live proof | `make diligence-demo` then `make red-team`. |
| 5:00–6:30 | Will not claim | Partnership, FLOPs, tape-out, HW SMMU, MIG, confidential GPU. |
| 6:30–8:00 | 2028 close | A real opcode list **or** an ABI freeze. Ask: “bring your opcode table.” |

If the slot is five minutes, keep beats 1, 4, and 5, and run only
`make diligence-demo` (no QEMU).

---

## 0:00–1:30 — Problem

**Say:**

> Linux still sees an NPU as a PCIe endpoint: ioctl, a userspace
> runtime, and a hope the driver flushed the right caches. That model
> is already strained on a discrete GPU. It fails on a **package** of
> CPU, NPU, GPU, and ASIC tiles.

**Then the inversion, in one breath:**

> Inference latency is a deadline, not a best-effort ioctl. Weights
> and KV caches are multi-tenant secrets, not files. “Shared memory”
> across dies is often not cache-coherent. The scarce resource is HBM
> banks and NPU waves, not CPU time slices.

**The product sentence (do not skip):**

> Isolation is the product. Blast radius — who can name a tile, a
> bank, a stream — is what we would sell a silicon OS team. FLOPs stay
> in their compiler.

Aether’s bet: **compute tiles are first-class peers** on a
capability-secured message fabric. CPU threads and accelerator waves
are the same kind of scheduled job. There is no IPC except
capability-checked messages. UCIe / EMIB / UALink move bytes; they
are not a programming model.

Do **not** open with a benchmark. There isn’t one in this tree.

---

## 1:30–3:00 — What ships

**Frame:** research opcodes, public IREE HAL nouns, **not** a signed
vendor. `PartnerNpuStub` is a leftover no-op sketch.

Three surfaces, in this order:

1. **Frozen `IreeHalCmd`.** 96-byte little-endian packet, magic
   `0xAE7E1EE1`, `backend = 4` (`IreeShapedCp`). Offsets and widths
   are locked in [ACCEL.md](ACCEL.md). Changing an offset is a dual
   update of that ADR, `drivers/src/ireecp.rs`, and host pack/unpack.
   Research opcodes until 2028 says otherwise. Not an IREE runtime.
   Not a signed vendor ISA. A second host consumer of the same packet
   is `examples/accel-client` (`cargo run -p aether-accel-client`) —
   a doorbell sketch, not a Makefile target. A MicroPerceptron-shaped
   thin consumer (`make mp-shim` / `host/aether-mp-shim`; **landed**
   PR #83) is a research sketch — inspiration name only, secondary to
   PJRT, not a port. `Add` / `Relu` pack into the same frozen packet
   (`function` 2 / 3; **landed** PR #84). Offsets unchanged.
2. **PJRT-shaped host nouns.** `host/aether-pjrt` speaks Device,
   MemorySpace, Buffer, Executable, Event — public PJRT / IREE HAL
   vocabulary, cited, not claimed. The crate packs frozen `IreeHalCmd`
   onto `IreeShapedCp`. It is **not** `GetPjRtApi`, **not**
   `iree_hal_driver_t`, **not** a plugin. Compilers keep the ISA blob
   and the graph IR. The kernel is a submission shim + resource solver.
   Walkthrough: [HOST.md](HOST.md), [ABI.md](ABI.md).
3. **Soft SMMU SID-at-submit.** Stream ID is armed at the job head,
   not only at map. Host1x-shaped SET_SID; SID sticks on the Soft-CP
   XQueue. Soft SMMU is a software STE→CD→Stage-1/2 walk + ATS-shaped
   invalidate. A real device can still DMA past it. Hardware SMMU
   needs partner silicon. Not a Tegra driver.

One more sentence if they ask “is it a kernel?”:

> Yes — a bootable v0.1. `make qemu` is path-B SoftNPU on stock QEMU
> (in-kernel BAR). Caps, typed places, SpectralCut refuse, and the
> diligence clips run in the guest self-check. It is not production
> silicon.

---

## 3:00–5:00 — Live proof

**On a call, run the named Makefile targets.** Host clips first
(no QEMU). Captured stdout lives next to this script if cargo is cold.

### Host Path B (default — no QEMU)

```bash
make diligence-demo
```

Partner clip: host Path B, no QEMU rebuild. CI greps
[`examples/diligence-demo/expected.txt`](../examples/diligence-demo/expected.txt).
Same binary: `cargo diligence-demo`. Captured run:
[pitch/diligence-demo.log](pitch/diligence-demo.log).

What to say while it prints:

> Two tenants. CrossCut and the other SID are refused. Frozen
> `IreeHalCmd` submit + wait — research opcodes, not FLOPs.
> Mutation-during-validate fails: command-stream integrity, not
> confidential GPU. SoftGreenCtx is a 70/30 software partition, not
> HW MIG — `[greenctx] interference partitioned 70/30 vs unpartitioned`
> (integer milli; residual shared-HBM tax). Event create/record/wait
> sits on SoftChipletSync fences already in the tree
> (`[event] SoftChipletSync create/record/wait` plus
> `[event] fence counts chiplet-local vs package`). SoftCCT package
> ≪ broadcast (`[softcct] package fences=`). Then the proves /
> does-not block. Soft SMMU is software.

Then the buyer stdout:

```bash
make red-team
```

Captured run: [pitch/red-team.log](pitch/red-team.log). Named attacks
refused. Makefile greps:

```
[redteam] attack=wrong-sid-crosscut result=refused
[redteam] attack=softcmdfirewall result=refused
[redteam] attack=softsfi-oob result=refused
[redteam] attack=softnoi-is result=refused
[redteam] attack=pasid-stale result=refused
[redteam] fabric-class admit/refuse
[redteam] ATOMIC_ADD accept/reject
[softsfi] heap=refused
[redteam] what this is not: confidential GPU; not HW MIG; Soft SMMU is software
[redteam] sealed
```

Expected lines: [pitch/transcript.txt](pitch/transcript.txt).

### Optional on the same call

```bash
make design-win-check
```

Admits the filled [DESIGN_WIN.md](DESIGN_WIN.md) sample (unknown
executable id, SID 0, and TRANSFER-only are **refused**). Same as
`cargo run -p aether-design-win-check`. IREE HAL research stand-in
(not a partner): `make design-win-standin`.

```bash
make partner-hello
```

Host `IreeHalCmd` leave-behind. No QEMU rebuild. See [PARTNER.md](PARTNER.md).

```bash
make accel-test
```

Path-A Soft-SMMU IOVA / wrong-SID on the host (PR #78). No QEMU
rebuild. Stock `make qemu` stays path B. Optional
`make qemu-accel` attaches `-device aether-accel` only when
`QEMU_ACCEL` names a patched binary.

```bash
make qemu
```

Path-B guest serial. `make qemu-ci` is the 45s CI wrapper. Use this
if they want the bootable slice; skip it if the laptop has no QEMU.

Doorbell second consumer of frozen `IreeHalCmd` (not in `make help`):

```bash
cargo run -p aether-accel-client
```

Do **not** show a FLOP number. Do **not** attach `-device aether-accel`
unless `QEMU_ACCEL` names a patched binary (`make qemu-accel` says so).
Stock `make qemu` stays path B.

---

## 5:00–6:30 — What we will not claim

Read this list. Pause after the first line.

We will **not** claim:

- **NVIDIA partnership** — or any ASIC house, compiler project, or
  booked bring-up. `PartnerNpuStub` is a labeled no-op.
- **FLOPs** — no benchmark vs Linux / seL4 / CUDA / any NPU SDK.
  SoftGreenCtx interference is integer throughput units, not FLOPs.
- **Tape-out** — no manufacturing climax. 2028 is not a foundry date.
- **Hardware SMMU** — Soft SMMU is software. Partner silicon required.
- **MIG-class isolation** — SoftGreenCtx is a fake 70/30 SM/WQ
  partition on Soft-CP. Not HW MIG, not a BAR firewall.
- **Confidential GPU** — SoftCmdFirewall is copy-then-validate of the
  command stream. Not HBM encryption, not GPU-CC, not SEC2.

Also not claimed in the same breath: a PJRT plugin, an IREE driver, a
signed vendor ISA, ARM SVA / PCIe PASID / CUDA UVA, UCIe, a coherence
protocol, seL4 proofs, a product-class second architecture
(RISC-V / aarch64 `/init` are documented subsets), or that
`DESIGN_WIN.md` is a signed vendor contract.

If they push on any of those, point at [DILIGENCE.md](DILIGENCE.md)
non-claims and stop. Do not paper over it.

---

## 6:30–8:00 — Two-year close

**Calendar:** [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md)
(Sep 2026 → Mar 2027). Horizon: [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md)
(Sep 2026 → Sep 2028). Sell demos: [SELL_GOALS.md](SELL_GOALS.md).

**Say:**

> 2027 deepens what already shipped: PJRT `Add` / `Relu` on the frozen
> packet (PR #84), SoftSFI widen with honest leftovers (`atomic_add`
> SID-proved; tensor `Unmodeled`; heap named refuse, PR #80), a
> MicroPerceptron-shaped thin consumer of the **frozen** packet
> (PR #83). M5–M6 named freeze-v1: research `IreeHalCmd` v1 stays
> until a real partner table forces a dual update of ACCEL.md,
> `ireecp`, and host pack/unpack. Port skipped. Hardware SMMU is not
> a software milestone.

**Then the stop condition:**

> 2028 is a real opcode list or an ABI freeze. Either a signed list
> from a partner command processor replaces research `IreeHalCmd`, or
> we freeze the research packet and stop inventing partners. There is
> no manufacturing climax.

**The ask (say it this way):**

> Bring your opcode table. Fill [DESIGN_WIN.md](DESIGN_WIN.md) —
> opcode names, SID budget, memory spaces, queue count, event/fence
> scope — against frozen `IreeHalCmd`. If the HAL contract matches
> the chip, that is the conversation. If it does not, a written no
> with reasons is a good outcome.

Follow-up that is honest: a filled worksheet, or a pass. Not a logo.
Not an NDA draft in this meeting. A research mapping (not a partner)
is [design-win/iree-hal-standin.md](design-win/iree-hal-standin.md).

---

## After the call

| They asked | Point at |
| --- | --- |
| One-page leave-behind | [SELL_PACK.md](SELL_PACK.md) / [pitch/partner-one-pager.md](pitch/partner-one-pager.md) |
| Run it without QEMU | `make diligence-demo` then `make red-team` |
| What to show next | isolation → packet → wait → admit class → sandbox hole |
| 20-minute Week 1 pack | [WEEK1_CALL.md](WEEK1_CALL.md) |
| Doorbell second consumer | `cargo run -p aether-accel-client` (not a Makefile target) |
| MP-shaped thin consumer | `make mp-shim` (`host/aether-mp-shim`; inspiration name only; PR #83) |
| PJRT Add/Relu | `cargo test -p aether-pjrt` (frozen packet `function` 2 / 3; PR #84) |
| Event / GreenCtx / SoftCCT / fabric-class / ATOMIC_ADD / heap | Event wait + fence counts + GreenCtx interference + `[softcct] package fences=` in `make diligence-demo`; fabric-class / ATOMIC_ADD / heap in `make red-team` (`[softsfi] heap=refused`) |
| Fill the opcode map | [DESIGN_WIN.md](DESIGN_WIN.md) + `make design-win-check` |
| IREE HAL stand-in | [design-win/iree-hal-standin.md](design-win/iree-hal-standin.md) + `make design-win-standin` |
| How to plug a CP | [ACCEL.md](ACCEL.md) driver steps + `aether_hal::AccelDevice` |
| Compiler boundary | [HOST.md](HOST.md), [ABI.md](ABI.md) |
| Isolation invariants | [SECURITY.md](SECURITY.md), [BLAST.md](BLAST.md) |
| What is stubbed | [DILIGENCE.md](DILIGENCE.md), [ROADMAP.md](ROADMAP.md) |
| Next six months | [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) / [SELL_GOALS.md](SELL_GOALS.md) |
| Next two years | [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) |
| 60–90 min silicon agenda | [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) |
| Partner landing page | [PARTNER.md](PARTNER.md) / `make partner-hello` |

One-page sell pack: [SELL_PACK.md](SELL_PACK.md).
Site sell / design-win: `#sell`. Pitch beats: `#pitch`.
Why-partner proofs: `#partners`.
