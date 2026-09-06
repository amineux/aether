# Accelerator HAL and VirtIO-Accel

Accelerators are activities on a capability fabric, not devices behind ioctl.
Every compute unit — CPU tile, virt accel, SoftNPU — is an `Activity`
behind a uniform `EndpointId`. Drivers may still talk MMIO; the ABI does
not.

The kernel schedules partitions and fences; compilers schedule FLOPs.
`AccelJobDesc` is a dispatch record (op, shape, `(place, local)` buffers,
phase, fence). It is not a graph IR.

Every activity that is not a CPU tile implements `aether_hal::AccelDevice`:

```text
probe()  -> AccelInfo
submit(job: &AccelJobDesc) -> job_token   // avail ring + doorbell kick
poll()   -> Option<Completion>            // used ring; IRQ ack
map(req: MapRequest) -> iova              // pin / IOMMU; requires Memory cap
```

`submit` does **not** execute the job. Completions arrive on the used
ring after the device services a doorbell (SoftNPU `service()`, or a
future MSI-X).

The job descriptor is the architectural contract (see
`aether_core::accel::AccelJobDesc`): opcode, MxNxK, physical bases
(`PhysAddr` is the *local* field of a fabric address), typed
`MemorySpace`, `Place`, `Phase`, partition, fence, strides, dtype,
tenant, completion endpoint.

Remote access is an explicit DMA/NoC Exchange. `aether_hal::map_fabric`
refuses a silent coherent load across places. `UNIFIED_MEMORY` is a
capability bit (`CapRights::UNIFIED`), never implied by `MEM_FULL`.

v0.1 opcodes:

| Op | Meaning |
| --- | --- |
| `Nop` | doorbell / latency probe |
| `MatMul` | `C = A @ B` (I32 / software F16 / software F32) |
| `Wave` | matmul + optional bias (stand-in for a fused wave) |

`DType` values (additive; `I32 = 0` unchanged):

| Value | Type | SoftNPU |
| --- | --- | --- |
| 0 | `I32` | integer matmul (unchanged) |
| 1 | `F16` | software IEEE-754 `binary16` via F32 helpers |
| 2 | `F32` | software IEEE-754 `binary32` add/mul (FTZ) |

This is **not** a silicon tensor ISA and not a hard-float HAL. Unknown
dtype values are `UnsupportedDType`. `UserAccelJob` has no dtype field
(`/init` stays I32). `AccelJobDesc` / `AccelJobWire` / `CpCmd` carry
the byte.

## Virtqueue MMIO layout (in-kernel BAR)

There is **no** upstream `virtio-accel` device. SpecForge Y1H1 **path B**
is the decision: this in-kernel BAR is the **canonical demo**. Path A
(a custom QEMU `-device` / virtio-mmio) stays optional later. The
kernel emulates a virtqueue-shaped MMIO window
(`aether_drivers::mmio::AccelMmio`, 1 KiB). The offsets below are
**frozen**.

```text
MMIO cfg (what a future -device virtio-accel would expose)
  0x00  magic       0xAE7EACC1
  0x04  version     1
  0x08  status      ACK | DRIVER | DRIVER_OK | FAILED
  0x0C  qsize       8
  0x10  doorbell    write 1 = kick
  0x14  used_idx    device-updated
  0x18  irq_status  bit0 = used-ring IRQ
  0x1C  irq_ack     driver write 1 to ack
  0x20  avail_idx   driver-updated

Queue (same BAR)
  0x80  avail[8]    AccelJobWire (88 bytes; opcode/shape/IOVAs)
  0x340 used[8]     { job_seq, status, cycles }
```

Path:

```text
AccelDevice::submit  → write avail[i], avail_idx++, doorbell=1
SoftNpuDevice::service (IRQ / kthread poll)
                     → take avail, execute SoftNPU, write used, irq_status|=1
AccelDevice::poll    → read used[i], ack IRQ
```

`SoftNpuDevice` is the backend executor. The kernel driver never calls
`SoftNpu::execute` in-process on the submit path. `make qemu` still
needs only stock QEMU.

A QEMU device team would implement the same offsets, DMA the job wire,
and raise a real IRQ. Swap `SoftNpuDevice` for `VirtioAccelMmio` without
touching fabric or caps.

The older `VirtioAccelQueue` helper remains as a host-tested ring model.

## ADR: SpecForge Y1H1 virtio path (A vs B)

**Status:** Accepted 2026-09-06.

**Context.** SpecForge Y1H1 asked for a real virtio-accel path: either
**(A)** a QEMU `-device` / virtio-mmio that DMA-reads the BAR above with
SoftNPU behind it, **or (B)** document that the in-kernel BAR is the
canonical demo and lock it with a golden MMIO trace. Falsifier deferred
a custom QEMU device until after AccelDevice; Soft-CP (`backend = 3`)
already covers a second AccelDevice path on the host.

**Decision: path B.** SoftNPU behind `AccelMmio` is what `make qemu`
runs. Stock QEMU only. No new QEMU device C code.

- The BAR layout in the previous section is **frozen**: `magic` (0x00),
  `version` (0x04), `status` (0x08), `qsize` (0x0C), `doorbell` (0x10),
  `used_idx` (0x14), plus irq/avail and the avail/used rings. Changing
  an offset is a dual SoftNPU + this doc + golden-trace update.
- Host tests record the cfg / doorbell / used-ring access sequence for
  one SoftNPU submit/complete (`golden_mmio_softnpu_submit_complete` in
  `drivers/src/mmio.rs`, and the SoftNPU twin in
  `drivers/src/softnpu.rs`).
- Path A remains optional later. A QEMU device would implement **these**
  offsets, DMA the job wire, and raise a real IRQ. Do not invent a
  second BAR.

**Not claimed.** This is not an upstream virtio device, not a silicon
BAR, and not a vendor integration.

## Map API (Soft SMMU; not hardware)

`aether_core::iommu::IommuMap` is a **software** stream-ID page table.
It is not a hardware SMMU and not a full SMMUv3 emulator.

```text
StreamId = chiplet | tile | ssid     (not a PCIe BDF)
STE  →  CD (ssid)  →  block descriptors

capture(sid)                         // first sighting; DMA still aborts
bind_stream(Memory+MAP, sid)         // install STE + CD
map(...)                             // capture+bind on first authorized use
translate / resolve                  // StreamAbort until Bound
unbind_stream(sid)                   // FLR analogue
```

Rules:

1. Refuse unless `cap.kind == Memory` and `cap.rights` contains `MAP`.
   Bind and map share that gate. Capture alone does not authorize DMA.
2. StreamIDs are accelerator / chiplet identities. Two SIDs (including
   two chiplets with the same tile number, or two SSIDs on one STE) may
   pin the same guest PA to different IOVAs. Same-SID guest-PA overlap
   is `Overlap` (or `CrossTenant` if another tenant holds the window).
3. IOVAs come from a per-(STE, CD) bump allocator above 4 GiB
   (`SOFT_SMMU_IOVA_BASE`). This is not identity.
4. Translate on an Unbound / Captured SID, or an STE whose SSID has no
   CD, is `StreamAbort`. Wrong SID is `WrongStream`. Wrong tenant is
   `CrossTenant`. Unmapped on a bound SID is `NotMapped`.
5. `AccelDevice::map` without a prior cap walk returns `NoMemoryCap`.
   Use `SoftNpuDevice::map_with_cap` / `IommuMap::map`. SoftNPU DMA
   uses stream 0: first pin binds that SID; submit writes IOVAs into
   the virtqueue; `service` resolves IOVA → guest PA.

Bank QoS / bandwidth coloring is not part of Soft SMMU.

`SYS_MAP` walks the Memory cap, pins the arena through Soft SMMU
(stream 0), and sets USER on the 2 MiB page. `/init` tensors may live
in the user image (also Soft-SMMU-pinned at boot) or in a mapped arena.

Do **not** map “all of HBM” into the NPU. The arena + cap is the point.

## Bank coloring

Tensor arenas carry a `BankColor { tenant, bank }`:

- `ArenaRequest::for_tenant` paints the allocation at birth.
- `transfer_owner` is the explicit ownership transfer; it recolors the
  arena to the new tenant and keeps the physical bank.
- `Phase::Exchange` is the other legal way to touch a foreign bank
  (DMA / NoC xfer). Compute on a foreign color is refused.

`TileScheduler::place_ok` and `admit_wave` share that gate. Host tests
cover refuse (foreign bank / foreign tenant) and transfer-then-admit.

## SoftNPU

`aether_core::SoftNpu` is a deterministic reference engine:

- shapes up to 64×64 (prototype bound)
- I32 overflow → `AccelError::Overflow`
- F16 / F32 use integer-only software IEEE (`core/src/softfloat.rs`);
  subnormals flush to zero. Not libm, not a vendor FLOP claim.
- `Wave` adds an optional bias vector (same dtype as the job)
- A DMA view without `load_u16` refuses F16 (`UnsupportedDType`)

It is a **model of a matmul/wave engine**, not a product NPU. The point is
that job submit, ownership, and completion look like silicon.

## How to plug a command processor

The worked example is `aether_drivers::SoftCommandProcessor`
(`backend = 3`, name `soft-cp`). It is a **software model** of a
silicon CP mailbox: it packs a 64-byte packet, translates through the
Soft SMMU (`StreamId` + bind/abort), and completes on an IRQ/poll path
into a fence. It is not SoftNPU (virtqueue BAR, `backend = 1`), not the
in-process SoftNPU engine (`backend = 0`), and not the no-op
`PartnerNpuStub` (`backend = 2`).

`AccelInfo.backend` ids:

| Id | Impl | Honest reading |
| --- | --- | --- |
| 0 | SoftNPU in-process / Dummy | Reference execute; no packet |
| 1 | `SoftNpuDevice` | Virtqueue MMIO + SoftNPU (QEMU demo) |
| 2 | `PartnerNpuStub` | No-op sketch; leave it alone |
| 3 | `SoftCommandProcessor` | Packed CP packet + Soft SMMU + IRQ/fence |

### `CpCmd` packet (64 bytes, little-endian)

```text
offset  type   field
0x00    u32    magic        0xAE7E0C01
0x04    u8     opcode       AccelOp (Nop=0, MatMul=1, Wave=2)
0x05    u8     dtype        DType (I32=0, F16=1, F32=2)
0x06    u8     space        MemorySpace
0x07    u8     phase        Phase (Compute=0, Exchange=1, Barrier=2)
0x08    u16    m
0x0A    u16    n
0x0C    u16    k
0x0E    u16    flags        bit0 = HAS_BIAS
0x10    u32    stream_id    StreamId: [31:24] chiplet | [23:8] tile | [7:0] ssid
0x14    u16    chiplet      job.place.chiplet
0x16    u16    tile         job.place.tile (0 if none)
0x18    u64    iova_a
0x20    u64    iova_b
0x28    u64    iova_c
0x30    u64    iova_bias    0 if no bias
0x38    u64    fence_id
```

`CpCmd::pack` fills this from `AccelJobDesc` after
`IommuMap::translate_result` on `StreamId::accel(chiplet, tile, CP_SSID)`
(`ssid = 1`, distinct from SoftNPU's `DEFAULT_STREAM`). A silicon CP
would DMA the same 64 bytes from a mailbox. `to_le_bytes()` is the wire
image.

### Driver steps (what SoftCommandProcessor already does)

```text
1. probe() → AccelInfo { backend: 3, vendor: 0xAE7E, device: 0x0003 }.
2. bind_stream(Memory+MAP, sid) and/or map_with_cap(pin_accel(...)):
   first authorized map captures+binds. Capture alone leaves the SID
   aborting. map() without a cap walk returns NoMemoryCap.
3. submit(): CpCmd::pack(job, &iommu) → mailbox, doorbell=1.
   Unbound / captured / missing / partial / wrong-stream → Fault.
   Does not execute. poll() is empty until service().
4. service() (IRQ / kthread poll): resolve_stream each IOVA, run the
   integer engine, write a Completion, raise IRQ.
5. poll(): pop the completion and ack the IRQ. The job's fence_id
   (a timeline seq) is retired through `Timeline::complete` /
   `retire_into`. `wait` polls the retired watermark. Do not treat
   this as a silicon fence unit.
6. Never accept a PA that did not come from a cap walk + IommuMap pin.
7. Honor BankColor at the scheduler / SYS_ACCEL_SUBMIT layer (unchanged).
```

Swap `SoftCommandProcessor` for a real BAR + MSI-X by keeping this
packet and replacing `service()` with a device IRQ. Do not invent a
second IR. Do not start from `PartnerNpuStub` — that sketch is still
in-tree as a labeled no-op, not progress.

QEMU still demos SoftNPU (which already writes Soft-SMMU IOVAs into
the avail ring). Soft-CP is host-contract tested; the kernel self-check
only probes it so the backend id is visible on the serial log. No
custom QEMU device is added here.

## Co-scheduling

`TileScheduler` has an `Npu` tile. Init enqueues an `AccelWave` with a
deadline, bank affinity, and arena color; `pick(npu0)` returns it before
a CPU thread, and refuses a foreign-colored Compute wave.
Work-stealing will not move a wave onto a CPU tile (`Job::compatible`).

Jobs are fence-ordered and credit-limited per `PartitionProfile`.
`Timeline` is a **software model** of what a CP would retire
(`TimelineId` + monotonic seq in `fence_id`, `wait` on the retired
watermark, in-order `complete`). That is not a CUDA stream: there
is no implicit catch-up, and a partition that is out of credits
refuses submit. `timeout` is a software overlay — it does not
claim a device IRQ. SoftCommandProcessor and SoftNPU both retire
through this API. QEMU's used-ring IRQ is still software.

A later cut should:

- let a real device IRQ (not only kthread poll) write the seq
- meter HBM bandwidth as the partition QoS budget already names
- replace Soft SMMU with a hardware SMMU page table (program a real SID)

Do not assume cache coherence across chiplets. SRAM on the tile is the
honest first place; HBM and CXL are other typed spaces, not a wafer-scale
flat address space.
