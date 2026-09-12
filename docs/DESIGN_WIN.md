# Design-win worksheet

This is the **artifact a partner fills in on a call** — opcode names, SID
budget, memory spaces, queue count, event/fence scope — mapped onto the
research ABI so it reads as a product interface.

It is **not** a signed vendor contract, a partnership announcement, a
tape-out checklist, or an IREE / PJRT plugin. `PartnerNpuStub` stays a
labeled no-op. The filled sample in
[`examples/design-win-check`](../examples/design-win-check) is a host
fixture that **refuses** an unknown executable id, SID 0, and a
TRANSFER-only packet. That is the proof the worksheet is executable,
not a PDF. A filled **research stand-in** (public IREE HAL nouns
already frozen on `IreeHalCmd`, **not a partner**) is
[`docs/design-win/iree-hal-standin.md`](design-win/iree-hal-standin.md)
(`make design-win-standin`).

Partner landing page: [`docs/PARTNER.md`](PARTNER.md) (`make partner-hello`).
Week 1 call pack: [`docs/WEEK1_CALL.md`](WEEK1_CALL.md).
One-page sell pack: [`docs/SELL_PACK.md`](SELL_PACK.md).

Public vocabulary is cited from IREE HAL headers on
[iree-org/iree](https://github.com/iree-org/iree)
(`runtime/src/iree/hal/{device,command_buffer,buffer,buffer_view,executable,semaphore}.h`)
and from OpenXLA PJRT. Aether does not claim those projects.

**Do not change frozen `IreeHalCmd` offsets.** The v1 image is the
architectural contract ([ACCEL.md](ACCEL.md) ADR). **M5–M6
freeze-v1:** research `IreeHalCmd` v1 stays until a real partner
table forces a dual update of `drivers/src/ireecp.rs` + that ADR +
host pack/unpack. This worksheet never relocates one. Freeze proof:
`make design-win-standin`. See [YEAR_AHEAD.md](YEAR_AHEAD.md) and
[SIX_MONTH_FORWARD.md](SIX_MONTH_FORWARD.md) (closed M5–M6 record).

How to check a filled copy:

```bash
cargo test -p aether-design-win-check
cargo run -p aether-design-win-check
cargo run -p aether-design-win-check -- path/to/filled.toml
make design-win-check
```

`make design-win-check` is sequential cargo (no pipe, POSIX `/bin/sh`
safe). Same binary as `cargo run -p aether-design-win-check`.
The IREE HAL research stand-in (not a partner):

```bash
make design-win-standin
cargo run -p aether-design-win-check -- docs/design-win/iree-hal-standin.toml
```

---

## 1. Opcode names → `IreeHalCmd` / `AccelOp`

Fill **their** command-processor names. v1 pack emits
`command_categories = 0` (Nop doorbell) or
`IREE_HAL_COMMAND_CATEGORY_DISPATCH` (`1 << 1`) only.
`IREE_HAL_COMMAND_CATEGORY_TRANSFER` (`1 << 0`) **alone is not a
defined v1 packet** (`IreeHalCmd::check_v1` → `HalError::Fault`).

`command_categories` and `function` are **not** `AccelOp` bytes
(`Nop=0`, `MatMul=1`, `Wave=2`, `Add=3`, `Relu=4`). Soft-CP
`CpCmd.opcode` still is. Decode keys off the DISPATCH bit first;
`categories = 0` ignores `function` (pack writes 0). Unknown DISPATCH
`function` is Unsupported.

| Their opcode name | IREE `iree_hal_command_category_t` | IREE `iree_hal_executable_function_t` | Maps to `AccelOp` | IREE `iree_hal_element_type_t` / Aether `DType` | IREE `iree_hal_queue_affinity_t` (low 32: `chiplet<<16 \| tile`) |
| --- | --- | --- | --- | --- | --- |
| _fill_ | `0` (doorbell) or `DISPATCH` | `0` (`HAL_FN_MATMUL`) / `1` (`HAL_FN_FUSED`) / `2` (`HAL_FN_ADD`) / `3` (`HAL_FN_RELU`) | `Nop` / `MatMul` / `Wave` / `Add` / `Relu` | `INT_32` `0x10000020` / `FLOAT_16` `0x21000010` / `FLOAT_32` `0x21000020` | _fill_ |
| | | | | | |
| | | | | | |

`workgroup_count_x/y/z` are `AccelJobDesc` `m,n,k` **shape stand-ins**,
not compiler tile sizes and not IREE launch geometry. Do not fill them
as if they were.

Sample filled rows (research mapping, not a vendor ISA): see
[`examples/design-win-check/sample.toml`](../examples/design-win-check/sample.toml).
IREE HAL public-noun stand-in (not a partner):
[`docs/design-win/iree-hal-standin.md`](design-win/iree-hal-standin.md).

---

## 2. Executable (`iree_hal_executable_t`)

The kernel does not parse IREE VM bytecode. v1 admits one opaque
`abi::Executable.isa_blob_id`:

| Field | Frozen research value | Their fill |
| --- | --- | --- |
| `isa_blob_id` / `IreeHalCmd.executable` | `IREE_REF_EXECUTABLE` = `0x0001EE00` | must be this id |
| Any other handle | `HalError::Unsupported` | _do not ship an unknown blob id_ |

---

## 3. SID budget (Soft SMMU)

StreamID packing is Aether-shaped, **not** a PCIe BDF:
`[31:24] chiplet | [23:8] tile | [7:0] ssid`.
`ssid = 0` is `DEFAULT_STREAM` (SoftNPU / PASID-0 analogue) and is
**not** a legal IreeShapedCp submit SID. IreeShapedCp uses
`IREE_SSID = 2` (distinct from SoftNPU 0 and Soft-CP 1).

Per-tenant Bound CDs are capped at `SID_BUDGET_PER_TENANT = 4`. A fifth
bind is `MapError::SidBudget`. That is a software budget, not a silicon
SID allocator.

| Field | Research ABI | Their fill |
| --- | --- | --- |
| Pool size (Bound CDs / tenant) | `4` | |
| Substream (`ssid`) | `2` (`IREE_SSID`) | |
| Packed submit SID | nonzero; `ssid = 2` | |
| SID 0 / `DEFAULT_STREAM` | **refused** | must stay unused on this path |

If the part's real STE/CD budget differs, that is partner silicon — not
a silent bump of `SID_BUDGET_PER_TENANT` in this tree.

---

## 4. `MemorySpace` kinds they need

Buffers bind to exactly one typed place. `UNIFIED` is a cap bit, never
implied by `MEM_FULL`. IREE `iree_hal_memory_type_t` /
PJRT `PJRT_Memory` map onto these names; they are not a unified VAS.

| `MemorySpace` | Need? (yes / no) | Notes (banks, HBM stacks, scratch) |
| --- | --- | --- |
| `HOST` | | CPU-local DRAM; host load legal |
| `DEVICE_HBM` | | on-package HBM; DMA, not a coherent CPU load |
| `TILE_SRAM` | | first-class tile scratch, not a cache of HBM |
| `CXL_REGION` | | typed place; `TypedWindow` is a pin stub, not CXL.mem silicon |
| `SCRATCH` | | software-managed; not randomly mappable from the PJRT shim |
| `STREAMING` | | FIFO / fabric buffer; not randomly mappable from the PJRT shim |

---

## 5. Queue count

| Path | `AccelInfo.n_queues` | Their fill (what the CP actually has) |
| --- | --- | --- |
| `IreeShapedCp` (`backend = 4`) | **1** mailbox | v1 packet path |
| `SoftCommandProcessor` (`backend = 3`) | **2** software XQueues | Aether-native `CpCmd`, not this worksheet's packet |
| Their silicon CP | _fill_ | do not pretend v1 grew queues |

v1 `IreeHalCmd` is a **single mailbox**. Wanting two hardware queues
does not relocate packet fields and does not turn IreeShapedCp into
Soft-CP.

---

## 6. Event / fence scope

| Noun | Research ABI | Their fill |
| --- | --- | --- |
| Event | `abi::Event` = `FenceId` on a `PartitionId` timeline; host create/record/wait may use SoftChipletSync chiplet/package | |
| Semaphore payload | `IreeHalCmd.signal_payload` (`iree_hal_semaphore_t`); offset `0x58` | |
| Wait / complete | software CP-shaped seq; timeout is software | |
| Optional SoftChipletSync | `{wave, CU, chiplet, package}` visibility; **not** Vulkan, not UCIe | which scopes, if any |

Not a CUDA stream. Not a silicon fence unit. Not `GetPjRtApi`, not XLA.

---

## 7. Frozen `IreeHalCmd` offsets (do not change)

96-byte little-endian image. Magic `0xAE7E1EE1` (not `CpCmd`
`0xAE7E0C01`). Changing an offset or width is a dual
`ireecp.rs` + [ACCEL.md](ACCEL.md) ADR + host pack/unpack update.
**This table is not a fill-in.**

```text
offset  type   field                 IREE HAL noun
0x00    u32    magic                 0xAE7E1EE1
0x04    u16    command_categories    iree_hal_command_category_t
0x06    u16    binding_count         iree_hal_buffer_ref_list_t.count (0–4)
0x08    u32    executable            iree_hal_executable_t / isa_blob_id
0x0C    u32    function              iree_hal_executable_function_t
0x10    u32    workgroup_count_x     dispatch_config.workgroup_count[0]
0x14    u32    workgroup_count_y     [1]
0x18    u32    workgroup_count_z     [2]
0x1C    u32    element_type          iree_hal_element_type_t
0x20    u32    queue_affinity        iree_hal_queue_affinity_t (low 32)
0x24    u32    stream_id             Soft-SMMU StreamId (ssid=2); not an IREE field
0x28    u64    binding0_offset       iree_hal_buffer_ref_t.offset (IOVA A)
0x30    u64    binding1_offset       IOVA B
0x38    u64    binding2_offset       IOVA C
0x40    u64    binding3_offset       IOVA bias (0 if unused)
0x48    u32    binding0_length       iree_hal_buffer_ref_t.length (byte spans, not elems)
0x4C    u32    binding1_length
0x50    u32    binding2_length
0x54    u32    binding3_length
0x58    u64    signal_payload        iree_hal_semaphore payload / Event.fence
```

`examples/design-win-check` re-packs sentinel values through
`IreeHalCmd::to_le_bytes` and asserts these offsets. That is the
freeze, not a second packet.

---

## 8. Non-goals (do not fill these in as if they shipped)

- A signed vendor opcode ROM, a partnership, or a design win already
  in hand.
- An in-kernel IREE runtime, PJRT plugin (`GetPjRtApi`), or graph IR /
  fusion pass.
- `TRANSFER`-only v1 packets, unknown `isa_blob_id` values, or SID 0
  on the IreeShapedCp path.
- Relocating frozen `IreeHalCmd` fields to match a proprietary
  descriptor.
- Hardware SMMU / Host1x / a silicon fence unit (Soft SMMU + software
  timeline stay software).
- CUDA unified VA, CXL.mem silicon, Vulkan timelines, UCIe sync.
- That `PartnerNpuStub` is this path.
- Benchmarks vs Linux / CUDA / any NPU SDK.

Walkthrough: [HOST.md](HOST.md), [ABI.md](ABI.md), [ACCEL.md](ACCEL.md),
[DILIGENCE.md](DILIGENCE.md), [DEEP_DIVE_AGENDA.md](DEEP_DIVE_AGENDA.md).
