# Host ABI (PJRT / IREE HAL shaped)

The kernel stays a **submission shim + resource solver**. Compilers own
the ISA, graph IR, and fusion. Aether does not grow an in-kernel ML
intermediate representation.

The host-facing nouns match a PJRT / IREE HAL sketch:

| Object | Aether type | Role |
| --- | --- | --- |
| Device | `abi::Device` / `Activity` | A compute unit on the fabric, not an ioctl node |
| MemorySpace | `space::MemorySpace` | `HOST`, `DEVICE_HBM`, `TILE_SRAM`, `CXL_REGION`, `SCRATCH`, `STREAMING` |
| TypedWindow | `window::TypedWindow` | `{ base, len, kind: Hbm\|CxlMemStub\|Dram, sid }` — Soft SMMU pin stub; not CXL.mem silicon |
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
  [`TypedWindow`](../core/src/window.rs) (`CxlMemStub`) is an exploration
  stub for typed fabric memory, **not** a CXL.mem programming model.

A runtime (IREE, XLA/PJRT, a vendor compiler) compiles to the tile ISA,
then submits `AccelJobDesc` records through an `Activity` endpoint.
`Wave` in v0.1 is a software stand-in for one compiled dispatch, not a
fusion pass.

## Host crate (`aether-pjrt`)

`core/src/abi.rs` names the nouns. The working host session is
`host/aether-pjrt`: a `std` workspace crate that creates a device,
allocates typed buffer places, pins them through Soft SMMU, submits
matmul/wave, and waits on an event/fence. The IREE/PJRT contract
packs frozen `IreeHalCmd` and submits through IreeShapedCp. SoftNPU
is an extra host backend; `make qemu` stays path B. Not Soft-CP, not
a fake vendor runtime.

`examples/accel-client` is a second tiny host client of that same
96-byte image (doorbell sketch). It is not a PJRT plugin and not a
MicroPerceptron port; MicroPerceptron remains later and optional.

That crate (`aether-pjrt`) is the **partner compiler contract** sketched against public
PJRT / IREE HAL vocabulary. It is not a PJRT plugin, not an IREE HAL
driver, and not a partnership claim. See [HOST.md](HOST.md).

## Syscall numbers (frozen 0–10; 11 additive)

Ring-3 / U-mode uses the same numbers on every arch. x86_64 is the
System V / Linux register convention: `rax` = number, `rdi,rsi,rdx` =
args, `rcx`/`r11` clobbered by `syscall`. Negative `rax` is `-SysError`.
RISC-V U-mode uses the Linux RISC-V convention: `a7` = number,
`a0,a1,a2` = args, return in `a0` (negative is `-SysError`). Entry is
`ecall`; the kernel returns with `sret`. aarch64 EL0 uses the Linux
AArch64 convention: `x8` = number, `x0,x1,x2` = args, return in `x0`.
Entry is `svc #0`; the kernel returns with `eret`.

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
| 9 | `exit(status)` | x86 `isa-debug-exit`; RISC-V sifive_test; aarch64 Angel SYS_EXIT (additive; 0–8 unchanged) |
| 10 | `clone(entry, stack, flags)` | User thread on the caller's PML4 / satp / TTBR0. `flags` must be 0. Returns child tid. Child starts at `entry` with arg0 = tid and `rsp`/`sp` = `stack`. Additive; 0–9 unchanged. |
| 11 | `mmap(addr, len, flags)` | Anonymous grow. `flags` must be 0. `addr` 0 = first free page in the grow window; nonzero must be page-aligned and in that window. Returns VA. Additive; 0–10 unchanged. |

`SYS_CLONE` is a **documented subset**, not Linux `clone` and not
`fork`: no new address space, no TLS, no files, no `CLONE_*` flags.
`entry` and `stack` must sit in the same known user window (16-byte
aligned stack). `SYS_EXIT` is still guest-wide (isa-debug-exit /
sifive_test), so the child must not call it. World still has one
shared `CapTable`. `/probe` remains a second ELF with its own PML4
— that is not this syscall.

No new syscall for COW. The kernel maps one shared 4 KiB USER page
at `USER_COW_BASE` (`0x0280_0000`) read-only into x86 `/init` and
`/probe`. A write fault allocates a private copy on that aspace.
`user_range_known` accepts the page so a later copy helper can name
it; `SYS_CLONE` entry/stack must still sit in an ELF window.
Numbers **0–10 stay frozen**. RISC-V / aarch64 do not map the VA.

`SYS_MMAP` is a **documented subset**, not POSIX `mmap`: no file,
no `MAP_SHARED`, no `PROT_*` / `MAP_*` bits, no `munmap` of
individual pages (`SYS_UNMAP` stays a no-op). The kernel allocates
4 KiB frames and maps them USER+RW in the caller's PML4 / satp /
TTBR0. `addr` 0 is first-fit in a fixed grow window
(`USER_MMAP_BASE` `0x02C0_0000` on x86, after virtio-blk;
`USER_RV_MMAP_BASE` / `USER_AA_MMAP_BASE` after the ELF window).
64 KiB cap. SoftNPU stays on kernel CR3. Soft SMMU is unchanged
(these pages are not DMA-pinned). `SYS_CLONE` siblings share the
grown region; `/probe` has its own PML4 and does not. Numbers
**0–10 stay frozen**.

User blobs: `UserIpcMsg`, `UserAccelJob`, `UserCompletion` in
`core/src/sysnr.rs`. `/init` is granted CPtr 0 (endpoint) and CPtr 1
(accel queue) before the ring-3 drop.

`UserAccelJob` has no `dtype` field (wire unchanged). Ring-3 `/init`
submits I32. Additive `DType` values on `AccelJobDesc` / `CpCmd` /
`AccelJobWire` / `IreeHalCmd`: `I32=0`, `F16=1`, `F32=2`. `IreeHalCmd`
stores IREE `iree_hal_element_type_t` (`INT_32` / `FLOAT_16` /
`FLOAT_32`), not the Aether `DType` byte. Documented in
[ACCEL.md](ACCEL.md). Do not reshape `UserAccelJob` without a version
bump there.

## Boot ramfs (kernel-internal)

The ELF loader opens `/init` (and optional `/probe`) from an
in-kernel ramfs (`core/src/ramfs.rs`: `seed` / `open` / `read`).
Boot seeds those names from an x86 virtio-blk AETHFS01 image when
a drive is present, otherwise from the embedded ELF blobs. This is
**not** a user syscall: numbers **0–10 stay frozen**; `SYS_MMAP` (11)
is the only additive slot in this cut. No `SYS_OPEN` / `SYS_READ`.
SoftNPU stays the in-kernel BAR.
