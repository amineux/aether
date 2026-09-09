# IREE HAL research stand-in (not a partner)

Filled [DESIGN_WIN.md](../DESIGN_WIN.md) using **public IREE HAL nouns**
already cited in [ACCEL.md](../ACCEL.md) and frozen on `IreeHalCmd`.

**This is a research stand-in, not a partner.** Not a signed vendor,
not NVIDIA, not an IREE runtime, not a PJRT plugin. `PartnerNpuStub`
stays a labeled no-op.

Machine copy the host checker admits:

```bash
make design-win-standin
# cargo run -p aether-design-win-check -- docs/design-win/iree-hal-standin.toml
```

**Do not change frozen `IreeHalCmd` offsets.** Magic `0xAE7E1EE1`,
96 bytes little-endian, `executable = 0x0001EE00`. Changing an offset
is a dual `ireecp.rs` + ACCEL.md ADR + host pack/unpack update.

Cited headers (Apache-2.0-with-LLVM-exception) on
[iree-org/iree](https://github.com/iree-org/iree)
`runtime/src/iree/hal/{device,command_buffer,buffer,buffer_view,executable,semaphore}.h`.
Aether does not claim that project.

---

## 1. Opcode names → `IreeHalCmd` / `AccelOp`

Public IREE `iree_hal_command_category_t` / `iree_hal_executable_function_t`
mapped the way `IreeShapedCp` already packs. v1 emits
`command_categories = 0` (Nop doorbell) or
`IREE_HAL_COMMAND_CATEGORY_DISPATCH` (`1 << 1`) only.
`IREE_HAL_COMMAND_CATEGORY_TRANSFER` (`1 << 0`) **alone is reserved** —
not a defined v1 packet (`IreeHalCmd::check_v1` → `HalError::Fault`).

`command_categories` and `function` are **not** `AccelOp` bytes
(`Nop=0`, `MatMul=1`, `Wave=2`). Decode keys off the DISPATCH bit
first; `categories = 0` ignores `function` (pack writes 0).

| Their opcode name (IREE HAL noun) | IREE `iree_hal_command_category_t` | IREE `iree_hal_executable_function_t` | Maps to `AccelOp` | IREE `iree_hal_element_type_t` / Aether `DType` | IREE `iree_hal_queue_affinity_t` (low 32: `chiplet<<16 \| tile`) |
| --- | --- | --- | --- | --- | --- |
| `iree_hal_command_buffer` (no DISPATCH bit; doorbell) | `0` | `0` (`HAL_FN_MATMUL`; ignored on decode) | `Nop` | `INT_32` `0x10000020` / `I32` | `0` |
| `iree_hal_command_buffer_dispatch` | `DISPATCH` (`1<<1` = `2`) | `0` (first export / `HAL_FN_MATMUL`) | `MatMul` | `FLOAT_32` `0x21000020` / `F32` | `0` |
| `iree_hal_device_queue_dispatch` (fused export) | `DISPATCH` (`1<<1` = `2`) | `1` (`HAL_FN_FUSED`) | `Wave` | `FLOAT_16` `0x21000010` / `F16` | `0` |
| `iree_hal_command_buffer` TRANSFER (`copy_buffer` shape) | `TRANSFER` (`1<<0` = `1`) | — | **reserved / refused** | — | — |

`workgroup_count_x/y/z` are `AccelJobDesc` `m,n,k` **shape stand-ins**,
not compiler tile sizes and not IREE launch geometry.

---

## 2. Executable (`iree_hal_executable_t`)

The kernel does not parse IREE VM bytecode. v1 admits one opaque
`abi::Executable.isa_blob_id`:

| Field | Frozen research value | This stand-in |
| --- | --- | --- |
| `isa_blob_id` / `IreeHalCmd.executable` | `IREE_REF_EXECUTABLE` = `0x0001EE00` | `0x0001EE00` |
| Any other handle | `HalError::Unsupported` | not shipped |

---

## 3. SID budget (Soft SMMU)

StreamID packing is Aether-shaped, **not** a PCIe BDF:
`[31:24] chiplet | [23:8] tile | [7:0] ssid`.
`ssid = 0` is `DEFAULT_STREAM` (SoftNPU / PASID-0 analogue) and is
**not** a legal IreeShapedCp submit SID. IreeShapedCp uses
`IREE_SSID = 2`.

Per-tenant Bound CDs are capped at `SID_BUDGET_PER_TENANT = 4`. A fifth
bind is `MapError::SidBudget`. **That is a software pool**, not a
silicon SID allocator.

| Field | Research ABI | This stand-in |
| --- | --- | --- |
| Pool size (Bound CDs / tenant) | `4` | `4` (software pool) |
| Substream (`ssid`) | `2` (`IREE_SSID`) | `2` |
| Packed submit SID | nonzero; `ssid = 2` | `0x00000002` (chiplet 0, tile 0, ssid 2) |
| SID 0 / `DEFAULT_STREAM` | **refused** | unused |

---

## 4. `MemorySpace` kinds they need

IREE `iree_hal_memory_type_t` / PJRT `PJRT_Memory` map onto these
names; they are not a unified VAS. `UNIFIED` is a cap bit, never
implied by `MEM_FULL`.

| `MemorySpace` | Need? | Notes |
| --- | --- | --- |
| `HOST` | yes | CPU-local DRAM; host load legal (`iree_hal_buffer_t` host-local) |
| `DEVICE_HBM` | yes | on-package HBM; DMA, not a coherent CPU load |
| `TILE_SRAM` | yes | first-class tile scratch, not a cache of HBM |
| `CXL_REGION` | no | typed place; `TypedWindow` is a pin stub, not CXL.mem silicon |
| `SCRATCH` | no | software-managed; not randomly mappable from the PJRT shim |
| `STREAMING` | no | FIFO / fabric buffer; not randomly mappable from the PJRT shim |

---

## 5. Queue count

| Path | `AccelInfo.n_queues` | This stand-in |
| --- | --- | --- |
| `IreeShapedCp` (`backend = 4`) | **1** mailbox | **1** (v1 packet path) |
| `SoftCommandProcessor` (`backend = 3`) | **2** software XQueues | not this packet |
| Their silicon CP | _fill on a real call_ | this stand-in does not invent queues |

v1 `IreeHalCmd` is a **single mailbox**. Wanting two hardware queues
does not relocate packet fields.

---

## 6. Event / fence scope

| Noun | Research ABI | This stand-in |
| --- | --- | --- |
| Event | `abi::Event` = `FenceId` on a `PartitionId` timeline; host create/record/wait may name SoftChipletSync chiplet/package | `partition-timeline` |
| Semaphore payload | `IreeHalCmd.signal_payload` (`iree_hal_semaphore_t`); offset `0x58` | same frozen field |
| Wait / complete | software CP-shaped seq; timeout is software | software |
| Optional SoftChipletSync | `{wave, CU, chiplet, package}` visibility; **not** Vulkan, not UCIe | all four named, software only |

Not a CUDA stream. Not a silicon fence unit. Not `GetPjRtApi`, not XLA.
Host `aether-pjrt` Event create / record / wait lower onto these
fences (chiplet or package where SoftChipletSync already exists).
`IreeHalCmd` offsets stay frozen; TRANSFER stays reserved.

---

## 7. Frozen `IreeHalCmd` offsets (do not change)

96-byte little-endian image. Magic `0xAE7E1EE1` (not `CpCmd`
`0xAE7E0C01`). **This table is not a fill-in.** Copied from
[ACCEL.md](../ACCEL.md) so the stand-in cites the same freeze.

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
`IreeHalCmd::to_le_bytes` and asserts these offsets. `make design-win-standin`
runs that freeze against this worksheet.

---

## 8. Non-goals (this stand-in does not fill these as if they shipped)

- A signed vendor opcode ROM, a partnership, or a design win already
  in hand. **Research stand-in, not a partner.**
- An in-kernel IREE runtime, PJRT plugin (`GetPjRtApi`), or graph IR /
  fusion pass.
- `TRANSFER`-only v1 packets, unknown `isa_blob_id` values, or SID 0
  on the IreeShapedCp path.
- Relocating frozen `IreeHalCmd` fields.
- Hardware SMMU / Host1x / a silicon fence unit (Soft SMMU + software
  timeline stay software). SID budget is a software pool.
- CUDA unified VA, CXL.mem silicon, Vulkan timelines, UCIe sync.
- That `PartnerNpuStub` is this path.
- Benchmarks vs Linux / CUDA / any NPU SDK. No NVIDIA. No FLOPs. No
  tape-out. Path B canonical.

Walkthrough: [HOST.md](../HOST.md), [ABI.md](../ABI.md), [ACCEL.md](../ACCEL.md),
[PARTNER.md](../PARTNER.md), [WEEK1_CALL.md](../WEEK1_CALL.md).
