# Aether

**[Marketing site](https://amineux.github.io/aether/)** — vision, architecture
visuals, two-year roadmap. Static HTML from [`site/`](site/); published by
GitHub Actions to Pages (kernel `docs/` are untouched).

**An accelerator-first fabric kernel** — a research prototype for how operating
systems should look when the package is a mesh of CPU, NPU, GPU, and custom
ASIC tiles rather than a host CPU with bolt-on devices.

> This is **not** production silicon, not a tutorial toy, and not a claim of
> partnership with any chip vendor. It is a bootable v0.1 whose *interfaces
> and invariants* are what we would pitch to an AI-chip OS team.

```
make test         # host unit tests (caps, fabric, arenas, scheduler, SoftNPU, L)
make qemu         # boot Aether in QEMU (x86_64 ring-3 /init)
make qemu-riscv   # same aether_core self-check on QEMU virt (thin S-mode port)
```

## Why this exists

Accelerators are activities on a capability fabric, not devices behind ioctl.
Traditional kernels treat GPUs and NPUs as PCIe endpoints: `ioctl`, a userspace
runtime, and a hope that the driver got cache flushing right. That model is
already strained on a discrete GPU. It breaks down on a **package** of chiplets
where:

- inference latency is a **deadline**, not a best-effort ioctl
- weights and KV caches are **multi-tenant secrets**, not files
- “shared memory” across tiles is often **not cache-coherent**
- the expensive resource is **HBM banks and NPU waves**, not CPU time slices

Aether inverts the picture. **Compute tiles are first-class peers** on a
capability-secured message fabric. CPU threads and accelerator waves are the
same kind of scheduled job. Tensor memory is an arena with explicit ownership
transfer. There is no IPC except capability-checked messages.

## Quickstart

**Host tests** (any `x86_64-unknown-linux-gnu` rustc 1.83+):

```bash
cargo test --workspace
```

**QEMU demo** (also needs `qemu-system-x86_64`, GNU `as`/`ld` with `elf_i386`,
`objcopy`, and `rustup target add x86_64-unknown-none`):

```bash
make qemu
```

Exact machine:

```bash
qemu-system-x86_64 \
  -kernel build/aether.elf \
  -serial stdio -display none \
  -no-reboot -no-shutdown -m 128M
```

You should see the trampoline enter long mode, a kernel self-check of the
host-identical fabric demo, then ring-3 `/init` over `syscall`:

```
[init] ring-3 /init (static ELF64 non-PIE @ 0x2000000)
[sched] kthread-B tick=…
FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE
  CUT BIND + HODGE FLOW CLASS ENFORCED
  TYPED SPACE + ACTIVITY ENDPOINT + FENCE-ORDERED JOB
  RING-3 /init VIA SYSCALL/SYSRET
```

The guest then exits QEMU via `isa-debug-exit` (status 1 means success).
`make qemu` treats that as a clean run. CI runs `make qemu-ci` (45s timeout).

**RISC-V virt** (`qemu-system-riscv64`, `rustup target add riscv64gc-unknown-none-elf`):

```bash
make qemu-riscv
```

```
qemu-system-riscv64 \
  -machine virt -cpu rv64 -m 128M -nographic \
  -no-reboot -kernel build/aether-riscv.elf
```

OpenSBI loads the ELF at `0x80200000`. The trampoline identity-maps
4 GiB (Sv39), then the same kernel self-check runs (fabric, cut, map,
bank color, Laplacian). This is a **thin port**: kmain + serial +
`aether_core`, not ring-3. Success writes `0x5555` to the virt test
finisher.

## Architecture

```mermaid
flowchart TB
  subgraph tasks [Tasks / tenants]
    TA[Tenant A<br/>init / runtime]
    TB[Tenant B<br/>isolated]
  end

  subgraph kernel [Aether kernel]
    Caps[Capability spaces]
    Fabric[Fabric IPC<br/>sync + async endpoints]
    Sched[Tile scheduler<br/>prio + deadline + steal]
    Arena[Tensor arenas<br/>bank-aware / pinned DMA]
    HAL[Accel HAL]
    Obs[Event ring]
  end

  subgraph tiles [Tiles on the package]
    CPU[CPU tiles]
    NPU[NPU / wave engine]
    GPU[GPU]
    ASIC[Custom ASIC]
  end

  TA -->|CPtr only| Caps
  TB -->|cannot forge A's caps| Caps
  Caps --> Fabric
  Fabric --> Sched
  Arena -->|ownership transfer| Fabric
  Sched --> CPU
  Sched --> NPU
  HAL --> NPU
  HAL --> GPU
  HAL --> ASIC
  Fabric --> Obs
  Sched --> Obs
  HAL --> Obs
```

| Piece | Role in v0.1 |
| --- | --- |
| **Fabric IPC** | seL4-inspired caps; sync/async endpoints; cap grants; chiplet route tags; `FlowClass` + Hodge quotas |
| **Tile scheduler** | CPU `Thread` and NPU `AccelWave` jobs; priority + deadline boost; bank affinity; work-steal; **SpectralCut** placement refusal |
| **Tensor arenas** | NUMA/bank first-fit; 4K / 2M align; pinned DMA; explicit owner tile/tenant |
| **Accel HAL** | `probe / submit / poll / map`; virtqueue MMIO + SoftNPU; IOMMU pin table; `(place, local)` map refuses silent remote load |
| **Typed spaces** | `HOST \| DEVICE_HBM \| TILE_SRAM \| CXL_REGION \| SCRATCH \| STREAMING`; UNIFIED is a cap bit |
| **Activity / partition / fence** | Uniform endpoint; spatial slice + QoS + blast radius; submit → fence → complete |
| **Caps** | Unforgeable `CPtr` slots; monotonic derive; cross-tenant mint rejected |
| **Observability** | COM1 console + structured `EventRing` |

## Repository layout

```
boot/x86_64/     multiboot1 trampoline (32-bit → long mode) + linker scripts
boot/riscv64/    OpenSBI S-mode trampoline + Sv39 linker script
core/            aether-core — alloc-free logic, `cargo test`
hal/             AccelDevice / Console / Timer traits
drivers/         VirtIO-Accel queue + SoftNPU backend
kernel/          freestanding kernel (x86_64 ring-3 + riscv64 thin port)
user/init/       ring-3 `/init` (static ELF64, embedded into the kernel)
docs/            architecture, fabric, accel, security, diligence, roadmap
```

The kernel and `/init` are **separate Cargo projects** so
`cargo test --workspace` stays on the host. `make qemu` builds both for
`x86_64-unknown-none` and embeds the init ELF.

## What v0.1 is honest about

- **Research prototype.** No hardware SMMU, no verified cap derivation tree, no real
  silicon driver, no SMP.
- **VirtIO-Accel is an in-kernel MMIO virtqueue**, not a tree in upstream QEMU.
  `submit` kicks a doorbell; SoftNPU services the queue on the used-ring IRQ
  path so the demo does not depend on a custom qemu. Identity IOVA only.
- **`/init` is a static non-PIE ELF64** linked at `0x0200_0000` and **embedded
  as a kernel blob** (`build/init.elf`). There is no ramfs or virtio-blk yet.
  Ring-3 entry is `syscall`/`sysret`; cap checks sit on send/recv/map/accel.
- **Identity map, UP only.** User gets one USER 2 MiB page; a second core
  does not. Preemption is PIT + a kernel companion thread.
- **RISC-V is a thin port.** `make qemu-riscv` reaches kmain and the
  fabric self-check. No ring-3, no PLIC virtio. aarch64 is not started.

See [docs/ROADMAP.md](docs/ROADMAP.md) for the path toward something a silicon
team could take into bring-up.

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — thesis, boot, modules
- [docs/ABI.md](docs/ABI.md) — PJRT/IREE-shaped host objects; no in-kernel graph IR
- [docs/FABRIC.md](docs/FABRIC.md) — messages, endpoints, route tags, Hodge class
- [docs/CUT.md](docs/CUT.md) — SpectralCut + AffinityLaplacian + Hodge
- [docs/ACCEL.md](docs/ACCEL.md) — HAL, virtqueue MMIO, map API, bank color, how to plug a real NPU
- [docs/SECURITY.md](docs/SECURITY.md) — cap invariants, tenant isolation
- [docs/DILIGENCE.md](docs/DILIGENCE.md) — what ships, stubs, partner pitch
- [docs/DEEP_DIVE_AGENDA.md](docs/DEEP_DIVE_AGENDA.md) — 60–90 min silicon agenda
- [docs/ROADMAP.md](docs/ROADMAP.md) — Month 1–6 status, stubs, next cuts

## Website

The public site lives in [`site/`](site/) (HTML/CSS/JS, no build step) and
deploys from `.github/workflows/pages.yml` on pushes to `main` that touch
`site/`. After the first successful run, enable **Settings → Pages → Source:
GitHub Actions** if it is not already on. The live URL is
[https://amineux.github.io/aether/](https://amineux.github.io/aether/).

Open `site/index.html` locally, or `python3 -m http.server -d site`, to
review offline.

## License

MIT OR Apache-2.0.
