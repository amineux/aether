# Week 1 partner call (20 minutes)

One folder. Clone this tree. Run the call. No QEMU rebuild. No new
kernel features. Research prototype.

One-page sell pack: [SELL_PACK.md](SELL_PACK.md) (printable:
[pitch/partner-one-pager.md](pitch/partner-one-pager.md)).
Eight-minute condensed script: [PITCH.md](PITCH.md). Host Path B clip
(and a captured run): [pitch/diligence-demo.log](pitch/diligence-demo.log).
Named-attack refuse clip: [pitch/red-team.log](pitch/red-team.log).
Clone-and-run leave-behind: [PARTNER.md](PARTNER.md). Blank worksheet:
[DESIGN_WIN.md](DESIGN_WIN.md). Filled IREE HAL research stand-in
(not a partner): [design-win/iree-hal-standin.md](design-win/iree-hal-standin.md).

**The ask (say it this way):** bring your opcode table.

Runnable demos this quarter: [SELL_GOALS.md](SELL_GOALS.md).
Near-term calendar: [SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md).

---

## Non-claims (read before the clock)

We will **not** claim:

- An **NVIDIA partnership** — or any booked ASIC / compiler bring-up.
  `PartnerNpuStub` is a labeled no-op. The filled worksheet in
  `docs/design-win/` is a **research stand-in**, not a signed vendor.
- **FLOPs** — no benchmark vs Linux / seL4 / CUDA / any NPU SDK.
- **Tape-out** — 2028 is an opcode list or an ABI freeze, not a foundry date.
- **Hardware SMMU** — Soft SMMU is software. A real device can DMA past it.
- **Path A as the demo** — Path B (stock `make qemu`, in-kernel SoftNPU
  BAR) is canonical. This call runs host clips only. Path A is optional
  (`make accel-test` / `make qemu-accel`); see [ACCEL.md](ACCEL.md).

Also not claimed: a PJRT plugin, an IREE driver, MIG-class isolation,
confidential GPU, or that [DESIGN_WIN.md](DESIGN_WIN.md) is a contract.

---

## Prep (async, before the call)

```bash
git clone https://github.com/amineux/aether.git
cd aether
# rustc 1.83+ on the host. No QEMU required for this slot.
make diligence-demo
make red-team
```

If cargo is cold on the laptop, walk the captured logs instead of
waiting on the first compile. Same golden lines CI greps.

Optional, not this slot: `make partner-hello`, `make design-win-standin`,
`make qemu`. Doorbell second consumer (not a Makefile target):
`cargo run -p aether-accel-client`.

**What to show next** (same tree; commands that exist): isolation
(`make red-team`) → packet (`make partner-hello` / doorbell) → wait
(Event line in `make diligence-demo`) → admit class (fabric-class
line in `make red-team`) → sandbox hole (`ATOMIC_ADD` +
`[softsfi] heap=refused` in `make red-team`). Map below.

---

## Clock

| Min | Beat | Point at |
| --- | --- | --- |
| 0:00–2:00 | Frame | Isolation is the product. Non-claims block above. |
| 2:00–8:00 | Host Path B | `make diligence-demo` / [pitch/diligence-demo.log](pitch/diligence-demo.log) |
| 8:00–13:00 | Buyer stdout | `make red-team` / [pitch/red-team.log](pitch/red-team.log) |
| 13:00–17:00 | Frozen packet | [PARTNER.md](PARTNER.md), [DESIGN_WIN.md](DESIGN_WIN.md), [design-win/iree-hal-standin.md](design-win/iree-hal-standin.md) |
| 17:00–20:00 | Ask | “Bring your opcode table.” Fill DESIGN_WIN or a written no. |

If the slot is twelve minutes, keep the non-claims, run only
`make diligence-demo` (or the captured log), and close on the ask.
The 8-minute script in [PITCH.md](PITCH.md) is the same thesis.

---

## What to show next

If they want the rest of the stack after the clock, walk this order.
Host clips only. No new kernel feature. No FLOPs. No invented
Makefile targets.

| Beat | Point at | Command that exists |
| --- | --- | --- |
| Isolation | named attacks refused | `make red-team` |
| Packet | frozen `IreeHalCmd`; second consumer | `make partner-hello` then `cargo run -p aether-accel-client` / `make mp-shim` |
| Wait | Event create/record/wait on SoftChipletSync | `make diligence-demo` — grep `[event] SoftChipletSync create/record/wait` |
| Admit class | fabric-class tag into SoftNoI | `make red-team` — grep `[redteam] fabric-class admit/refuse` |
| Sandbox hole | SoftSFI `ATOMIC_ADD` accept/reject; heap named refuse | `make red-team` — grep `[redteam] ATOMIC_ADD accept/reject` and `[softsfi] heap=refused` |

Worksheet still: `make design-win-check` / `make design-win-standin`.
Blank: [DESIGN_WIN.md](DESIGN_WIN.md). Filled IREE stand-in (not a
partner): [design-win/iree-hal-standin.md](design-win/iree-hal-standin.md).

---

## 0:00–2:00 — Frame

**Say:**

> Linux still sees an NPU as a PCIe endpoint. That fails on a package
> of CPU, NPU, GPU, and ASIC tiles. Isolation is the product — blast
> radius, who can name a tile, a bank, a stream. FLOPs stay in their
> compiler.

Then one breath on what ships: frozen 96-byte `IreeHalCmd` (magic
`0xAE7E1EE1`), public IREE HAL nouns, Soft SMMU SID-at-submit
(software). Path B is canonical. Not a vendor ISA.

---

## 2:00–8:00 — Host Path B

```bash
make diligence-demo
```

Captured stdout: [pitch/diligence-demo.log](pitch/diligence-demo.log).
CI greps [`examples/diligence-demo/expected.txt`](../examples/diligence-demo/expected.txt).

While it prints:

> Two tenants. CrossCut and the other SID are refused. Frozen
> `IreeHalCmd` submit + wait — research opcodes, not FLOPs.
> Mutation-during-validate fails: command-stream integrity, not
> confidential GPU. SoftGreenCtx is a 70/30 software partition, not
> HW MIG. Event create/record/wait sits on SoftChipletSync fences
> already in the tree — grep `[event] SoftChipletSync`. Soft SMMU
> is software. Path B.

Do **not** show a FLOP number. Do **not** attach `-device aether-accel`.

---

## 8:00–13:00 — Named attacks refused

```bash
make red-team
```

Captured stdout: [pitch/red-team.log](pitch/red-team.log). Makefile greps:

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

Same refuse paths the kernel already has. Not a new isolator.
`fabric-class`, `ATOMIC_ADD`, and heap refuse are the same SoftNoI /
SoftSFI clips — admit/refuse and accept/reject on code that already
exists. Heap is a named `Unmodeled` fault, not a bump allocator.

---

## 13:00–17:00 — Frozen packet and the worksheet

Walk three pages, in this order:

1. **[PARTNER.md](PARTNER.md)** — `make partner-hello`. Host
   `IreeHalCmd` → `IreeShapedCp`. No QEMU rebuild. Magic `0xAE7E1EE1`,
   `executable = 0x0001EE00`, `backend = 4`. Offsets stay frozen.
2. **[DESIGN_WIN.md](DESIGN_WIN.md)** — blank they fill: opcode names,
   SID budget, memory spaces, queue count, event/fence scope.
3. **[design-win/iree-hal-standin.md](design-win/iree-hal-standin.md)** —
   a filled **research stand-in** from public IREE HAL nouns already
   cited in [ACCEL.md](ACCEL.md). Not a partner. `make design-win-standin`
   admits the TOML twin. TRANSFER-only is reserved / refused. SID
   budget is a software pool (`SID_BUDGET_PER_TENANT = 4`).

Do not change packet offsets on the call.

---

## 17:00–20:00 — Ask

**Say:**

> Bring your opcode table. Fill DESIGN_WIN.md against frozen
> `IreeHalCmd`. If the HAL contract matches the chip, that is the
> conversation. If it does not, a written no with reasons is a good
> outcome.

Follow-up that is honest: a filled worksheet, or a pass. Not a logo.
Not an NDA draft in this meeting. Not NVIDIA.

---

## After the call

| They asked | Point at |
| --- | --- |
| Run it without QEMU | `make diligence-demo` then `make red-team` (logs above) |
| What to show next | isolation → packet → wait → admit class → sandbox hole (table above) |
| Clone-and-run packet | [PARTNER.md](PARTNER.md) / `make partner-hello` |
| Doorbell second consumer | `cargo run -p aether-accel-client` (not a Makefile target) |
| MP-shaped thin consumer | `make mp-shim` (inspiration name only; secondary to PJRT; PR #83) |
| PJRT Add/Relu | `cargo test -p aether-pjrt` (frozen packet `function` 2 / 3; PR #84) |
| Event wait | `make diligence-demo` — `[event] SoftChipletSync create/record/wait` |
| Fabric-class admit | `make red-team` — `[redteam] fabric-class admit/refuse` |
| ATOMIC_ADD hole | `make red-team` — `[redteam] ATOMIC_ADD accept/reject` |
| SoftSFI heap refuse | `make red-team` — `[softsfi] heap=refused` |
| Fill the opcode map | [DESIGN_WIN.md](DESIGN_WIN.md) + `make design-win-check` |
| Example mapping | [design-win/iree-hal-standin.md](design-win/iree-hal-standin.md) + `make design-win-standin` |
| How to plug a CP | [ACCEL.md](ACCEL.md) + `aether_hal::AccelDevice` |
| Path A IOVA (optional BAR) | [ACCEL.md](ACCEL.md) path-A ADR + `make accel-test` / [qemu/README.md](../qemu/README.md). Stock `make qemu` stays B. Do not rebuild QEMU. |
| Isolation invariants | [SECURITY.md](SECURITY.md), [BLAST.md](BLAST.md) |
| What is stubbed | [DILIGENCE.md](DILIGENCE.md) |
| One-page sell pack | [SELL_PACK.md](SELL_PACK.md) |
| 8-minute script | [PITCH.md](PITCH.md) |
| 60–90 min silicon agenda | [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md) |
