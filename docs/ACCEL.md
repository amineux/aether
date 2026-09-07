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

SoftNPU `AccelOp` (path B):

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
is still the **canonical demo**: the in-kernel BAR is what `make qemu`
runs (stock QEMU). Path A landed as an **optional** QEMU `-device`
(`qemu/aether_accel.c`, `make qemu-accel`) that implements these same
offsets. The kernel emulates a virtqueue-shaped MMIO window
(`aether_drivers::mmio::AccelMmio`, 1 KiB). The offsets below are
**frozen**.

```text
MMIO cfg (path A -device aether-accel and path B in-kernel BAR)
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

`SoftNpuDevice` is the backend executor on path B. The kernel driver
never calls `SoftNpu::execute` in-process on the submit path. `make qemu`
still needs only stock QEMU.

Path A (`qemu/aether_accel.c`) implements the same offsets, DMA-reads
tensor GPAs from the job wire, runs SoftNPU I32, and raises a used-ring
IRQ. A guest driver can swap `SoftNpuDevice` for `VirtioAccelMmio`
without touching fabric or caps. The stock kernel still uses path B so
Soft SMMU / identity islands / mmap / KPTI stay on the code `make qemu`
already boots.

The older `VirtioAccelQueue` helper remains as a host-tested ring model.

## ADR: SpecForge Y1H1 virtio path (A vs B)

**Status:** Accepted 2026-09-06; path A addendum 2026-09-06.

**Context.** SpecForge Y1H1 asked for a real virtio-accel path: either
**(A)** a QEMU `-device` / virtio-mmio that DMA-reads the BAR above with
SoftNPU behind it, **or (B)** document that the in-kernel BAR is the
canonical demo and lock it with a golden MMIO trace. Falsifier deferred
a custom QEMU device until after AccelDevice; Soft-CP (`backend = 3`)
already covers a second AccelDevice path on the host. Path B landed
first (golden MMIO trace, stock QEMU).

**Decision: path B remains canonical for stock QEMU.** SoftNPU behind
`AccelMmio` is what `make qemu` runs. Stock QEMU only on that target.

**Path A landed as optional.** `qemu/aether_accel.c` is a self-contained
softmmu device model (PCI wrapper in `qemu/aether_accel_pci.c`) that
implements **these** offsets, DMA-reads the job wire, executes SoftNPU
I32, and raises a used-ring IRQ. `make accel-test` is the host/unit
test CI runs. `make qemu-accel` uses `-device aether-accel` when
`QEMU_ACCEL` points at a QEMU built with the device (see
`qemu/README.md`). CI does **not** rebuild QEMU.

- The BAR layout in the previous section is **frozen**: `magic` (0x00),
  `version` (0x04), `status` (0x08), `qsize` (0x0C), `doorbell` (0x10),
  `used_idx` (0x14), plus irq/avail and the avail/used rings. Changing
  an offset is a dual SoftNPU + path-A device + this doc + golden-trace
  update.
- Host tests record the cfg / doorbell / used-ring access sequence for
  one SoftNPU submit/complete (`golden_mmio_softnpu_submit_complete` in
  `drivers/src/mmio.rs`, and the SoftNPU twin in
  `drivers/src/softnpu.rs`). Path A’s C test checks the same published
  cfg values after one I32 submit/complete.
- Do not invent a second BAR. Path A DMA uses guest physical addresses
  from the job wire. Soft SMMU stays a kernel table on path B; this
  device is not a QEMU IOMMU.
- F16 / F32 software IEEE stay path-B SoftNPU. Path A completes those
  dtypes with status `-1`.

**Not claimed.** This is not an upstream virtio device, not a silicon
BAR, not a vendor integration, and not a kernel driver that has swapped
off `SoftNpuDevice`. The stock guest still retires jobs on the
in-kernel BAR.

## Map API (Soft SMMU; not hardware)

`aether_core::iommu::IommuMap` is a **software** stream-ID page table.
It is not a hardware SMMU and not a full SMMUv3 emulator.

```text
StreamId = chiplet | tile | ssid     (not a PCIe BDF)
STE  →  CD (ssid ≤ S1CDMax)  →  Stage-1 (IOVA→IPA)  →  Stage-2 (IPA→PA)

capture(sid)                         // first sighting; DMA still aborts
bind_stream(Memory+MAP, sid)         // install STE + CD (Nested, identity S2)
bind_nested(Memory+MAP, sid)         // Nested with distinct IPA (host tests)
map(...)                             // capture+bind on first authorized use
walk / resolve                       // STE→CD→S1→S2; StreamAbort until Bound
invalidate(Ats|Tlbi|CfgSte|CfgCd)    // software ATC; tables stay
unbind_cd(sid)                       // drop one SSID
unbind_stage2(sid)                   // drop S2 only → Stage2Fault
unbind_stream / flr(sid)             // STE-wide FLR analogue
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
4. Translate on an Unbound / Captured SID, an illegal SSID (`> S1CDMax`),
   or an STE whose SSID has no valid CD, is `StreamAbort`. Wrong SID is
   `WrongStream`. Wrong tenant is `CrossTenant`. Unmapped Stage-1 on a
   bound SID is `NotMapped`. Nested Stage-2 miss is `Stage2Fault`.
5. `AccelDevice::map` without a prior cap walk returns `NoMemoryCap`.
   Use `SoftNpuDevice::map_with_cap` / `IommuMap::map`. SoftNPU DMA
   uses stream 0: first pin binds that SID (Nested, identity Stage-2);
   submit writes IOVAs into the virtqueue; `service` walks
   STE→CD→S1→S2 to resolve IOVA → guest PA. This is still a software
   table. A hardware SMMU requires partner silicon.

Bank QoS / bandwidth coloring is not part of Soft SMMU.

`SYS_MAP` walks the Memory cap, pins the arena through Soft SMMU
(stream 0), and sets USER on the 2 MiB page. `/init` tensors may live
in the user image (also Soft-SMMU-pinned at boot) or in a mapped arena.

Do **not** map “all of HBM” into the NPU. The arena + cap is the point.

## Typed windows (exploration stub; not CXL.mem)

`TypedWindow { base, len, kind: Hbm | CxlMemStub | Dram, sid }` is a
host/kernel range Soft SMMU can pin with Memory+MAP. CXL.mem is
inspiration for the `CxlMemStub` noun only. See [WINDOW.md](WINDOW.md).

```text
IommuMap::map_window(Memory+MAP, win)     // pin on win.sid; not identity
IommuMap::map_window_sid(..., sid)        // sid must equal win.sid
IommuMap::unmap_window(..., sid, iova)    // WrongStream / CrossTenant
SpectralCut::allow_window(win, caller)    // foreign tenant → CrossCut
AccelDevice::map_window / map_window_with_cap
```

This is **not** a CXL.mem HDM decoder and not QEMU CXL. Host tests cover
the stub. Hardware CXL.mem still requires partner silicon.

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

The Aether-native worked example is `aether_drivers::SoftCommandProcessor`
(`backend = 3`, name `soft-cp`). It is a **software model** of a
silicon CP mailbox: it packs a 64-byte packet, translates through the
Soft SMMU (`StreamId` + bind/abort), and completes on an IRQ/poll path
into a fence. It is not SoftNPU (virtqueue BAR, `backend = 1`), not the
in-process SoftNPU engine (`backend = 0`), and not the no-op
`PartnerNpuStub` (`backend = 2`). SoftNPU path B and Soft-CP stay.

The **partner-shaped HAL spine** is `aether_drivers::IreeShapedCp`
(`backend = 4`, name `iree-shaped-cp`). It packs a frozen IREE HAL
dispatch packet (public Device / Buffer / Executable / Event nouns),
not Aether-native `AccelOp` bytes and not a signed vendor. See the
opcode/packet ADR below.

`AccelInfo.backend` ids:

| Id | Impl | Honest reading |
| --- | --- | --- |
| 0 | SoftNPU in-process / Dummy | Reference execute; no packet |
| 1 | `SoftNpuDevice` | Virtqueue MMIO + SoftNPU (QEMU demo) |
| 2 | `PartnerNpuStub` | No-op sketch; leave it alone |
| 3 | `SoftCommandProcessor` | Packed Aether-native `CpCmd` + Soft SMMU + IRQ/fence |
| 4 | `IreeShapedCp` | IREE HAL dispatch packet + Soft SMMU + IRQ/fence; not a vendor |

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
only probes it so the backend id is visible on the serial log. Soft-CP
does not add a QEMU device. Path A (`qemu/aether_accel.c`) is a
separate optional BAR device; stock `make qemu` does not attach it.

## ADR: partner-shaped opcode packet (`IreeShapedCp`)

**Status:** Accepted 2026-09-07.

**Context.** Falsifier-revised spine item: land a partner-shaped
`AccelDevice` whose frozen command packet uses a **concrete public ISA
/ HAL noun set**, not `PartnerNpuStub` enrichment theater and not
Soft-CP 2.0 with the same Aether-native opcodes (`Nop` / `MatMul` /
`Wave`) only. SoftNPU path B (`backend = 1`) and Soft-CP
(`backend = 3`) stay.

**Choice: IREE HAL / PJRT-shaped Device / Buffer / Executable / Event.**

Rejected alternatives:

- **TT-Metal / other NPU command descriptors.** Viable only with cited
  public field names. IREE HAL already matches the host nouns in
  `aether_core::abi` and [ABI.md](ABI.md); adding a second public
  vocabulary would be a second IR.
- **Invented NVIDIA opcode list.** Forbidden. No partnership claim.

Cited public IREE HAL fields (Apache-2.0-with-LLVM-exception headers
on [iree-org/iree](https://github.com/iree-org/iree) `main`):

| Packet field | IREE HAL noun | Header |
| --- | --- | --- |
| `command_categories` | `iree_hal_command_category_t` (`TRANSFER = 1<<0`, `DISPATCH = 1<<1`) | `runtime/src/iree/hal/command_buffer.h` |
| `executable` / `function` | `iree_hal_executable_t` / `iree_hal_executable_function_t` | `runtime/src/iree/hal/{executable,command_buffer}.h` |
| `workgroup_count[3]` | `iree_hal_dispatch_config_t.workgroup_count` | `command_buffer.h` |
| `binding[i].offset` / `.length` | `iree_hal_buffer_ref_t.{offset,length}` | `command_buffer.h` |
| `element_type` | `iree_hal_element_type_t` (`IREE_HAL_ELEMENT_TYPE_VALUE`) | `runtime/src/iree/hal/buffer_view.h` |
| `queue_affinity` | `iree_hal_queue_affinity_t` (low 32; Place stand-in) | `runtime/src/iree/hal/device.h` |
| `signal_payload` | `iree_hal_semaphore_t` payload | `runtime/src/iree/hal/semaphore.h` |

Aether mapping (compiler-owned `AccelJobDesc` → HAL packet; kernel
does **not** parse IREE VM bytecode):

| AccelJobDesc | HAL packet |
| --- | --- |
| `op = Nop` | `command_categories = 0`, `function = 0` (doorbell; no pins) |
| `op = MatMul` | `DISPATCH`, `function = 0` (first export) |
| `op = Wave` | `DISPATCH`, `function = 1` (fused export) |
| `dtype` I32 / F16 / F32 | `IREE_HAL_ELEMENT_TYPE_{INT_32,FLOAT_16,FLOAT_32}` = `0x10000020` / `0x21000010` / `0x21000020` |
| `m,n,k` | `workgroup_count_x/y/z` (shape stand-in) |
| `a,b,c,bias` after Soft SMMU | `binding[0..3].offset` = IOVA; `.length` = `job.bytes_*()` byte spans (dtype-aware), not element counts |
| `place` | `queue_affinity` = `chiplet<<16 \| tile` |
| `fence_id` | `signal_payload` (`abi::Event.fence`) |
| — | `executable = 0x0001EE00` frozen `isa_blob_id` (`IREE_REF_EXECUTABLE`) |

Nop → `categories = 0`, `function = 0` (`function` is ignored on decode;
decode keys off categories bit `DISPATCH`). Prevents a shim that branches
on `function` first from mis-labeling Nop as MatMul.

v1 `pack` emits `command_categories = 0` or `DISPATCH` only; `TRANSFER`
(`1<<0`) alone is Fault / not defined for this software CP.

`workgroup_count ← m,n,k` is a shape stand-in. PJRT/IREE host must not
treat these as compiler tile sizes or launch geometry; they are
`AccelJobDesc` shape fields copied for the research CP only.

The PJRT shim must pack `executable == 0x0001EE00` (`IREE_REF_EXECUTABLE`);
other `isa_blob_id` values must be refused (the kernel does not parse IREE VM
bytecode).

`command_categories` and `function` are **not** `AccelOp` (`MatMul = 1`,
`Wave = 2`). Soft-CP's `CpCmd.opcode` still is. That is the point of
this backend.

**Not claimed.** Not an IREE runtime in the kernel, not a PJRT plugin,
not a signed IREE or silicon partnership, not FLOPs, not a vendor
opcode ROM.

### `IreeHalCmd` packet (96 bytes, little-endian)

`IreeHalCmd` offsets + field widths are **frozen**. Changing an
offset/width is a dual `ireecp.rs` + this ADR + host pack/unpack test
update. PJRT shim (#41) consumes this image only.

```text
offset  type   field                 IREE HAL noun
0x00    u32    magic                 0xAE7E1EE1 (not CpCmd 0xAE7E0C01)
0x04    u16    command_categories    iree_hal_command_category_t
0x06    u16    binding_count         iree_hal_buffer_ref_list_t.count (0–4)
0x08    u32    executable            iree_hal_executable_t / isa_blob_id
0x0C    u32    function              iree_hal_executable_function_t
0x10    u32    workgroup_count_x     dispatch_config.workgroup_count[0]
0x14    u32    workgroup_count_y     [1]
0x18    u32    workgroup_count_z     [2]
0x1C    u32    element_type          iree_hal_element_type_t
0x20    u32    queue_affinity        iree_hal_queue_affinity_t (low 32)
0x24    u32    stream_id             Soft-SMMU StreamId (Aether pin; ssid=2)
0x28    u64    binding0_offset       iree_hal_buffer_ref_t.offset (IOVA A)
0x30    u64    binding1_offset       IOVA B
0x38    u64    binding2_offset       IOVA C
0x40    u64    binding3_offset       IOVA bias (0 if unused)
0x48    u32    binding0_length       iree_hal_buffer_ref_t.length (job.bytes_*; dtype-aware bytes, not elems)
0x4C    u32    binding1_length
0x50    u32    binding2_length
0x54    u32    binding3_length
0x58    u64    signal_payload        iree_hal_semaphore payload / Event.fence
```

`IreeHalCmd::pack` fills this from `AccelJobDesc` after
`IommuMap::translate_result` on `StreamId::accel(chiplet, tile, IREE_SSID)`
(`ssid = 2`). Identity DMA is not a tensor path. `to_le_bytes()` is the
wire image. `AccelDevice` / `AccelJobDesc` ABI is unchanged (this packet
is additive).

### Driver steps (what `IreeShapedCp` already does)

```text
1. probe() → AccelInfo { backend: 4, vendor: 0xAE7E, device: 0x0004 }.
2. bind_stream(Memory+MAP, sid) and/or map_with_cap(pin_accel(...)):
   first authorized map captures+binds. map() without a cap walk
   returns NoMemoryCap. SoftNPU ssid 0 and Soft-CP ssid 1 are WrongStream.
3. submit(): IreeHalCmd::pack(job, &iommu) → mailbox, doorbell=1.
   Unbound / captured / missing / partial / wrong-stream → Fault.
   Does not execute. poll() is empty until service().
4. service() (IRQ / kthread poll): resolve_stream each binding IOVA,
   run the integer engine, write a Completion, raise IRQ.
5. poll(): pop the completion and ack the IRQ. signal_payload (fence
   seq) retires through Timeline::complete / retire_into.
6. Never accept a PA that did not come from a cap walk + IommuMap pin.
```

The kernel self-check only probes `IreeShapedCp` so backend 4 is
visible on the serial log (`[accel] IreeShapedCp probe backend=4
iree-shaped-cp (IREE HAL packet; not a vendor)`). QEMU still demos
SoftNPU. This backend does not add a QEMU device.

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
claim a device IRQ. SoftCommandProcessor, IreeShapedCp, and SoftNPU
all retire through this API. QEMU's used-ring IRQ is still software on x86
(kthread poll after the PIC timer). On RISC-V the same AccelMmio
BAR is serviced from a **PLIC claim** (UART THRE software doorbell,
source 10) — a real interrupt path, still path B, still not a
virtio-mmio `-device`.

A later cut should:

- let a virtio-mmio / MSI-X device IRQ write the seq (RISC-V already
  retires from a PLIC claim on the path-B BAR; x86 is still kthread poll)
- meter HBM bandwidth as the partition QoS budget already names
- replace Soft SMMU with a hardware SMMU page table (program a real SID).
  Soft SMMU now walks STE→CD→Stage-1/2 in software; that is not silicon.

Do not assume cache coherence across chiplets. SRAM on the tile is the
honest first place; HBM and CXL are other typed spaces, not a wafer-scale
flat address space.
