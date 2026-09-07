# Deep-dive agenda (silicon OS teams)

A concrete 60–90 minute technical session for an NVIDIA-class or custom
ASIC operating-systems group. **This meeting has not happened.** The
agenda exists so it could, with a demo script and open questions, not
a press quote.

Audience: kernel / firmware / runtime engineers who own the command
processor, SMMU story, and the compiler’s submit path.

Prep (10 minutes, async): clone this repo, skim
[ARCHITECTURE.md](ARCHITECTURE.md) and [DILIGENCE.md](DILIGENCE.md).
No NDA draft, no performance slide.

## Minute-by-minute

| Time | Block | Owner-side goal |
| --- | --- | --- |
| 0:00–0:05 | Frame | Aether is a research prototype. No partnership claim. We want to know if the HAL contract is one they would implement. |
| 0:05–0:20 | Live demo | Serial boot + fabric banner on x86_64, then RISC-V / aarch64. Host tests on the same `run_boot_demo()`. |
| 0:20–0:40 | HAL walkthrough | `AccelDevice`, `AccelJobDesc`, SoftNPU vs a real doorbell. Where their driver would sit. |
| 0:40–0:55 | Caps, spaces, cuts | Tenant isolation, `(place, local)`, SpectralCut + AffinityLaplacian, Hodge refuse. |
| 0:55–1:10 | Compiler boundary | Why there is no in-kernel graph IR. PJRT/IREE-shaped nouns. Who owns fusion. |
| 1:10–1:25 | Open questions | Their command ISA, SMMU, QoS, what we got wrong. |
| 1:25–1:30 | Close | Concrete follow-up: a one-page opcode map, or a “no” with reasons. |

For a 60-minute slot, drop the compiler block to five minutes and keep
the open questions.

## Demo script

Machine: any `x86_64` Linux with rustc 1.83+, `qemu-system-x86_64`,
GNU `as`/`ld`/`objcopy`, `rustup target add x86_64-unknown-none`.
RISC-V also needs `qemu-system-riscv64` and
`rustup target add riscv64gc-unknown-none-elf`. aarch64 needs
`qemu-system-aarch64` and `rustup target add aarch64-unknown-none`.

```bash
# 1. Same invariants on the host (no QEMU).
cargo test --workspace

# 2. x86_64 vertical slice: kernel self-check, then ring-3 /init.
make qemu

# 3. RISC-V S-mode + U-mode /init. OpenSBI chatter first, then ecall.
make qemu-riscv

# 4. aarch64 EL1 + EL0 /init. QEMU virt; svc / eret. Documented subset.
make qemu-aarch64
```

What to point at on the serial:

1. Trampoline line (`multiboot1`, `riscv64, OpenSBI S-mode`, or
   `aarch64, QEMU virt EL1`).
2. `[fabric] IPC ok  arena ok … isolation ok`.
3. `[cut] bind SpectralCut … CrossCut refuse`.
4. `[hodge] … harmonic-tree REFUSE ok`.
5. `[map] Soft SMMU pin + Memory-cap refuse ok`.
5b. `[window] TypedWindow CxlMemStub … (stub; not CXL.mem)`.
6. `[color] tenant/bank paint  Compute foreign refuse + Exchange ok`.
7. `[laplace] L=D-A n=6 … chiplet-split=ok`.
8. x86: `[mm] SMEP+SMAP` + `[mm] aspace isolate ok`, then
   `[init] ring-3 /init` and `RING-3 /init VIA SYSCALL/SYSRET`.
   RISC-V: `[mm] aspace isolate ok`, `[init] U-mode /init`,
   `U-MODE /init VIA ECALL/SRET`.
   aarch64: `[mm] aspace isolate ok`, `[init] EL0 /init`,
   `EL0 /init VIA SVC/ERET`.
9. `FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE`.

If QEMU is blocked, `cargo test -p aether-core laplacian -- --nocapture`
still shows `L = D − A` and the Fiedler-ish chiplet split.

Do **not** show a benchmark. There isn’t one.

## HAL walkthrough (whiteboard)

```text
compiler / runtime          Aether                     tile
IREE / PJRT / vendor   ┌─ AccelJobDesc ─┐         cmd processor
  Executable handle ──►│ partition+fence │──submit──► doorbell
  Buffer (place,local) │ SpectralCut     │            SMMU SID
                       │ Memory cap      │──map()────►
                       └─────────────────┘
```

Files, in this order:

1. `hal/src/lib.rs` — `AccelDevice` trait. Four methods.
2. `core/src/accel.rs` — `AccelJobDesc`. Opcode, shape, dtype, place,
   phase, partition, fence. Not a graph.
3. `docs/ACCEL.md` — how a command processor plugs in (`CpCmd` and `IreeHalCmd`).
4. `drivers/src/softnpu.rs` + `drivers/src/mmio.rs` — virtqueue BAR + SoftNPU.
5. `drivers/src/fakecp.rs` — software CP: packed Aether-native packet + Soft SMMU + IRQ/fence.
6. `drivers/src/ireecp.rs` — partner-shaped HAL spine: IREE HAL dispatch packet (`backend = 4`; not a vendor).
7. `host/aether-pjrt` + `docs/HOST.md` — compiler-facing nouns; freeze packs `IreeHalCmd` → IreeShapedCp
   (SoftNPU stays `make qemu`; not a PJRT plugin / IREE driver / partnership).
8. `core/src/iommu.rs` + `core/src/color.rs` — map refuse + bank color + SID.
9. `drivers/src/partner.rs` — leftover no-op sketch (not a partner, not this path).
10. `boot/riscv64/trampoline.S` + `kernel/src/arch/riscv64/` and
   `boot/aarch64/trampoline.S` + `kernel/src/arch/aarch64/` — evidence
   the HAL split is real: new UART/timer/page tables, same core.

Questions to ask *them* while the board is up:

- Which fields in `AccelJobDesc` already exist on the command packet?
- What is missing (stream ID, barrier group, scratch color)?
- Do they already refuse a coherent load across a chiplet? If yes, our
  `map_fabric` refuse should match their hardware.

## Open questions (bring these)

**Command processor**

- Opcode / dtype / stride surface: closest existing packet?
- Is a wave a hardware object or a compiler fiction?
- Completion: IRQ, doorbell poll, or both?

**Memory and isolation**

- SMMU / IOMMU topology: one SID per tenant, per queue, per context?
- Is HBM bank coloring a hardware feature they want the OS to name?
- Unified virtual address: do they need the `UNIFIED` cap bit, or is
  it a host-runtime myth they are trying to kill?

**Package graph**

- What is the real affinity graph (on-die / EMIB / UALink / NVLink)?
- Do they want a cut capability, or only a placement hint?
- Is Fiedler / conductance the language their architects already use?

**Runtime boundary**

- Who submits today — CUDA/HIP runtime, IREE HAL, a proprietary
  userspace? What would they delete if the kernel admitted jobs?
- Do they need a timeline that is *not* a CUDA stream (credits,
  timeout, blast radius)?

**What would make this a waste of their time**

- Ask them to say it. Record it. Do not paper over it in a follow-up.

## What we will not say in the room

- That we have a design win, a joint roadmap, or a shared customer.
- That RISC-V U-mode `/init` or aarch64 EL0 `/init` is a
  product-class second kernel.
- That SoftNPU predicts their silicon latency.
- That seL4 proofs are “in progress.”

If the session happens, the output is an opcode-map note or a written
pass. Not a logo.
