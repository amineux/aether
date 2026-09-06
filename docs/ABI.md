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
- Order submit → fence → complete / timeout.
- Move Memory caps and pin local places.
- Account Hodge flow class and SpectralCut placement.

## What the kernel will not do

- Lower, fuse, or canonicalize a tensor graph.
- Invent a CUDA stream or a default unified virtual address space.
- Treat UCIe / EMIB / CXL as a programming model. Those are transports;
  the programming model is activities, places, and fences.

A runtime (IREE, XLA/PJRT, a vendor compiler) compiles to the tile ISA,
then submits `AccelJobDesc` records through an `Activity` endpoint.
`Wave` in v0.1 is a software stand-in for one compiled dispatch, not a
fusion pass.
