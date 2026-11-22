# Security model (research prototype)

Aether's isolation story is **capabilities + tenants**, not POSIX users.
This is the invariant we would defend in a design review — and the places
v0.1 is still incomplete.

## What a task can name

A task holds a `CapTable` of 32 slots. A `CPtr` is an index into *that*
table. Slot `7` in tenant A's table is unrelated to slot `7` in tenant B's
table. There is no global “handle namespace” to guess.

The kernel decodes the complete 64-bit syscall capability argument before
looking up the 16-bit slot. `SYS_SEND`, `SYS_RECV`, `SYS_MAP`, `SYS_ACCEL_SUBMIT`
and `SYS_ACCEL_WAIT` reject nonzero upper bits with `NoCap`; a value such as
`0x1_0001` cannot alias slot `1`. Width validation is followed by the existing
slot, kind and rights checks. This closes an input-validation defect, not a
claim that the alias granted a capability the caller did not already hold.
`SYS_ARENA_ALLOC` likewise rejects bank IDs larger than 255 with `Inval`, before
allocation, rather than wrapping them onto a lower bank. In-range IDs still go
through the allocator's existing bank selection rules.

Host regression tests cover high bits and valid handles. The userspace `/init`
smoke tries oversized arguments against live slots through all five syscalls
and checks the existing error channel, then continues through normal map/submit
operations. Every QEMU CI variant requires `[init] wide syscall arguments refused`.
Syscall numbers, structures and `IreeHalCmd` remain unchanged.

Each `Capability` stores:

- `kind` — Memory, Endpoint, AccelQueue, Notification, SpectralCut,
  FlowQuota, Activity, Partition, OperatorKernel
- `rights` — subset of READ/WRITE/GRANT/MAP/SUBMIT/WAIT/EXECUTE/BIND/UNIFIED
  (`UNIFIED` is never in `MEM_FULL`)
- `object` — kernel object id
- `generation` — bumped at mint; with `tenant` this is the derivation node
- `tenant` — must match the table owner at lookup
- `parent` — derivation edge (set on derive / GRANT-copy)

Production tables share a process/boot-wide atomic mint-generation allocator.
Generations run from 1 through `u32::MAX` without reuse across tables, including
separate tables for the same tenant. This keeps `(tenant, generation)` derivation
IDs distinct and preserves legitimate same-tenant KV grants. `CapTable::new`
remains const and does not reset the shared allocator. Relaxed atomics allocate
unique numbers; callers still synchronize publication and mutation of tables.

After the final generation, mint returns `GenerationMismatch` before changing
slots; derive and GRANT-copy/move propagate that refusal. Revocation frees slots
but cannot reset exhaustion. The limit is now shared by all production tables
for the process/boot lifetime, not a separate quota per table. Existing caps and
revocation remain usable. Raw CPtrs still name recyclable slots.

Cloning a table preserves its existing capabilities and their identities, so a
clone is an authority snapshot, not a new isolation domain. Future mints in the
original and clone use the shared allocator and cannot collide. Named-table
revocation still reaches only the explicitly supplied tables; this change adds
neither a global CNode registry nor a user-facing per-process cap-table syscall.

## IPC receive completion

Both `SYS_SEND` and the kernel fabric ping complete a blocked receive from the
checked endpoint FIFO, not a separate copy of the sender's input. Completion
dequeues exactly one message and copies its stored badge, flags and payload.
This keeps the queued and blocked paths consistent, including the endpoint-cap
badge fallback when the caller supplied zero, and releases the queue slot.

Only the waiter whose saved output buffer was handled becomes Ready. Other
receivers on the endpoint remain blocked until their own completion. An empty
or unauthorized dequeue does not touch the saved buffer or wake the waiter.
The current BSP-only userspace scheduler serializes selection, dequeue, copy,
saved errno and targeted wake under the interrupt-disabling World lock.

A failed copy returns the negative syscall error to that receiver instead of
success. The message is consumed, matching the immediate receive path; there is
no retry/requeue guarantee for an invalid receive buffer. Host regressions cover
FIFO/header fidelity, capacity reuse, ownership, copy failure and targeted wake.
All seven QEMU gates require the cloned-thread protocol to receive two distinct
messages around a known-window/unmapped-buffer copy fault, plus an actual kernel
blocked-delivery marker. This does not change the IPC ABI. The current boot
capability spaces are described below.

## Accelerator wait completion

Immediate and blocked `SYS_ACCEL_WAIT` paths use the same one-slot completion
consumer. A result is taken before copy and cannot satisfy another wait. An empty
slot does not call the output function, clear a saved buffer or wake a waiter.
For blocked waits, publishing the result, taking it, writing the selected output,
saving the syscall result and targeted wake run under the World lock with
interrupts disabled on the current BSP-only userspace scheduler. Other accelerator
and IPC waiters remain blocked until their own completion.

Copy faults return the actual negative syscall errno to the selected waiter,
rather than success. The completion is consumed on copy failure, matching the
immediate path. Device status/cycle fields remain completion data, not copy errno.
Cloned-thread boot checks cover valid/fault/valid waits submitted one at a time,
reject stale early returns and require increasing job sequences for valid results;
the original parent-side matmul wait then receives a later completion. All seven
QEMU gates require both the user protocol and an actual blocked-completion marker.

The single-result pipeline admits a new `SYS_ACCEL_SUBMIT` only when no job is
pending and no unread completion occupies the cache. After existing capability,
shape, address and operation validation, a busy pipeline returns `Again` before
allocating a timeline fence or calling the device. Admission and device submission
run under the World lock, preventing a competing caller or IRQ from crossing the
capacity check. A successful submit marks the job pending; a failed device submit
keeps the slot idle and retains the existing fence-timeout cleanup.

A completed but unread result is never cleared by a new submit. Consuming it,
including a copy-fault consumption, releases admission capacity. The boot regression
leaves a result unread, requires six valid submissions to return `Again`, checks
that the original result survives with the next device sequence, and retries
successfully after consumption. All seven QEMU gates require this result and a
kernel cache marker; host tests additionally cover the in-flight guard and failed
submission retry. The cache still holds one completion rather than a general
multi-job result queue. These are software checks, not new hardware isolation.

## Invariants (implemented and tested)

1. **No cross-tenant mint.** `CapTable::mint` rejects a cap whose tenant
   field is not the table owner.
2. **Monotonic derive.** Rights may only shrink. `WouldEscalate` otherwise.
3. **GRANT required to transfer.** No silent aliasing of a cap to another
   table without GRANT.
4. **Kind + rights checked on use.** `require(cptr, AccelQueue, SUBMIT)`
   fails for a Memory cap or a read-only queue cap.
5. **Isolation demo.** Tenant B does not `holds(Memory, A's arena)` and
   cannot `require` A's `CPtr` (the slot is empty in B's table).
6. **Cut bind.** `SpectralCut` requires `BIND`. Tenant B does not hold
   A's cut. Cross-cut placements are `CutError::CrossCut`.
7. **Hodge class.** `FlowQuota` badge is a class mask. Harmonic +
   `TREE_OFFLOAD` is refused even if the tenant is authorized (`deadlock`).
   An `OperatorKernel` cap binds one topology to one class; Tree+Harmonic
   / Tree+Curl refuse at bind, and inject of a different class is
   `ClassMismatch`. `SparsifiedCollective` may drop below-threshold
   harmonic before enqueue; it does not bypass Harmonic+TREE refuse.
8. **Activity + partition.** A virt accel is an `Activity` cap, not an
   ioctl. Jobs bind a `PartitionProfile` (spatial slice, credits, blast
   radius). Isolation is spatial (slices/columns) first, temporal second—QoS and blast radius are invariants.
9. **Typed spaces.** A Memory cap does not imply a unified VAS.
   `CapRights::UNIFIED` must be granted explicitly. `TypedWindow` is a
   Soft-SMMU pin stub (Exploration E); foreign-tenant windows are
   `CrossCut`. Not a CXL.mem decoder.
10. **Revoke descendants.** `derive` and GRANT-copy record a parent edge.
    `revoke(parent)` empties that lineage in the same table;
    `revoke_in(parent, others)` empties grant-children in the named
    tables too. A lineage larger than the 64-node walk bound returns
    `RevokeTooLarge` before any slot changes. Unrelated caps stay.
    See [bounded revocation](CAP_REVOCATION.md) for the named-table limits.

This is a research-prototype capability machine (Helios / M3 / Barrelfish /
Twizzler-shaped names, seL4-inspired CPtrs). It does **not** claim
seL4-level proofs. Formal caps are not a calendar item.

Host tests in `core/src/caps.rs`, `core/src/caps_props.rs`,
`core/src/demo.rs`, and `core/src/blast.rs` lock the statements below.
The blast-radius diligence clip serial-prints `[blast]` after CrossCut
and wrong-SID refuse. The SID-at-submit clip serial-prints `[sid]` after
two-tenant SET_SID, refuse-until-armed, and per-tenant SID budget
(`core/src/sid.rs`). Host1x is inspiration only — not a Tegra driver
and not hardware-grade isolation. The software SID budget (4 Bound CDs
per tenant) plus the `inject_wrong_sid` / `SubmitSid` host tests are
the isolation we can unit-test today; they are not a silicon SID
allocator or a Host1x fault-injection campaign.

SoftCmdFirewall (`drivers/src/firewall.rs`) copies the Soft-CP
command image into a kernel-owned arena **before** opcode / reloc /
SID / addr-cap validate, then enqueues the copy. That is Host1x's
copy-then-validate lesson: a client that mutates the buffer in the
validate window cannot sneak a rewritten stream. It is **integrity
of the command stream only** — not confidential GPU, not HBM
encryption, not GPU-CC HMAC (optional later), and not NVIDIA SEC2.
Serial: `[firewall]`.

SoftChipletSync (`core/src/chipsync.rs`) is a software fence-domain
model (scoped timelines + SoftCCT last-writer elision). It is **not**
a coherence protocol, not UCIe, not a Vulkan / ROCm product, and not
hardware-grade isolation across chiplets.

SoftGreenCtx (`core/src/greenctx.rs`) is a software SM/WQ partition
on Soft-CP. CUDA Green Contexts / DetShare are inspiration only. It
is **not** HW MIG, not a BAR firewall, and not silicon SM isolation.
Host tests measure memcpy-like interference; they are not FLOPs.

SoftSFI (`core/src/softsfi.rs`) is a toy Soft-CP bytecode sandbox
(GPU-AToLL-shaped SFI). Every modeled load/store/dma/`atomic_add`
proves `base+bound` in the SID-allowed range. `atomic_add` is a
sequential toy RMW, not a coherent hardware atomic. It is **not** an
NVVM pipeline and **not** “safe multi-tenant kernels.” Tensor copies
are **not** in the modeled ISA. Heap/alloc is a **named** opcode
that is refused (`SfiError::Unmodeled`) — not a bump allocator.
Skip-verify fault injection still traps on the SID window and does
not cross-read. Software only. Sell clip: `[softsfi] heap=refused`.

PASID / SVA (`core/src/sva.rs`, `IommuMap::bind_mm` / `map_va` /
`unmap_va`) binds a process mm to a Soft-SMMU SSID so Soft-CP DMA
uses that process VA. Host unmap invalidates the SSID ATC. Unmap is
tenant-checked (Memory+MAP cap; another tenant's pin is
`MapError::CrossTenant` and stays mapped; issue #161). Skipping
invalidate is a stale translate. Linux SVA / PASID inspiration. It
is **not** ARM SVA, **not** PCIe PASID/PRI, **not** CUDA UVA, and
**not** zero-copy SVA without invalidate. No new syscall.

OperatorInject (`core/src/opinject.rs`) is a Soft-CP resident worker
plus a versioned function table (`memcpy` / `saxpy`, hot-add `scale`).
GPUOS / Mirage MPK are inspiration only. It is **not** NVRTC, CUDA, a
vendor compiler, or a full LLM compiler. SID-at-submit and
SoftCmdFirewall copy-then-validate still gate every call. Serial:
`[opinject]`.

The host sell-path `make red-team` (`examples/red-team`) prints one
line per named attack (`[redteam] attack=… result=refused`) by calling
those same clips. It does not add isolation. Closer: not confidential
GPU, not HW MIG, Soft SMMU is software.

## CDT properties (host tests, not a proof)

Aether stores a parent pointer (`CdtNode` = table owner + mint
generation). The tests in `core/src/caps_props.rs` are property /
exhaustive cases on that pointer. They are **not** a proof, **not** a
syscall, and **not** a reason to schedule a formal cap kernel.

| Id | Statement |
| --- | --- |
| **P-Revoke** | mint → derive (optional GRANT-copy) → `revoke_in(root, named tables)` empties descendants in those tables |
| **P-Unrelated** | a cap outside that lineage stays live |
| **P-Named** | GRANT-copy across tables is collected only if the dest table is passed to `revoke_in`; `revoke` alone leaves the foreign child live |
| **P-Unforge** | `mint` rejects a foreign tenant; a `CPtr` is a slot in *one* table |
| **P-Monotone** | derive / GRANT may only shrink rights; GRANT is required |

`revoke_in` walks tables the caller names. A GRANT-child in a table
that was not passed survives — that is `P-Named`, not a missing global
walk. No `SYS_REVOKE`. Syscall numbers 0–8 stay frozen.

## What we do *not* yet enforce

These are marked so a security review does not assume them:

| Gap | Risk | Roadmap |
| --- | --- | --- |
| Init is kernel-mode | A buggy demo can touch any PA | Ring-3 / U-mode / EL0 + user page tables — **landed** on x86, RISC-V, and aarch64: `/init` is user; send/recv/map/accel `require()` the CPtr. Kernel `run_boot_demo` is still a trusted self-check. |
| Send path in the kernel demo does not re-walk the sender CPtr on every fabric.send | A kernel-internal caller could pass a raw EndpointId | `SYS_SEND` is the user send path and always `require`s WRITE |
| No hardware SMMU | A real device DMA can ignore Soft SMMU | Soft SMMU walks STE→CD→S1/S2, hardens SSID/CD, aborts until Bound, ATS-invalidates a software ATC, allocates non-identity IOVA, refuses maps/binds without Memory+MAP, and (Soft-CP / IreeShapedCp) refuses DMA until SET_SID is armed for that submit; `TypedWindow` is a software pin stub; hardware SMMU still needs partner silicon |
| Revoke is not a user syscall | Ring-3 cannot name revoke; kernel World has one bootstrap `CapTable`, restricted to the boot-designated /init root | Internal `CapTable::revoke` / `revoke_in`; per-task tables still open |
| `revoke` is not a global CNode walk | A GRANT-child in a table the caller did not pass to `revoke_in` survives | Explicit named-table walk; not a seL4 MDB |
| Identity islands on kernel CR3 | Bulk 4 GiB identity is unmapped. Remaining supervisor islands: low 2 MiB (SIPI / mailbox / trampoline), virtio-blk window, APIC MMIO. SoftNPU is Soft SMMU + HH. User CR3 has no identity (KPTI subset). `USER_MMAP_BASE` is user-only, not an identity island | Meltdown-complete trampoline unmap; POSIX MM |
| COW is one 4 KiB page | `/init` + `/probe` share one RO template until a write fault; `SYS_CLONE` shares the broken page | `fork`-shaped aspace clone |
| `SYS_MMAP` is a 64 KiB anon window | First-fit 4 KiB USER pages at `0x02C0_0000` (after virtio-blk); no file / no `MAP_SHARED` / no `munmap` | POSIX `mmap` / file-backed / `MAP_SHARED` |
| `SYS_UNMAP` names only tracked `SYS_MAP` pins | Number 5 and `(vaddr, len)` are unchanged. World retains exact IOVA starts and Memory+MAP authority in a bounded `MAX_MAPS` table; each live pin can be removed independently. Boot pins are separate. `len` is ignored and `SYS_MMAP` pages are not individually unmapped | Per-task authority/revocation and finer CPU-page unmapping remain open |
| SoftSFI tensor / heap | A kernel that used those ops would bypass the toy sandbox | Named refuse: `SfiError::Unmodeled` (`SoftOp::Tensor`, `SoftOp::Heap`). `atomic_add` is SID-proved (in-range accept, cross-tenant `Oob`) but is a sequential toy RMW, not full SFI. Heap is refused, not a modeled bump allocator. Not a GPU-AToLL port. Hardware SMMU / real SFI still need partner silicon |
| No crypto / measured boot | Out of scope for v0.1 | — |

## Multi-tenant weights / KV

The intended story:

- Tenant A's weights live in an arena minted to A.
- The NPU queue receives a **derived** Memory cap (READ, maybe not GRANT).
- Tenant B never receives a cap to that object. Knowing the physical
  address (if it leaked) is not enough once a hardware IOMMU is present.
  v0.1 has Soft SMMU (software STE→CD→S1/S2 walk + ATS invalidate).
  That is **policy complete and software-mechanism present**; a real
  device can still ignore it. Hardware SMMU needs partner silicon.

Ring-3 is live; each user *task* has its own PML4 with USER only on its
2 MiB ELF window (the other user window is unmapped). `SYS_CLONE`
threads share that PML4 — they are not a second isolation domain.
CR4.SMEP/SMAP are on. The map API refuses a pin without a Memory cap, allocates a
non-identity IOVA per stream, and refuses wrong-stream / cross-tenant
unmap. `SYS_UNMAP` (number 5, same arguments) returns that result:
`-NoCap` for a root without a registered capability table or when World has no tracked syscall pins, `-Fault` for an unknown or
interior address, or a pin owned by a different address-space root, or a refused/missing device
pin, and `0` when the caller's named tracked pin was dropped. Authority is removed only after device unmap succeeds; refusal
preserves the other entries. Map reserves tracking capacity before pinning, and
a failed map leaves that capacity available for retry. The table has `MAX_MAPS`
slots; the software IOMMU's own limit includes boot pins and may be reached first.
Map/unmap and tracking updates run under the World lock. `len` is ignored and CPU
user mappings are retained: this is not `munmap` of `SYS_MMAP` pages. The stored
capability snapshots remain part of the single shared kernel World. Each syscall
pin is tagged with the trusted caller's address-space root at map time and can be
unmapped only from that root. SYS_CLONE siblings share the root and pin authority;
separate roots cannot consume each other's recorded unmap authority even within
the same tenant.

### Boot capability spaces

World stores separate capability tables for the two current boot images. Trusted
boot code binds /init's loaded root once; on x86 it also binds /probe's loaded
root to a fresh table containing just a private Endpoint READ|WRITE cap (no
GRANT). Both images remain in tenant 1. CPtr 0 names different endpoints in the
two tables; a CPtr is never a global object id. All capability syscalls select
the table using the scheduler's trusted current root. Zero, unbound and unknown
roots have no bootstrap fallback and return NoCap before touching user buffers.
Blocked IPC delivery uses the saved waiter's root to select its table owner.

/probe has no queue or memory cap, and cannot inherit /init's caps by guessing
slots. SYS_ARENA_ALLOC remains an explicit init-only boot policy until an
allocator-cap ABI exists. SYS_UNMAP uses the registered root and recorded per-pin
authority; a foreign/missing pin returns Fault when pins exist, NoCap when the
set is empty. SYS_CLONE siblings share their root's actual table, not snapshots.
Kernel-internal ping sends remain trusted through the bootstrap table.

Host tests exercise table selection, same-tenant slot separation, grant limits,
shared-root visibility and refused rebinding. The five x86 QEMU gates require
/probe's private endpoint round-trip (including badge and payload), plus refusal
of absent endpoint, queue, memory and allocator authority and invalid unmap.
All three architecture demos still exercise /init and cloned-thread IPC, DMA
unmap and accelerator completion. This is a bounded two-image boot registry,
not a dynamic cap-space or user grant syscall, separate-tenant scheduling,
cap-revocation synchronization or hardware isolation. Roots cannot be rebound.
SYS_EXIT currently shuts down the guest, not one task. Root-reuse lifetime cleanup
must accompany a future task-reaping implementation before roots can be recycled.
Treat isolation as “the cap tables + Soft SMMU + task-local
USER leaves do the right thing” — which is the part we can unit-test
and boot-test today — not “the hardware cannot cheat.” The kernel
is linked at `0xffffffff80400000` and may run at a 16/32 MiB slide
after PIE `.rela.dyn` apply; the unused HH alias is unmapped. The
trampoline identity 4 GiB is torn down on the **kernel** CR3 except
SIPI / mailbox / trampoline / virtio-blk / APIC islands. SoftNPU
DMA is Soft SMMU + HH (`KernelDma`).
User CR3 maps only the
ELF window plus a 4 KiB supervisor trampoline — a forged low kernel
pointer is not present there. That is a KPTI subset, not
Meltdown-complete (trampoline pages remain mapped). PCID tags
KPTI `mov cr3` when CPUID advertises it so the switch is not a
full TLB flush; stock `qemu64` often falls back to a full flush.
PCID is not a speculation barrier. A documented **COW subset**
maps one shared 4 KiB USER page (`0x0280_0000`) read-only in
`/init` and `/probe`; a write fault copies the frame on that
aspace only. That is not `fork` and not POSIX `mmap`. RISC-V /
aarch64 do not map it. SoftNPU DMA still uses kernel CR3.

## Covert channels

Scheduler timing, DRAM bank contention, and NPU occupancy are classic
side channels. v0.1 does not mitigate them. A production AI-chip OS would
need cache coloring / bank partitioning and probably a deterministic
wave scheduler for high-assurance tenants.

### AArch64 user trap register preservation

EL0 return restores IP0/IP1 (`x16`/`x17`) only after selecting the destination
kernel stack. Reusing `x16` for that selection after restoring user registers
would corrupt a live EL0 value across SVC or timer preemption. The AArch64 QEMU
gate requires sentinel values to survive repeated `SYS_YIELD` calls, alongside
the cloned-thread DMA unmap check. This is a register-preservation regression,
not a complete scheduler or trap-handler proof.
