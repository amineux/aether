# Host ABI (PJRT / IREE HAL shaped)

The kernel stays a **submission shim + resource solver**. Compilers own
the ISA, graph IR, and fusion. Aether does not grow an in-kernel ML
intermediate representation.

The host-facing nouns match a PJRT / IREE HAL sketch:

| Object | Aether type | Role |
| --- | --- | --- |
| Device | `abi::Device` / `Activity` | A compute unit on the fabric, not an ioctl node |
| MemorySpace | `space::MemorySpace` | `HOST`, `DEVICE_HBM`, `TILE_SRAM`, `CXL_REGION`, `SCRATCH`, `STREAMING` |
| Buffer | `abi::Buffer` | Bound to one space + a `(place, local)` address |
| Executable | `abi::Executable` | Opaque `isa_blob_id`; kernel does not parse it |
| Event | `abi::Event` | A `FenceId` on a partition timeline |

## What the kernel will do

- Admit a job against a **partition** (spatial slice, credits, blast radius).
- Order submit → wait → complete on a CP-shaped timeline seq.
  `timeout` is a software overlay (releases a credit; not a device
  IRQ). `AccelJobDesc.fence_id` / `CpCmd` stay a `u64` seq.
- Move Memory caps and pin local places.
- Account Hodge flow class and SpectralCut placement.
- Bind an `OperatorKernelHandle` (collective topology × Hodge class)
  as a cap. No new syscall; host / kernel-internal API only.
- Sparsify that handle (or a FlowClass header) by dropping harmonic
  energy strictly below an integer milli threshold before inject.
  Hodge refuse is unchanged. No new syscall.

## What the kernel will not do

- Lower, fuse, or canonicalize a tensor graph.
- Invent a CUDA stream or a default unified virtual address space.
- Treat UCIe / EMIB / CXL as a programming model. Those are transports;
  the programming model is activities, places, and fences.

A runtime (IREE, XLA/PJRT, a vendor compiler) compiles to the tile ISA,
then submits `AccelJobDesc` records through an `Activity` endpoint.
`Wave` in v0.1 is a software stand-in for one compiled dispatch, not a
fusion pass.

## Syscall numbers (frozen 0–8)

Ring-3 uses the System V / Linux register convention: `rax` = number,
`rdi,rsi,rdx` = args, `rcx`/`r11` clobbered by `syscall`. Negative
`rax` is `-SysError`.

| nr | Name | Notes |
| --- | --- | --- |
| 0 | `debug_print(ptr,len)` | User pointer, max 256 bytes |
| 1 | `yield()` | Reschedule |
| 2 | `send(ep_cptr, msg_ptr)` | `require(Endpoint, WRITE)` then fabric.send |
| 3 | `recv(ep_cptr, msg_out)` | `require(Endpoint, READ)`; blocks if empty |
| 4 | `map(mem_cptr, vaddr, flags)` | `require(Memory, MAP)`; USER bit on 2 MiB |
| 5 | `unmap(vaddr, len)` | Accepted; no-op unmap in this cut |
| 6 | `accel_submit(queue_cptr, job_ptr)` | `require(AccelQueue, SUBMIT)` |
| 7 | `accel_wait(queue_cptr, cpl_out)` | `require(AccelQueue, WAIT)`; blocks |
| 8 | `arena_alloc(size, flags, bank)` | Mints a Memory cap |
| 9 | `exit(status)` | `isa-debug-exit` (additive; 0–8 unchanged) |

User blobs: `UserIpcMsg`, `UserAccelJob`, `UserCompletion` in
`core/src/sysnr.rs`. `/init` is granted CPtr 0 (endpoint) and CPtr 1
(accel queue) before the ring-3 drop.

`UserAccelJob` has no `dtype` field (wire unchanged). Ring-3 `/init`
submits I32. Additive `DType` values on `AccelJobDesc` / `CpCmd` /
`AccelJobWire`: `I32=0`, `F16=1`, `F32=2`. Documented in
[ACCEL.md](ACCEL.md). Do not reshape `UserAccelJob` without a version
bump there.
