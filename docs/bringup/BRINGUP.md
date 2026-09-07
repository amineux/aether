# Soft SMMU bring-up kit

Optional M2 leave-behind. **Software tables only.** This is not a Soft-SMMU
redo, not a hardware SMMU, and not an SMMUv3 emulator. There is no MMIO
register file, no command queue, and no silicon SID.

The kit shows the walk a silicon team would watch on the same pins M1
already uses:

```text
AccelDevice::map  /  Soft-CP SID bind (ssid = 1)
        ↓
IommuMap  STE → CD (SSID) → Stage-1 → Stage-2
        ↓
ATS-shaped invalidate (software ATC; page tables stay)
```

M1 `IreeShapedCp` (`backend = 4`, `ssid = 2`) installs the same
`IommuMap` on a sibling CD of that STE. SoftNPU path B stays `ssid = 0`.

## What you get

| Piece | Where |
| --- | --- |
| This how-to | `docs/bringup/BRINGUP.md` |
| Replay script (one map / translate / abort-until-bound / wrong-stream / ATS sequence) | [`smmu_replay.jsonl`](smmu_replay.jsonl) |
| Golden software-table dump after that sequence | [`smmu_dump.golden.json`](smmu_dump.golden.json) |
| Dump pretty-printer | `scripts/smmu_dump.py` |
| Replay driver (validates the JSONL, prints the walk, optional host tests) | `scripts/smmu_replay.py` |
| Host-test replay | `aether_core::smmu_bringup::replay_jsonl` (`cargo test -p aether-core smmu_bringup`) |
| Soft-CP `AccelDevice::map` / SID bind twin | `cargo test -p aether-drivers fakecp -- bringup` |

`IommuMap::dump` copies STE / CD / S1 / S2 / ATC. It does not change the
walk. Replay feeds the host test: the JSONL is `include_str!`'d and
executed against a real `IommuMap`.

## How to run

From the repo root (host rustc is enough; no QEMU):

```bash
python3 scripts/smmu_replay.py          # print + check the JSONL
python3 scripts/smmu_dump.py            # pretty-print the golden dump
make smmu-bringup                       # scripts + the host tests below
cargo test -p aether-core --lib smmu_bringup
cargo test -p aether-drivers --lib fakecp -- bringup
```

`cargo test --workspace` already runs those tests. Regenerating the
golden dump (format change only):

```bash
GENERATE_SMMU_GOLDEN=1 cargo test -p aether-core --lib smmu_bringup::tests::replay_jsonl_map_translate_abort_wrong_stream_ats
```

Do not regenerate to “fix” a walk bug — that would be a Soft-SMMU redo.

## Sequence (what the JSONL does)

Packed SIDs are chiplet / tile / ssid, **not** PCIe BDF. The kit STE is
chiplet 0, tile 2:

| Alias | Packed | Meaning |
| --- | --- | --- |
| `cp` | `0x201` | Soft-CP `ssid = 1` (bind / map pin) |
| `iree` | `0x202` | M1 IreeShapedCp `ssid = 2` (same STE, no pin) |
| `wrong` | `0x203` | same STE, no CD |

Guest PA `0x1000`, length `0x1000`. `bind_nested` so Stage-1 IPA is
**not** the guest PA (`SOFT_SMMU_IPA_BASE`). Default SoftNPU `map` still
uses identity Stage-2; this kit opts into distinct IPA so both stages
are visible.

1. **capture** Soft-CP SID → `Captured`. DMA still aborts.
2. **translate** guest PA → `StreamAbort` (abort-until-bound).
3. **map** without Memory+MAP → `NoMemoryCap`.
   `AccelDevice::map` on Soft-CP is the same refuse (no cap walk).
4. **bind_nested** with Memory+MAP → `Bound`, Nested, distinct IPA.
5. **map** pin → IOVA `>= 0x1_0000_0000`, `iova != guest_pa`.
6. **walk** IOVA → STE→CD→S1→S2. `pa = 0x1000`, `ipa != pa`, Nested.
7. **translate** guest PA on the CP SID → the IOVA (hit).
8. **translate** `wrong` SSID → `StreamAbort` (no CD).
9. **bind_stream** IreeShapedCp SSID on the same STE (M1 pin).
10. **translate** that guest PA on the IREE SID → `WrongStream`.
11. **resolve_ats** fills the software ATC.
12. **invalidate Ats** drops the ATC line; **tables stay**.
13. **walk** still returns the guest PA (invalidate is not unmap).
14. **resolve_ats** refills the ATC (miss, then insert).
15. **dump** the software tables (golden JSON).

Expected dump (abridged; see the golden file):

```text
STE key=0x200  state=Bound  config=Nested  distinct_ipa=true
  CD ssid=1  S1  IOVA 0x100000000 → IPA 0x200000000
  CD ssid=2  (bound, empty S1 — M1 stream, no pin)
  S2         IPA 0x200000000 → PA 0x1000
  ATC        sid=0x201 IOVA→PA after the second resolve_ats
```

## Soft-CP / AccelDevice pins

`SoftCommandProcessor` (`backend = 3`) owns an `IommuMap`. Contract:

```text
AccelDevice::map(req)              → NoMemoryCap   // no cap walk
map_with_cap(Memory+MAP, req)      → IOVA          // bind + pin, ssid = 1
bind_stream(Memory+MAP, sid)       → Bound
submit / CpCmd::pack               → translate on that SID; unbound = Fault
```

That is SID-at-**map**, not SID-at-submit. Programming / validating
`StreamId` at the doorbell is **M3** (Host1x-shaped SET_SID; landed on
Soft-CP / IreeShapedCp). This kit does not redo that.

IreeShapedCp (`ssid = 2`) is the M1 packet path. It is bound in the
replay so a wrong-stream translate is visible on the M1 CD. It is not a
second SMMU.

## What we will not claim

- That these tables are a hardware SMMU or an SMMUv3 emulator
- That dump/replay programs a real SID / PT walk (partner silicon)
- That ATS invalidate is a device ATC or a command queue
- That Soft-CP is Host1x, XQueue, or a silicon CP
- That M1 `IreeShapedCp` is a signed vendor ISA
