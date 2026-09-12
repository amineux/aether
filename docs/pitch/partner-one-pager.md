# Aether — partner one-pager

**Fabric kernel for AI silicon.** Isolation is the product (blast
radius), not FLOPs. Research prototype. Clone and run.

**Ask:** bring your opcode table.

**2028:** signed opcode list from a real partner CP, **or** freeze the
research ABI. Not tape-out. Not a foundry date.

Full leave-behind: [SELL_PACK.md](../SELL_PACK.md). Week 1 call:
[WEEK1_CALL.md](../WEEK1_CALL.md). Next year:
[YEAR_AHEAD.md](../YEAR_AHEAD.md). Site:
[https://amineux.github.io/aether/#sell](https://amineux.github.io/aether/#sell)
· [https://amineux.github.io/aether/#roadmap](https://amineux.github.io/aether/#roadmap).

---

## What you get today

| Surface | Honest reading |
| --- | --- |
| Frozen `IreeHalCmd` | 96-byte LE, magic `0xAE7E1EE1`, `backend = 4`. **freeze-v1** (research v1 stays). Not a signed vendor. |
| PJRT-shaped host nouns | Device / MemorySpace / Buffer / Executable / Event. Event create/record/wait on existing SoftChipletSync fences. Not `GetPjRtApi`. |
| Soft SMMU SID | STE→CD→Stage-1/2 software walk. SET_SID at submit. A real device can DMA past it. |
| Blast-radius refuse | Two tenants. CrossCut + wrong-SID abort. |
| Path-A IOVA | Host proof: non-identity IOVA, wrong SID aborts. Stock `make qemu` stays path B. |

---

## Commands that exist (`make help`)

```bash
make diligence-demo      # host Path B; [event] wait + counts; [softcct]; [greenctx] interference; no QEMU
make red-team            # named attacks + fabric-class + ATOMIC_ADD + heap refuse
make partner-hello       # frozen IreeHalCmd; no QEMU rebuild
make mp-shim             # MP-shaped thin consumer (inspiration name; not a port)
make design-win-standin  # IREE HAL research mapping — not a partner
make accel-test          # path-A Soft-SMMU IOVA / wrong-SID (host)
make qemu                # optional guest; path B SoftNPU
```

Walk: isolation (`make red-team`) → packet (`make partner-hello`) →
wait (`make diligence-demo` · `[event]`) → fence counts / GreenCtx
interference (`[softcct]` / `[greenctx] interference`) → admit class
(`make red-team` · `fabric-class`) → sandbox hole (`make red-team` ·
`ATOMIC_ADD`).

Doorbell (not a Makefile target): `cargo run -p aether-accel-client`.

---

## Next year (Sep 2026 → Sep 2027)

M0–M6 **landed**. Sell the demos. Wait for a real opcode table.
2028 (handoff or freeze) is the horizon, not this year’s climax.
Spine: [YEAR_AHEAD.md](../YEAR_AHEAD.md).

1. Every first meeting runs the host clips. No QEMU required.
2. Collect a filled [DESIGN_WIN.md](../DESIGN_WIN.md) — or a written no.
3. Leave the frozen packet. Path-A IOVA is optional (`make accel-test`).
4. Hold 2028: signed list or ABI freeze. Hardware SMMU is partner silicon.

---

## Will not claim

No NVIDIA partnership. No FLOPs. No tape-out. No hardware SMMU. No
HW MIG. No confidential GPU. Soft SMMU is software. Path B is
canonical. `PartnerNpuStub` is a labeled no-op. The IREE stand-in is
not a partner.

Fill the worksheet. Or pass. Not a logo.
