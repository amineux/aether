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
bind_mm(Memory+MAP, sid, MmId)       // PASID/SVA: mm ↔ SSID (this device)
map_va(sid, va, guest_pa, len)       // S1 VA = process VA; DMA uses VA
unmap_va(sid, va)                    // drop S1 + SSID ATC (TLB)
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
5. **SID-at-submit (Host1x-shaped).** Soft-CP / IreeShapedCp program
   SET_SID at the job head (`IommuMap::set_sid` / `set_sid_bound`)
   before DMA. `resolve_submit` / `walk_submit` abort (`SubmitSid`)
   until that SID is armed for **this** submit; packet SID ≠ armed SID
   is `WrongStream`. Bind-at-map is not enough on the CP submit path.
   SoftNPU path B still walks Bound SIDs without a submit latch.
6. Per-tenant SID budget (`SID_BUDGET_PER_TENANT = 4` Bound CDs).
   Exhausting it is `SidBudget`. Process/tenant-level contexts, not a
   silicon SID allocator.
7. `AccelDevice::map` without a prior cap walk returns `NoMemoryCap`.
   Use `SoftNpuDevice::map_with_cap` / `IommuMap::map`. SoftNPU DMA
   uses stream 0: first pin binds that SID (Nested, identity Stage-2);
   submit writes IOVAs into the virtqueue; `service` walks
   STE→CD→S1→S2 to resolve IOVA → guest PA. This is still a software
   table. A hardware SMMU requires partner silicon.

The optional bring-up kit dumps these software tables and replays one
map / translate / abort-until-bound / wrong-stream / ATS sequence:
[bringup/BRINGUP.md](bringup/BRINGUP.md). `make smmu-bringup`. Not a
Soft-SMMU redo.

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
silicon CP mailbox: it packs a 64-byte packet, programs SET_SID at
submit (Host1x-shaped job head; SID sticks on the XQueue), translates
through the Soft SMMU (`StreamId` + bind/abort + submit latch), and
completes on an IRQ/poll path into a fence. It is not SoftNPU
(virtqueue BAR, `backend = 1`), not the in-process SoftNPU engine
(`backend = 0`), and not the no-op `PartnerNpuStub` (`backend = 2`).
SoftNPU path B and Soft-CP stay.

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
| 3 | `SoftCommandProcessor` | Packed Aether-native `CpCmd` + SET_SID-at-submit + two XQueues + SoftGreenCtx SM/WQ + SoftChipletSync/SoftCCT + SoftNoI-IS admit + SoftCmdFirewall + PASID/SVA + OperatorInject + Soft SMMU + IRQ/fence |
| 4 | `IreeShapedCp` | IREE HAL dispatch packet + SET_SID-at-submit + Soft SMMU + IRQ/fence; not a vendor |

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
0x0E    u16    flags        bit0 = HAS_BIAS; bit1 = SET_SID (job head stamped)
0x10    u32    stream_id    SET_SID operand: [31:24] chiplet | [23:8] tile | [7:0] ssid
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
3. submit() / submit_xqueue(): SET_SID at the job head, then
   CpCmd::pack_on_stream on the queue SID, then SoftCmdFirewall
   copy-then-validate (opcode / reloc / SID / addr cap on the kernel
   copy). Privileged `set_sid(Memory+MAP, sid)` programs the latch;
   first submit inherits `stream_for_job` (or the latch) and sticks it
   on the XQueue. Unbound / captured / missing / partial / wrong-stream
   / no SET_SID / identity IOVA sneak → Fault. Does not execute.
   poll() is empty until service(). `submit_cmdbuf` is the userspace
   image path (same arena).
4. service() (IRQ / kthread poll): dequeue a Running XQueue, re-arm
   SET_SID from the packet, resolve_submit each IOVA, run the integer
   engine, write a Completion, raise IRQ, clear the submit latch.
   Tampered packet SID is status -2.
5. poll(): pop the completion and ack the IRQ. The job's fence_id
   (a timeline seq) is retired through `Timeline::complete` /
   `retire_into`. `wait` polls the retired watermark. Do not treat
   this as a silicon fence unit.
6. Never accept a PA that did not come from a cap walk + IommuMap pin.
7. Honor BankColor at the scheduler / SYS_ACCEL_SUBMIT layer (unchanged).
8. Two software XQueues (`n_queues = 2`): create / submit / suspend /
   resume. `AccelDevice::submit` is queue 0. See the XQueue section.
9. SoftGreenCtx (`sm_count` / `wq_count` on `AccelInfo`): fake SM/WQ
   pool, 70/30 split, bind XQueue, migrate-to-yield. SID unchanged.
   See the SoftGreenCtx section.
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

## Soft-CP XQueue (software; XSched-shaped)

**Status:** **M4 done (PR #47).** Two software queues; queue-boundary
suspend/resume. Not a silicon queuing unit. Not an XSched LD_PRELOAD
shim. SID-at-submit (M3) is landed (SET_SID sticks on the queue).

**Inspiration.** [XSched](https://github.com/XpuOS/xsched) (OSDI’25)
exposes an **XQueue** as the schedulable object on an open, multi-level
hardware execution model (device / context / queue), with suspend and
resume as first-class controls. Soft-CP borrows that *shape*: the
queue, not the device mailbox, is what the scheduler parks.

**This is not:**

- An XSched port, and not XSched’s **LD_PRELOAD** CUDA / HIP / OpenCL
  interceptor shims.
- A silicon queuing unit, hardware doorbell fabric, or GPU preemption
  engine.
- Mid-command / mid-op stop. Soft-CP can refuse the **next** packed
  `CpCmd` on a queue (`PreemptionLevel::QueueBoundary`). A command
  already inside `service()` / `SoftNpu::execute` runs to completion.
  `PreemptionLevel::MidOp` is named so callers do not overclaim; no
  in-tree backend returns it.
- A change to path-B SoftNPU, the virtqueue BAR, or `make qemu`.
  `IreeShapedCp` stays a single device-shaped mailbox this cut.

**Contract** (`aether_hal::AccelDevice` defaults + Soft-CP override):

```text
create_queue(id, stream_id, priority)  // stamp sticky SID; Running
submit_queue(id, job)                  // pack CpCmd on the queue SID
suspend_queue(id) → QueueBoundary      // park; pending stays
resume_queue(id)                       // unpark
service()                              // highest-priority Running queue
AccelDevice::submit                    // queue 0 (device-shaped compat)
```

Soft-SMMU SID **sticks to the queue**. `create_queue` / Soft-CP
`stamp_queue_sid` program it. If the queue has no SID yet, first
submit inherits `stream_for_job` (or a privileged SET_SID latch) and
sticks it — that is SID-at-submit (Host1x-shaped doorbell write;
landed). An empty queue may restamp from SET_SID between jobs; a
pending queue refuses a foreign SID (`Fault` / `Busy`). Pins on
another SID are still `Fault`. `CpCmd` layout is unchanged
(`stream_id` at 0x10); `CP_FLAG_SET_SID` marks the job-head stamp.

Two queues (`SOFT_CP_XQUEUES = 2`, depth 4). Freeze A and B keeps
DMA under B’s SID (blast-radius). Priority picks among Running
queues; suspend is how A loses the engine at the next boundary.

Host tests: `xqueue_suspend_a_b_progresses_blast_radius`,
`xqueue_wrong_sid_still_aborts`, plus the existing wrong-stream
pack tests.

## SoftChipletSync (software; Fleet-shaped)

**Status:** **Landed** (PR #51; SpectraScout post-M2 leftover after M3+M4).
Scoped timelines `{wave, CU, chiplet, package}` on the existing seq /
wait / complete model. Not a Vulkan timeline product. Not UCIe.
Not ChipletFleet placement (`ChipletTaskScope` stays a killed calendar
stub). SoftCCT (below) is the elision layer on this object.

**Inspiration.** [Fleet](https://arxiv.org/abs/2604.15379) hierarchical
event counters: workers increment a chiplet-local counter with **no**
package fence; only the last worker on a participating chiplet issues a
package-scope fence. Chiplet-local signal is free; package-scope costs
more.

This is **not** a Fleet port and not a multi-chiplet latency result.
Host tests measure fence **counts** (package ≪ naive global).

**Contract** (`aether_core::chipsync::SoftChipletSync` + Soft-CP /
IreeShapedCp `submit_scoped`):

```text
open(scope) / expect(chiplet, n_workers)
arrive(chiplet, write_label)   // chiplet-local free; last worker may fence
wait(consumer, read_label)     // SoftCCT elides if last-writer == consumer
Soft-CP submit_scoped(queue, job, scope, write, read)
IreeShapedCp submit_scoped     // still a single mailbox
```

`submit_xqueue` / `AccelDevice::submit` stay unscoped (SID / XQueue
intact). `CpCmd` / `IreeHalCmd` layouts unchanged. Path B SoftNPU /
`make qemu` unchanged.

Host tests: `package_scope_fence_count_much_less_than_naive`, Soft-CP
two-fake-chiplet producer/consumer, IreeShapedCp sequential
producer/consumer. Kernel serial `[chipsync]`.

## SoftCCT (software; CPElide-shaped elision)

**Status:** **Landed** (SpectraScout leftover on SoftChipletSync).
Soft-CP / IreeShapedCp jobs carry buffer labels. SoftCCT
(`aether_core::chipsync::SoftCct`) tracks the last-writer chiplet per
label and tells SoftChipletSync to issue a package-scope fence **only**
when that table says a cross-chiplet hazard. Same-chiplet consume on a
≥2-chiplet package **elides**. Single-chiplet CCT is a **no-op**.

**Inspiration.** [CPElide](https://doi.org/10.1109/MICRO61859.2024.00058)
(MICRO’24): the command processor tracks last-writer chiplet per buffer;
targeted acquire/release vs all-chiplet (broadcast) fences.

This is **not** CPElide silicon, **not** a full coherence protocol,
**not** a cache-coherence directory, and **not** a Vulkan / ROCm
product. Host tests measure fence **counts** (CCT package ≪ broadcast
baseline). Latency wins need a multi-chiplet sim — single-die QEMU /
host numbers are not partner proof. An incorrect-elision test (elide
whenever a label is known, ignoring writer chiplet) must fail on
chiplet0 → chiplet1.

Host tests: `softcct_package_fence_lt_broadcast_two_chiplet`,
`softcct_incorrect_elision_fails`, `softcct_single_chiplet_is_noop`,
plus Soft-CP twins. Kernel serial `[softcct]`.

## SoftGreenCtx (software; Green Contexts / DetShare-shaped)

**Status:** **Landed** (SpectraScout M5 digest #1, after M3+M4+SoftChipletSync).
Soft-CP partitions a **fake** SM / work-queue pool. XQueues bind to a
`SoftGreenCtx`. Soft-SMMU SID is unchanged across migrate-to-yield.
Not HW MIG. Not a BAR firewall.

**Inspiration.**

- [CUDA Green Contexts](https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__GREEN__CONTEXTS.html):
  a lightweight context that owns a subset of SMs and work queues.
  Streams bind to that context. Soft partition — even disjoint SMs do
  **not** isolate L2 / HBM.
- DetShare (arXiv:2603.15042): virtual contexts bound to physical Green
  Contexts with SM quotas; **migrate-to-yield** rebinds a queue to a
  larger partition at a queue boundary. DetShare has **no** public repo.
  GC is a real HW API; this crate is a software model.

**This is not:**

- Hardware MIG, a BAR firewall, or silicon SM isolation.
- A CUDA driver, an NVIDIA partnership, or a DetShare port.
- A FLOP / partner-bandwidth claim. Host tests measure memcpy-like
  **normalized integer BW** (bytes / SM-scaled cycles) vs an
  unpartitioned baseline. Residual shared-HBM tax stays on so a
  partitioned 70% slice is still below solo.
- A change to path-B SoftNPU, the virtqueue BAR, or `make qemu`.
  `IreeShapedCp` stays a single mailbox (no SoftGreenCtx).
- A `CpCmd` layout change. Memcpy-like jobs are Soft-CP `Nop` + byte
  count, retired through the existing IRQ/poll path.

**Contract** (`aether_core::greenctx::SoftGreenPool` + Soft-CP /
`aether_hal::AccelDevice`):

```text
AccelInfo { sm_count: 10, wq_count: 10 }   // Soft-CP only; others 0
sm_wq_budget()                             // AccelDevice capability
split_green_70_30() / share_unpartitioned()
bind_green_ctx(queue, ctx)
submit_memcpy(queue, bytes)                // memcpy-like; not SoftNPU
service()                                  // SM-share cycles + residual tax
migrate_to_yield(queue, dest)              // queue-boundary; SID sticks
```

Canonical clip: fake pool **10 SM / 10 WQ**, exclusive **70/30** (7+3).
Two XQueues bind to those partitions, co-run memcpy-like kernels, and
report BW vs the unpartitioned 50/50 share. Queue A may yield and
migrate onto the larger slice; `StreamId` does not change.

Host tests: `memcpy_interference_partitioned_beats_unpartitioned`,
`greenctx_two_queue_70_30_memcpy_beats_unpartitioned`,
`greenctx_migrate_to_yield_sid_unchanged`. Kernel serial `[greenctx]`.

## SoftSFI (software; GPU-AToLL-shaped)

**Status:** **Landed** (SpectraScout M5 leftover). Toy Soft-CP ISA
(`load` / `store` / `add` / `dma`) with an SFI verifier. Every
load/store/dma proves `base+bound` sits in the SID-allowed IOVA
range. Not an NVVM pipeline. Not CUDA.

**Inspiration.** [GPU-AToLL](https://github.com/AERO-Project-EU/gpu-atoll)
hardens NVVM-IR so each memory side-effect proves a distinct location
and PTX state space before a tenant kernel may run. SoftSFI borrows
that *shape* on Soft-CP bytecode: abstract-interpret registers
(`r0 = 0`, `Add` / `AddImm` refine constants) and refuse a memory op
whose base is not a proved constant (`UnknownBase`) or whose span
escapes the SID window (`Oob`).

**This is not:**

- An NVVM / LLVM pass, a PTX rewriter, or a CUDA runtime.
- “Safe multi-tenant kernels” covering all side-effects. GPU-AToLL
  itself only claims memory isolation at validation time.
- A replacement for Soft-SMMU or SET_SID. SFI + SID stack: the
  verifier proves the span; Soft SMMU still walks the SID at
  execute. Fault injection skips the static verifier; the SID
  sandbox still traps and does not cross-read.

**Honest TODOs (stay open):**

- Atomics (`ATOMIC_ADD`) are **refused**, not modeled.
- Tensor copies / SoftNPU `MatMul` / `Wave` / TMA-shaped ops are
  **refused**, not modeled.
- Heap / dynamic allocation is not a sandbox (no heap in this ISA).

**Contract** (`aether_core::softsfi` + `drivers/src/softsfi.rs` on Soft-CP):

```text
verify(program, SidSandbox::from_iommu(sid))
  Load/Store: prove [rs+imm, +4) ⊆ SID IOVA window
  Dma:        prove src and dst spans ⊆ window
  Add/AddImm: no memory; refine constants
  Atomic/Tensor/unknown: Unmodeled
Soft-CP submit_sfi(sid, program)     // verify then execute
Soft-CP inject_sfi_skip_verify(...)  // runtime SID trap only
```

`CpCmd` / XQueue / SET_SID / SoftChipletSync stay unchanged. Path B
SoftNPU / `make qemu` unchanged. Two tenants on the same Soft-CP use
distinct SIDs; host tests accept in-bounds, reject OOB, and show
skip-verify does not leak tenant B.

Host tests: `verifier_accepts_in_bounds_program`,
`verifier_rejects_oob`, `verifier_rejects_atomics_and_tensor`,
`softsfi_two_tenants_fault_inject_no_cross_read`. Kernel serial
`[softsfi]`.

## PASID / SVA (software; Linux SVA-shaped)

**Status:** Software bind + SSID TLB invalidate on Soft SMMU (Linux
SVA-shaped). Not marked Done on [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md)
(H2 2026 exploration). Per-`AccelDevice` PASID
space. Bind process mm ↔ SSID; Soft-CP DMA uses that process VA;
host unmap invalidates the SSID ATC (TLB); a skipped invalidate is
a stale translate. Software only.

**Inspiration.** Linux SVA (`iommu_sva_bind_device`) plus PASID-tagged
DMA: a process address space is bound to a device context, DMA uses
the CPU VA, and `unmap` / `mmu_notifier` must invalidate the IOMMU
TLB for that PASID. Soft SMMU borrows that *shape* on a software CD
(`MmId` on the SSID). The PASID *is* the software SSID on that
AccelDevice's `IommuMap`.

**This is not:**

- ARM SVA, PCIe PASID/PRI, or a hardware ATS/PRI implementation.
- CUDA unified virtual addressing / UVA, or a default unified VA.
- Zero-copy SVA **without** the unmap → SSID TLB invalidate path.
  That pairing is the product rule; the negative test is a stale ATC
  hit when invalidate is skipped.
- A hardware SMMU. SID-at-submit and XQueue are unchanged. No new
  syscall (host / kernel-internal `bind_mm` / `map_va` / `unmap_va`).

**Contract** (`aether_core::sva` + `IommuMap` + Soft-CP):

```text
IommuMap::bind_mm(Memory+MAP, sid, MmId)   // PASID = SSID on this device
IommuMap::map_va(sid, va, guest_pa, len)   // S1 VA = process VA
Soft-CP pack / service                     // DMA address is that VA
IommuMap::unmap_va(sid, va)                // drop S1 + InvCmd::CfgCd
IommuMap::unmap_va_keep_atc(...)           // fault injection; ATC stale
```

Each AccelDevice owns an `IommuMap`; that table *is* the PASID space
(two Soft-CPs may bind the same `MmId` to different SSIDs). Path B
SoftNPU / `make qemu` still uses allocated IOVAs. `CpCmd` layout
unchanged (`iova_*` holds the VA when SVA is bound).

Host tests: `sva_demo_bind_dma_unmap_stale`,
`soft_cp_dma_uses_process_va`,
`host_unmap_invalidates_ssid_tlb_then_stale_service_faults`,
`skip_invalidate_queued_cmd_stale_translate`,
`two_acceldevices_have_independent_pasid_spaces`. Kernel serial
`[sva]`.

## OperatorInject (software; GPUOS / Mirage MPK-shaped)

**Status:** this leftover slice (H2 2026 on [TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md);
**not** marked Done). Soft-CP
keeps **one resident worker**. The host publishes operator slots on a
versioned function table. `memcpy` and `saxpy` seed at CP construct;
`scale` **hot-adds** without Soft-CP restart (`epoch` / `launches`
unchanged). SID is still enforced per submit. SoftCmdFirewall
copy-then-validate admits the packed `CpCmd` before dispatch. Own
bytecode / IR only.

**Inspiration.** [GPUOS / XpuOS](https://github.com/XpuOS) is a
resident GPU-side service: the host publishes work into a live worker
instead of relaunching a kernel per fused op.
[Mirage](https://github.com/mirage-project/mirage) MPK (persistent
kernel) fuses tensor ops into a resident GPU kernel so a new fused
graph does not relaunch CUDA. Soft-CP copies **neither** — a software
table + loop. Distinct from landed `OperatorKernelHandle` Hodge
inject.

**This is not:**

- NVRTC, CUDA, a vendor compiler, or NVIDIA.
- A full LLM compiler or Mirage superoptimizer.
- A `CpCmd` layout change or a second SoftNPU opcode. Calls pack a
  MatMul-shaped reloc for firewall / SID walks; the resident worker
  interprets the 32-byte `OpCall` IR.
- A change to path-B SoftNPU, the virtqueue BAR, or `make qemu`.

**Contract** (`aether_core::opinject` + `drivers/src/opinject.rs` on Soft-CP):

```text
OperatorInject::with_resident_memcpy_saxpy()  // one loop; slots 0,1
publish(slot, kind) / hot_add_scale()         // table_version++; epoch stays
submit(call, SidSandbox, mem)                 // SID window on src/dst
Soft-CP submit_injected(sid, call)            // SET_SID + firewall + dispatch
```

Host tests: `memcpy_and_saxpy_on_resident_worker`,
`hot_add_scale_does_not_relaunch`,
`memcpy_saxpy_then_hot_add_scale_without_relaunch`,
`sid_at_submit_refuses_unbound_and_foreign_iova`,
`firewall_copy_then_validate_on_inject`. Kernel serial `[opinject]`.

## SoftNoI-IS (software; PARL / NoI-shaped admit)

**Status:** **In-flight / landing this PR** (H2 2026 exploration on
[TWO_YEAR_PLAN.md](TWO_YEAR_PLAN.md)). Do not mark calendar Done
until merge. Not a Month 5 digest. SoftChipletSync advertises a
per-tenant Interference Score on a **fake** shared
Network-on-Interposer. Soft-CP XQueue admit refuses when the
projected IS exceeds the budget (canonical **1.5×**). Two Soft-CP
tenants.

**Inspiration.** [PARL / NoI](https://arxiv.org/abs/2510.24113)
(“Taming the Tail”) defines
`IS = max_k T_solo(k) / T_con(k)` — worst-case concurrent/solo
slowdown — and uses it as a **topology-synthesis** objective. SoftNoI-IS
reuses the *metric* as **runtime admit control**.

**This is not:**

- PARL topology synthesis, UniCNet, or optimal NoI design.
- A silicon interposer, UCIe PHY, or a multi-chiplet sim.
- A FLOP / partner-bandwidth claim. Host tests measure integer
  throughput units (`T_solo` / `T_con`) and admit/refuse counters.
- A change to path-B SoftNPU, the virtqueue BAR, or `make qemu`.
  `IreeShapedCp` stays a single mailbox (no XQueue admit).
- A `CpCmd` layout change or a new syscall. Default
  `submit_xqueue` is ungated; `submit_xqueue_noi` is the admit path.
  SoftNoI is **off** by default (SID / XQueue / CCT unchanged).

**Contract** (`aether_core::noi::SoftNoI` + SoftChipletSync + Soft-CP):

```text
SoftChipletSync.noi / enable_noi(true)
project_is_milli(tenant, demand)     // max(1.0, sum/capacity)
admit(tenant, demand)                // refuse if projected IS > 1.5
advertised_is_milli(tenant)          // per-tenant estimate
Soft-CP submit_xqueue_noi(queue, job, demand)
```

Canonical clip: fake capacity **1000**, budget **1500 milli**. Two light
demands (400+400) stay under capacity (IS = 1.0) and **admit**. Two
heavy demands (800+800) project IS = 1.6 and **refuse** the second
tenant. Solo vs concurrent throughput is the same integer model.

Host tests: `admit_light_two_tenants_refuse_heavy`,
`heavy_concurrent_is_over_budget`,
`softnoi_solo_vs_concurrent_then_xqueue_refuse`. Kernel serial
`[softnoi]`.

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

v1 decode keys off the **DISPATCH** bit first. `categories = 0` is Nop
and **ignores** `function` (pack writes 0). v1 pack emits **0 or
DISPATCH only**; `TRANSFER` alone is `HalError::Fault` / not defined.
`workgroup_count_*` are AccelJobDesc `m,n,k` shape stand-ins — not
compiler tile sizes or IREE launch geometry. Binding `.length` fields
are dtype-aware **byte spans**, not element counts. The only accepted
`isa_blob_id` is `IREE_REF_EXECUTABLE` (`0x0001EE00`).

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
0x24    u32    stream_id             SET_SID operand / Soft-SMMU StreamId (ssid=2)
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
3. submit(): SET_SID then IreeHalCmd::pack_on → mailbox, doorbell=1.
   PJRT host path: abi nouns → pack IreeHalCmd → submit_hal (arms
   SET_SID from `stream_id`). Unbound / captured / missing / partial /
   wrong-stream / no SET_SID → Fault. TRANSFER-only image → Fault.
   Other isa_blob_id → Unsupported. Does not execute. poll() is empty
   until service().
4. service() (IRQ / kthread poll): resolve_submit each binding IOVA
   (armed SID must match the packet), run the integer engine, write a
   Completion, raise IRQ, clear the submit latch.
5. poll(): pop the completion and ack the IRQ. signal_payload (fence
   seq) retires through Timeline::complete / retire_into.
6. Never accept a PA that did not come from a cap walk + IommuMap pin.
```

The kernel self-check only probes `IreeShapedCp` so backend 4 is
visible on the serial log (`[accel] IreeShapedCp probe backend=4
iree-shaped-cp (IREE HAL packet; not a vendor)`). QEMU still demos
SoftNPU. This backend does not add a QEMU device.

The compiler-facing nouns on this path live in `host/aether-pjrt`
([HOST.md](HOST.md)): Device, MemorySpace, Buffer, Executable, Event
lower onto `AccelJobDesc` + Soft SMMU. The host session submits through
SoftNPU and `IreeShapedCp` (`backend = 4`), not Soft-CP. That crate is
not a PJRT plugin, not an IREE HAL driver, and not a vendor runtime.

## ADR: Host1x-shaped SET_SID at submit

**Status:** Accepted 2026-09-07.

**Context.** SpectraScout post-M2 #1: Soft SMMU already binds SIDs at
map. A Host1x-shaped CP programs StreamID at the **job head** (doorbell)
so DMA cannot start on a Bound-but-not-submitted SID. NVIDIA Tegra
Host1x is the inspiration for “first command sets the channel SID.”

**Decision.** Extend Soft-CP (`CpCmd.stream_id`, `CP_FLAG_SET_SID`) and
IreeShapedCp (`IreeHalCmd.stream_id`) — no second IR, no packet-offset
change, no new syscall.

```text
map / bind_stream (Memory+MAP)     // STE+CD Bound; SID pool charged
set_sid / submit auto-arm          // job-head SET_SID; latch armed
XQueue inherit / stick             // first submit or empty-queue restamp
pack stream_id = queue SID         // existing field; Host1x operand
service: resolve_submit            // re-arm from packet; WrongStream
clear_submit_sid                   // end of job / FLR / unbind
```

- Privileged `IommuMap::set_sid` requires Memory+MAP and a Bound SID
  owned by that tenant. Soft-CP `submit_xqueue` uses `set_sid_bound`
  and sticks the SID on the XQueue (bind already walked the cap).
  `IreeShapedCp` stays a single mailbox.
- Two tenants / two SIDs: host tests in `core/src/sid.rs`,
  `drivers/src/{fakecp,ireecp}.rs`; optional QEMU serial `[sid]`.
- Limited pool: `SID_BUDGET_PER_TENANT = 4`. `SidBudget` on the 5th
  Bound CD. Process/tenant-level contexts.
- Fault injection: `inject_wrong_sid` after submit; service status -2.

**Not claimed.** This is **not** a Tegra Host1x driver, not a Host1x
class opcode ROM, not a silicon stream-ID allocator, and **not**
hardware-grade isolation. Soft SMMU remains a software table. A real
SMMU / Host1x still needs partner silicon, a SID budget that matches
the part, and broader fault-injection than these host tests.

## ADR: SoftCmdFirewall (copy-then-validate)

**Status:** Accepted 2026-09-07.

**Context.** SpectraScout M5 digest #2. Tegra Host1x DRM taught a
hard lesson: if the kernel validates a userspace command buffer
**in place**, a client can rewrite opcodes, relocs, StreamID, or
addresses after the check and before enqueue. The engine then sees
the mutated stream. GPU-CC HMAC over a kernel copy is optional
later integrity — **not** confidential GPU / HBM encryption, and
not NVIDIA SEC2.

**Decision.** Soft-CP submit copies the packed `CpCmd` image into a
kernel-owned arena (`SoftCmdFirewall`), then validates the **copy**
(opcodes, IOVA relocs, SID, Soft-SMMU addr caps), then enqueues the
copy. `submit_cmdbuf` is the userspace-image path; `submit_xqueue`
packs then admits through the same arena. IreeShapedCp
`submit_hal` round-trips the frozen image the same way (parse the
copy). Not a Host1x class ROM and not a second IR.

```text
client image  ──copy──►  kernel arena
                          │
                     validate copy
                     (op / reloc / SID / IOVA cap)
                          │
                     enqueue copy
```

- In-place validate-then-re-read is a **test-only** hole
  (`FirewallMode::ValidateInPlace`) so the race is unit-testable.
  Default submit is copy-then-validate. Mutation during the
  window is ignored.
- Relocs are the packet IOVA fields (`iova_a/b/c/bias`). Addr cap:
  refuse identity guest PAs (`< SOFT_SMMU_IOVA_BASE`) sneaking into
  the stream. SID must be Soft-CP `ssid = 1` and match the queue.
- `FirewallSim.{copy_steps,validate_steps}` is a software step
  count. **Not** a vendor microsecond claim.

Host tests: `drivers/src/firewall.rs` (race sneak / hold / refuse)
and Soft-CP golden + cmdbuf tests in `fakecp.rs`. Kernel serial
`[firewall] copy-then-validate race sealed`.

**Not claimed.** Not confidential GPU. Not HBM encryption. Not
GPU-CC / SEC2. Not a Tegra driver. Soft SMMU is still software.

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
all retire through this API. SoftChipletSync adds scoped (wave / CU /
chiplet / package) timelines on the same seq model; SoftCCT is the
CPElide-shaped elision layer (not a coherence protocol, not Vulkan /
ROCm). QEMU's used-ring IRQ is still software on x86
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
