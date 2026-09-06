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
| `MatMul` | `C = A @ B` (i32 reference) |
| `Wave` | matmul + optional bias (stand-in for a fused wave) |

`F16` / `F32` are **STUB** (no libm / no hard-float in the kernel).

## Virtqueue MMIO layout (in-kernel BAR)

There is **no** upstream `virtio-accel` device. A custom QEMU fork is
still out of scope. The kernel emulates a virtqueue-shaped MMIO window
(`aether_drivers::mmio::AccelMmio`, 1 KiB):

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

## Map API (Soft SMMU; not hardware)

`aether_core::iommu::IommuMap` is a **software** stream-ID page table:

```text
map(Memory cap + MAP, guest_pa, len, stream_id) -> MappedRegion { iova != guest_pa }
translate_stream(sid, guest_pa) -> iova
resolve_stream(sid, iova)       -> guest_pa
covers_stream(sid, pa, len)     -> bool
unmap / unmap_stream / unmap_for
```

Rules:

1. Refuse unless `cap.kind == Memory` and `cap.rights` contains `MAP`.
2. Each `stream_id` is its own IOVA namespace. Two streams may pin the
   same guest PA to different IOVAs. Same-stream guest-PA overlap is
   `Overlap` (or `CrossTenant` if another tenant holds the window).
3. IOVAs come from a per-stream bump allocator above 4 GiB
   (`SOFT_SMMU_IOVA_BASE`). This is not identity and not a hardware SMMU.
4. Translate / unmap that name the wrong stream return `WrongStream`.
   Unmap authorized by the wrong tenant is `CrossTenant`. Unmapped is
   `NotMapped`.
5. `AccelDevice::map` without a prior cap walk returns `NoMemoryCap`.
   Use `SoftNpuDevice::map_with_cap` / `IommuMap::map`. SoftNPU DMA
   uses stream 0: submit writes IOVAs into the virtqueue; `service`
   resolves IOVA → guest PA before the software engine loads.

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

`aether_core::SoftNpu` is a deterministic integer engine:

- shapes up to 64×64 (prototype bound)
- overflow → `AccelError::Overflow`
- `Wave` adds an optional bias vector

It is a **model of a matmul/wave engine**, not a product NPU. The point is
that job submit, ownership, and completion look like silicon.

## How to plug a real NPU

Start from `aether_drivers::PartnerNpuStub` (no-op backend, `backend = 2`).
That sketch shows how opcode / dtype / route fields become a command
packet (`PartnerCmd`). Then:

```text
1. PCI/MMIO probe; fill AccelInfo { backend: 2, vendor, device, ... }.
2. On Memory cap + map(): program the IOMMU / SMMU and the device's
   page table / stream IDs. Refuse if the cap lacks MAP or the tenant
   does not own the arena. Soft SMMU is the software table; a real
   device still needs a hardware SMMU.
3. On submit(): PartnerCmd::from_job(job, &iommu) → chip command packet
   (opcode, dtype, route_chiplet/tile, IOVAs). Ring the doorbell.
   Do not execute in the syscall; wait for the used ring / IRQ.
4. On IRQ: read completion, AccelDevice::poll equivalent, then
   fabric.send(REPLY) to job.completion_ep.
5. Never accept a PA that did not come from a cap walk + IommuMap pin.
6. Honor BankColor: Compute waves stay on the painted bank unless the
   caller transferred ownership or submitted Phase::Exchange.
```

TODOs left in `PartnerNpuStub` on purpose: BAR probe, hardware SMMU SID,
MSI-X pop, partner opcode packing. Soft SMMU already records
`req.stream_id`. This is not a vendor partnership.

## Co-scheduling

`TileScheduler` has an `Npu` tile. Init enqueues an `AccelWave` with a
deadline, bank affinity, and arena color; `pick(npu0)` returns it before
a CPU thread, and refuses a foreign-colored Compute wave.
Work-stealing will not move a wave onto a CPU tile (`Job::compatible`).

Jobs are fence-ordered and credit-limited per `PartitionProfile`.
That is not a CUDA stream: there is no implicit catch-up, and a
partition that is out of credits refuses submit.

A later cut should:

- let a real device IRQ (not only kthread poll) complete the fence
- meter HBM bandwidth as the partition QoS budget already names
- replace Soft SMMU with a hardware SMMU page table (program a real SID)

Do not assume cache coherence across chiplets. SRAM on the tile is the
honest first place; HBM and CXL are other typed spaces, not a wafer-scale
flat address space.
