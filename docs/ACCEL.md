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
submit(job: &AccelJobDesc) -> job_token
poll()   -> Option<Completion>     // IRQ/doorbell eventually calls this
map(pa, len)                       // pin / IOMMU map; requires Memory cap
```

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

## VirtIO-Accel (QEMU story)

There is **no** upstream `virtio-accel` device. Shipping a custom QEMU
fork is out of scope for v0.1. Instead we specified a virtio-shaped ABI
and implemented both sides in-tree:

```text
MMIO cfg (what a future -device virtio-accel would expose)
  0x00  magic     0xAE7EACC1
  0x04  version   1
  0x08  status    ACK | DRIVER | DRIVER_OK | FAILED
  0x0C  qsize
  0x10  doorbell  (write = kick)
  0x14  used_idx

Queue
  avail[]  AccelJobDesc
  used[]   { job_seq, status, cycles }
```

`aether_drivers::VirtioAccelQueue` is that ring. `SoftNpuDevice` binds it
to the software NPU so `make qemu` needs only stock QEMU.

A QEMU device team would:

1. Implement the MMIO block and a virtqueue.
2. DMA the job desc.
3. Either execute a model or forward to a plugin.
4. Write a used element and raise IRQ.

The kernel driver then swaps `SoftNpuDevice` for `VirtioAccelMmio` without
touching fabric or caps.

## SoftNPU

`aether_core::SoftNpu` is a deterministic integer engine:

- shapes up to 64×64 (prototype bound)
- overflow → `AccelError::Overflow`
- `Wave` adds an optional bias vector

It is a **model of a matmul/wave engine**, not a product NPU. The point is
that job submit, ownership, and completion look like silicon.

## How a real NPU driver plugs in

```text
1. PCI/MMIO probe; fill AccelInfo { backend: 2, ... }.
2. On Memory cap + map(): program the IOMMU / SMMU and the device's
   page table / stream IDs. Refuse if the cap lacks MAP or the tenant
   does not own the arena.
3. On submit(): translate AccelJobDesc into the chip's command packet
   (opcode, stride, dtype). Ring the doorbell.
4. On IRQ: read completion, AccelDevice::poll equivalent, then
   fabric.send(REPLY) to job.completion_ep.
5. Never accept a PA that did not come from a cap walk.
```

Do **not** map “all of HBM” into the NPU. The arena + cap is the point.

## Co-scheduling

`TileScheduler` has an `Npu` tile. Init enqueues an `AccelWave` with a
deadline and bank affinity; `pick(npu0)` returns it before a CPU thread.
Work-stealing will not move a wave onto a CPU tile (`Job::compatible`).

Jobs are fence-ordered and credit-limited per `PartitionProfile`.
That is not a CUDA stream: there is no implicit catch-up, and a
partition that is out of credits refuses submit.

A later cut should:

- block the submitting thread on SYNC + `accel_wait` (fence wait)
- let the NPU IRQ complete the fence and wake that thread
- meter HBM bandwidth as the partition QoS budget already names

Do not assume cache coherence across chiplets. SRAM on the tile is the
honest first place; HBM and CXL are other typed spaces, not a wafer-scale
flat address space.
