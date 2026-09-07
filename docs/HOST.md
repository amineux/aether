# Partner compiler contract (PJRT / IREE HAL shaped)

This is the host-side contract a compiler runtime would speak. It is
**not** a PJRT plugin, **not** an IREE HAL driver, **not** a vendor
runtime, and **not** a partnership with OpenXLA, IREE, or any silicon
vendor. The vocabulary is public and borrowed on purpose so a compiler
team can map their existing nouns onto Aether without an in-kernel
graph IR.

The working crate is `aether-pjrt` (`host/aether-pjrt`). It is a `std`
workspace member. Host tests create a device, allocate typed buffer
places, pin through Soft SMMU, submit `MatMul` / `Wave`, and wait on
an event/fence. The **IREE/PJRT contract** consumes a frozen 96-byte
little-endian `IreeHalCmd` (magic `0xAE7E1EE1`, `backend = 4`,
`ssid = 2`) and submits through `IreeShapedCp`. SoftNPU is an extra
host backend for virtqueue tests; `make qemu` remains the path-B
SoftNPU demo. Soft-CP (`backend = 3`) stays in-tree as the
Aether-native packet path; this crate does not submit through it.
There is no fake vendor runtime and no `PartnerNpuStub` path.

Kernel CI is unchanged: the crate is not linked into `aether-kernel`.
`cargo test --workspace` runs its tests on the host.

## Public vocabulary (cited, not claimed)

| Noun | Public source | Aether host object | Lowers to |
| --- | --- | --- | --- |
| Client | OpenXLA PJRT [`PJRT_Client`](https://openxla.org/xla/pjrt) | `aether_pjrt::Client` | owns one `AccelDevice` + partition timeline |
| Device | PJRT `PJRT_Device`; IREE [`iree_hal_device_t`](https://github.com/iree-org/iree/blob/main/runtime/src/iree/hal/device.h) | `abi::Device` + `AccelInfo` | `Activity` on the fabric, not `/dev` ioctl |
| Memory / MemorySpace | PJRT `PJRT_Memory`; IREE `iree_hal_memory_type_t` (`HOST_LOCAL`, `DEVICE_LOCAL`, …) | `space::MemorySpace` | `HOST`, `DEVICE_HBM`, `TILE_SRAM`, `CXL_REGION`, … — typed places, not a unified VAS |
| Buffer | PJRT `PJRT_Buffer`; IREE [`iree_hal_buffer_t`](https://github.com/iree-org/iree/blob/main/runtime/src/iree/hal/buffer.h) | `abi::Buffer` | `(place, local)` + Soft-SMMU IOVA |
| Executable | PJRT `PJRT_Executable` / `PJRT_LoadedExecutable`; IREE [`iree_hal_executable_t`](https://github.com/iree-org/iree/blob/main/runtime/src/iree/hal/executable.h) | `abi::Executable` | opaque `isa_blob_id`; IreeShaped frozen id is `0x0001EE00` (`IREE_REF_EXECUTABLE`); refuse any other |
| Event / Fence | PJRT `PJRT_Event`; IREE `iree_hal_event_t` / `iree_hal_fence_t` | `abi::Event` | `FenceId` on a CP-shaped `Timeline` |

IREE's HAL device is the handle that allocates buffers, prepares
executables, dispatches work, and synchronizes with the host
([C API](https://iree.dev/reference/bindings/c-api/)). PJRT is the
uniform device API frameworks call so each backend can stay opaque
([PJRT overview](https://openxla.org/xla/pjrt)). Aether's kernel is
the same kind of object: a **submission shim + resource solver**.
Compilers own the ISA, graph IR, and fusion.

This crate does **not** export `GetPjRtApi`, does **not** implement
`iree_hal_driver_t`, and does **not** load HSACO / PTX / a vendor
ISA blob. Those bytes stay compiler-owned (`isa_blob_id` is a handle).

## Lowering

```text
Client::create(SoftNpu | IreeShaped)
    probe() → AccelInfo                    // IREE device query / PJRT_Client devices
    allocate(MemorySpace, len)
        bump guest PA in the host heap
        IommuMap pin (Memory+MAP)          // Soft SMMU; IOVA ≠ identity
        → Buffer { space, (place, local), unified=false }
    load_executable(op, dtype)
        → Executable { isa_blob_id }       // no graph parse
                                           // IreeShaped: 0x0001EE00 only
    execute(executable, A, B, C [, bias])
        Timeline::submit → FenceId
        map abi::{Device,Buffer,Executable,Event}
        IreeShaped:
            pack IreeHalCmd (magic 0xAE7E1EE1, 96-byte LE, ssid=2)
            workgroup_count = AccelJobDesc m,n,k (shape, not tile sizes)
            binding.length = dtype-aware byte spans (not element counts)
            categories = 0 (Nop) or DISPATCH; TRANSFER alone is Fault
            IreeShapedCp::submit_hal
        SoftNpu:
            AccelJobDesc → virtqueue submit   // path-B qemu demo
        → Event { fence, partition }
    wait(event)
        service()                          // host IRQ pump (SoftNPU used-ring / IreeShapedCp mailbox)
        AccelDevice::poll
        Timeline::complete / retire_into
        Timeline::wait                     // watermark; not a CUDA stream
```

`AccelJobDesc` is the architectural dispatch record (see
[ACCEL.md](ACCEL.md)). IreeShapedCp consumes the frozen 96-byte
`IreeHalCmd` (IREE HAL nouns, Soft-SMMU `ssid = 2`). SoftNPU writes
Soft-SMMU IOVAs into the virtqueue. Both refuse a pin without a
Memory+MAP cap walk. Soft-CP's Aether-native `CpCmd` is a separate
AccelDevice, not this crate's submit path. There is no graph IR.

Host copies (`copy_from_host` / `copy_to_host`) are explicit transfers
in the sense of PJRT `BufferFromHostBuffer` / `ToHostBuffer` and IREE
`iree_hal_device_transfer_*`. They are not a coherent CPU load of
HBM or tile SRAM. `UNIFIED` stays off unless that cap bit is granted
(it is not granted here).

## What this cut will not claim

- An OpenXLA PJRT plugin (`GetPjRtApi`) or an in-tree IREE HAL driver.
- A vendor compiler integration or a signed silicon partnership.
- In-kernel ML graph IR / fusion (`Wave` is a stand-in dispatch).
- A CUDA stream, a default unified virtual address space, or CXL.mem.
- Hardware SMMU (the pin is the software STE→CD→S1/S2 table).
- That `make qemu` uses this crate. The guest still submits through
  syscalls onto path-B SoftNPU. This is the **host** contract tests
  exercise against the same `AccelDevice` implementations.

Walkthrough for a silicon OS team: start at
`aether_hal::AccelDevice` and `IreeShapedCp`, then this crate
as the compiler-facing nouns on top. `SoftCommandProcessor` remains
the Aether-native packet example. `PartnerNpuStub` is a leftover
no-op sketch, not this path. [ABI.md](ABI.md) is the kernel ABI;
[DILIGENCE.md](DILIGENCE.md) is the leave-behind.
