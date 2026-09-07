# Security model (research prototype)

Aether's isolation story is **capabilities + tenants**, not POSIX users.
This is the invariant we would defend in a design review — and the places
v0.1 is still incomplete.

## What a task can name

A task holds a `CapTable` of 32 slots. A `CPtr` is an index into *that*
table. Slot `7` in tenant A's table is unrelated to slot `7` in tenant B's
table. There is no global “handle namespace” to guess.

Each `Capability` stores:

- `kind` — Memory, Endpoint, AccelQueue, Notification, SpectralCut,
  FlowQuota, Activity, Partition, OperatorKernel
- `rights` — subset of READ/WRITE/GRANT/MAP/SUBMIT/WAIT/EXECUTE/BIND/UNIFIED
  (`UNIFIED` is never in `MEM_FULL`)
- `object` — kernel object id
- `generation` — bumped at mint; with `tenant` this is the derivation node
- `tenant` — must match the table owner at lookup
- `parent` — derivation edge (set on derive / GRANT-copy)

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
   `CapRights::UNIFIED` must be granted explicitly.
10. **Revoke descendants.** `derive` and GRANT-copy record a parent edge.
    `revoke(parent)` empties that lineage in the same table;
    `revoke_in(parent, others)` empties grant-children in the named
    tables too. Unrelated caps stay.

This is a research-prototype capability machine (Helios / M3 / Barrelfish /
Twizzler-shaped names, seL4-inspired CPtrs). It does **not** claim
seL4-level proofs. Formal caps are not a calendar item.

Host tests in `core/src/caps.rs`, `core/src/caps_props.rs`,
`core/src/demo.rs`, and `core/src/blast.rs` lock the statements below.
The blast-radius diligence clip serial-prints `[blast]` after CrossCut
and wrong-SID refuse.

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
| No hardware SMMU | A real device DMA can ignore Soft SMMU | Soft SMMU walks STE→CD→S1/S2, hardens SSID/CD, aborts until Bound, ATS-invalidates a software ATC, allocates non-identity IOVA, and refuses maps/binds without Memory+MAP; hardware SMMU still needs partner silicon |
| Revoke is not a user syscall | Ring-3 cannot name revoke; kernel World still has one shared `CapTable` (PR #10) | Internal `CapTable::revoke` / `revoke_in`; per-task tables still open |
| `revoke` is not a global CNode walk | A GRANT-child in a table the caller did not pass to `revoke_in` survives | Explicit named-table walk; not a seL4 MDB |
| Identity islands on kernel CR3 | Bulk 4 GiB identity is unmapped. Remaining supervisor islands: low 2 MiB (SIPI / mailbox / trampoline), virtio-blk window, APIC MMIO. SoftNPU is Soft SMMU + HH. User CR3 has no identity (KPTI subset). `USER_MMAP_BASE` is user-only, not an identity island | Meltdown-complete trampoline unmap; POSIX MM |
| COW is one 4 KiB page | `/init` + `/probe` share one RO template until a write fault; `SYS_CLONE` shares the broken page | `fork`-shaped aspace clone |
| `SYS_MMAP` is a 64 KiB anon window | First-fit 4 KiB USER pages at `0x02C0_0000` (after virtio-blk); no file / no `MAP_SHARED` / no `munmap` | POSIX `mmap` / file-backed / `MAP_SHARED` |
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
unmap. Treat isolation as “the cap tables + Soft SMMU + task-local
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
