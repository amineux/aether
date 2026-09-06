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
    tables too. Unrelated caps stay. This is a small derivation tree,
    not a seL4 CNode/MDB and not a proof.

This is a research-prototype capability machine (Helios / M3 / Barrelfish /
Twizzler-shaped names, seL4-inspired CPtrs). It does **not** claim
seL4-level proofs.

Host tests in `core/src/caps.rs` and `core/src/demo.rs` lock these down.

## What we do *not* yet enforce

These are marked so a security review does not assume them:

| Gap | Risk | Roadmap |
| --- | --- | --- |
| Init is kernel-mode | A buggy demo can touch any PA | Ring-3 + user page tables — **landed**: `/init` is ring-3; send/recv/map/accel `require()` the CPtr. Kernel `run_boot_demo` is still a trusted self-check. |
| Send path in the kernel demo does not re-walk the sender CPtr on every fabric.send | A kernel-internal caller could pass a raw EndpointId | `SYS_SEND` is the user send path and always `require`s WRITE |
| No hardware SMMU | A real device DMA can ignore Soft SMMU | Soft SMMU tracks chiplet SIDs (STE/CD), aborts until Bound, allocates non-identity IOVA, and refuses maps/binds without Memory+MAP; hardware SMMU is still open |
| Revoke is not a user syscall | Ring-3 cannot name revoke; kernel World still has one shared `CapTable` (PR #10) | Internal `CapTable::revoke` / `revoke_in`; per-task tables still open |
| `revoke` is not a global CNode walk | A GRANT-child in a table the caller did not pass to `revoke_in` survives | Explicit named-table walk; not a seL4 MDB |
| Identity map / no higher-half | Kernel CR3 still names every PA; user isolation is USER-local 2 MiB windows | Higher-half + KPTI |
| No crypto / measured boot | Out of scope for v0.1 | — |

## Multi-tenant weights / KV

The intended story:

- Tenant A's weights live in an arena minted to A.
- The NPU queue receives a **derived** Memory cap (READ, maybe not GRANT).
- Tenant B never receives a cap to that object. Knowing the physical
  address (if it leaked) is not enough once a hardware IOMMU is present.
  v0.1 has Soft SMMU (software stream-ID IOVA map). That is **policy
  complete and software-mechanism present**; a real device can still
  ignore it.

Ring-3 is live; each user task has its own PML4 with USER only on its
2 MiB ELF window (the other user window is unmapped). CR4.SMEP/SMAP
are on. The map API refuses a pin without a Memory cap, allocates a
non-identity IOVA per stream, and refuses wrong-stream / cross-tenant
unmap. Treat isolation as “the cap tables + Soft SMMU + task-local
USER leaves do the right thing” — which is the part we can unit-test
and boot-test today — not “the hardware cannot cheat.” The kernel
trampoline is still an identity map: a forged kernel pointer is a
physical address.

## Covert channels

Scheduler timing, DRAM bank contention, and NPU occupancy are classic
side channels. v0.1 does not mitigate them. A production AI-chip OS would
need cache coloring / bank partitioning and probably a deterministic
wave scheduler for high-assurance tenants.
