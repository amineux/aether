# Sell pack — what you get today

One page. Clone this tree. Run the named Makefile targets. Research
prototype. Isolation is the product (blast radius), not FLOPs.

**Ask:** bring your opcode table.

**2028:** a signed opcode list from a real partner command processor,
**or** freeze the research ABI and stop inventing partners. Not a
foundry date. Not tape-out.

Week 1 20-minute pack: [WEEK1_CALL.md](WEEK1_CALL.md). Eight-minute
script: [PITCH.md](PITCH.md). Printable cut:
[pitch/partner-one-pager.md](pitch/partner-one-pager.md). This-quarter
demos: [SELL_GOALS.md](SELL_GOALS.md). Near-term calendar:
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) (Sep 2026 → Mar 2027).
Site:
[https://amineux.github.io/aether/#sell](https://amineux.github.io/aether/#sell).

---

## Non-claims (read first)

We will **not** claim: an NVIDIA partnership (or any booked ASIC /
compiler bring-up); FLOPs or a benchmark vs Linux / seL4 / CUDA / any
NPU SDK; tape-out; hardware SMMU (Soft SMMU is software); HW MIG
(SoftGreenCtx is a fake 70/30 SM/WQ partition); confidential GPU
(SoftCmdFirewall is copy-then-validate); a PJRT plugin; an IREE
driver; that [DESIGN_WIN.md](DESIGN_WIN.md) is a signed vendor
contract; that the IREE HAL stand-in is a partner.

`PartnerNpuStub` is a labeled no-op. Path B (stock `make qemu`,
in-kernel SoftNPU BAR) is canonical.

---

## Clone and run

Host clips first. No QEMU rebuild. rustc 1.83+. Commands below are
`Makefile` targets (`make help`). POSIX `/bin/sh` safe — no
`pipefail`.

```bash
git clone https://github.com/amineux/aether.git
cd aether
make diligence-demo
make red-team
make partner-hello
make mp-shim
make design-win-standin
```

| Beat | Command that exists | What you see |
| --- | --- | --- |
| Isolation | `make red-team` | `[redteam] attack=… result=refused` |
| Packet | `make partner-hello` | frozen `IreeHalCmd` → `IreeShapedCp`; bad exec refused |
| Third consumer | `make mp-shim` | MicroPerceptron-**shaped** thin consumer (PR #83). Inspiration name only. Not a port. |
| Wait | `make diligence-demo` | `[event] SoftChipletSync create/record/wait` |
| Admit class | `make red-team` | `[redteam] fabric-class admit/refuse` |
| Sandbox hole | `make red-team` | `[redteam] ATOMIC_ADD accept/reject` + `[softsfi] heap=refused` |
| Worksheet | `make design-win-check` | admits the filled [DESIGN_WIN.md](DESIGN_WIN.md) sample |
| IREE stand-in | `make design-win-standin` | admits public IREE HAL nouns — **not a partner** |
| Path-A IOVA | `make accel-test` | host Soft-SMMU IOVA / wrong-SID; **no QEMU rebuild** |
| Guest (optional) | `make qemu` | path-B SoftNPU on stock QEMU |

Doorbell second consumer of the **same** frozen packet (not a
Makefile target): `cargo run -p aether-accel-client`.

Optional attach of `-device aether-accel` is `make qemu-accel` **only**
when `QEMU_ACCEL` names a patched binary. Stock `make qemu` stays
path B. Default CI does not rebuild QEMU. See [ACCEL.md](ACCEL.md)
and [qemu/README.md](../qemu/README.md).

---

## What ships (post #73 / #74 / #75 / #77 / #78 / #80 / #83 / #84)

| Surface | Honest reading | Proof |
| --- | --- | --- |
| Frozen `IreeHalCmd` | 96-byte LE, magic `0xAE7E1EE1`, `backend = 4`. Research opcodes. Not a signed vendor ISA. | `make partner-hello` |
| PJRT-shaped host nouns | Device / MemorySpace / Buffer / Executable / Event. Event create/record/wait on **existing** SoftChipletSync fences (PR #74). Not `GetPjRtApi`. | `[event]` in `make diligence-demo` |
| Soft SMMU SID-at-submit | STE→CD→Stage-1/2 software walk. SET_SID at the job head. A real device can DMA past it. | `[blast]` / `[sid]` |
| Blast-radius refuse | Two tenants. CrossCut + wrong-SID abort. | `make red-team` · `wrong-sid-crosscut` |
| SoftCmdFirewall | Copy-then-validate. Command-stream integrity, not confidential GPU. | `make red-team` · `softcmdfirewall` |
| SoftGreenCtx | Fake 70/30 SM/WQ partition + interference vs unpartitioned (integer milli). Not HW MIG. | `[greenctx]` 70/30 + `interference partitioned` in `make diligence-demo` |
| SoftCCT / Event counts | Package ≪ broadcast (`1` vs `10`). Chiplet-local vs package. Not latency. | `[softcct] package fences=` + `[event] fence counts` |
| SoftNoI-IS + fabric-class | Fake shared NoI; refuse `IS > 1.5`. Software fabric-class tag (PR #75): Gradient admits, second Curl refuses the reserved ring. Not topology synth. | `make red-team` · `softnoi-is` + `fabric-class` |
| SoftSFI `ATOMIC_ADD` | SID-proved toy fetch-add (PR #73). In-range accept; cross-tenant `Oob`. Tensor stays `Unmodeled`. Not a hardware atomic. | `make red-team` · `ATOMIC_ADD` |
| SoftSFI heap refuse | Named `SoftOp::Heap` is `SfiError::Unmodeled` (PR #80). Not a bump allocator. | `make red-team` · `[softsfi] heap=refused` |
| MP-shaped shim | Thin consumer of the **same** frozen image (PR #83). `make mp-shim`. Inspiration name only. Secondary to PJRT. Not a MicroPerceptron port. | `make mp-shim` |
| PJRT Add / Relu | Extra research opcodes on the frozen packet (PR #84). Still not FLOPs. Still not a plugin. | `host/aether-pjrt` |
| Path-A IOVA | Job wire carries non-identity IOVAs; DMA walks ssid 4; wrong SID aborts (PR #78). Host `PathABar`. Kernel PCI bind still optional. | `make accel-test` |
| Week 1 call pack | Isolation → packet → wait → admit class → sandbox hole (PR #77). | [WEEK1_CALL.md](WEEK1_CALL.md) |

SoftNoI-IS, PASID/SVA, and OperatorInject **landed** as H2 2026
explorations (PRs #60 / #62 / #61). They are not “in flight.”

---

## Six-month sell goals (Sep 2026 → Mar 2027)

Process goals. Not a product kernel. Not a booked lab.

1. **Every first meeting runs the host clips.** `make diligence-demo`
   then `make red-team`. No QEMU required. Captured logs live in
   [pitch/](pitch/) if cargo is cold.
2. **Collect an opcode table — or a written no.** Fill
   [DESIGN_WIN.md](DESIGN_WIN.md) against frozen `IreeHalCmd`. The
   IREE HAL stand-in (`make design-win-standin`) is the research
   mapping, not a logo.
3. **Walk the five beats on the same tree.** Isolation → packet →
   wait → admit class → `ATOMIC_ADD`. Commands above exist today.
4. **Leave the packet.** `make partner-hello` is clone-and-run.
   Doorbell is a second consumer of the same 96-byte image.
5. **Offer path-A IOVA as optional proof.** `make accel-test`. Stock
   `make qemu` stays B. Do not rebuild QEMU on the call.
6. **Hold the 2028 stop condition.** Signed opcode list **or** ABI
   freeze. Hardware SMMU stays partner silicon. No FLOPs. No tape-out.

Open on the kernel calendar ([TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md)):
guest PCI BAR0 bind; gated `CapTable` only if two shim tenants alias
slots; **one** port if path B’s doorbell fails. Those are not this
page’s asks. PJRT Add/Relu, GreenCtx interference, and SoftCCT/Event
counts already landed.

---

## The ask

> Bring your opcode table. Fill DESIGN_WIN.md — opcode names, SID
> budget, memory spaces, queue count, event/fence scope — against
> frozen `IreeHalCmd`. If the HAL contract matches the chip, that is
> the conversation. If it does not, a written no with reasons is a
> good outcome.

Not a logo. Not an NDA draft in this meeting. Not NVIDIA.

---

## After this page

| They asked | Point at |
| --- | --- |
| 20-minute Week 1 pack | [WEEK1_CALL.md](WEEK1_CALL.md) |
| 8-minute script | [PITCH.md](PITCH.md) |
| Diligence + non-claims | [DILIGENCE.md](DILIGENCE.md) |
| How to plug a CP | [ACCEL.md](ACCEL.md) + `aether_hal::AccelDevice` |
| Blank worksheet | [DESIGN_WIN.md](DESIGN_WIN.md) + `make design-win-check` |
| IREE HAL stand-in | [design-win/iree-hal-standin.md](design-win/iree-hal-standin.md) + `make design-win-standin` |
| Clone-and-run packet | [PARTNER.md](PARTNER.md) / `make partner-hello` |
| Path-A IOVA notes | [ACCEL.md](ACCEL.md) + `make accel-test` / `make qemu-accel` |
| 2028 calendar | [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md) |
