# Security model (research prototype)

Aether's isolation story is **capabilities + tenants**, not POSIX users.
This is the invariant we would defend in a design review — and the places
v0.1 is still incomplete.

## What a task can name

A task holds a `CapTable` of 32 slots. A `CPtr` is an index into *that*
table. Slot `7` in tenant A's table is unrelated to slot `7` in tenant B's
table. There is no global “handle namespace” to guess.

Each `Capability` stores:

- `kind` — Memory, Endpoint, AccelQueue, Notification
- `rights` — subset of READ/WRITE/GRANT/MAP/SUBMIT/WAIT/EXECUTE
- `object` — kernel object id
- `generation` — bumped at mint; revoke empties the slot
- `tenant` — must match the table owner at lookup

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

Host tests in `core/src/caps.rs` and `core/src/demo.rs` lock these down.

## What we do *not* yet enforce

These are marked so a security review does not assume them:

| Gap | Risk | Roadmap |
| --- | --- | --- |
| Init is kernel-mode | A buggy demo can touch any PA | Ring-3 + user page tables |
| Send path in the kernel demo does not re-walk the sender CPtr on every fabric.send | A kernel-internal caller could pass a raw EndpointId | Wire `syscall::SYS_SEND` as the only send |
| No IOMMU | A real device DMA can ignore caps | `map()` must program SMMU |
| No revocation broadcast | A derived cap in another table survives revoke of the parent | seL4-style CNode / CDT |
| Identity map | Kernel and “user” share one address space | Per-task PML4 |
| No crypto / measured boot | Out of scope for v0.1 | — |

## Multi-tenant weights / KV

The intended story:

- Tenant A's weights live in an arena minted to A.
- The NPU queue receives a **derived** Memory cap (READ, maybe not GRANT).
- Tenant B never receives a cap to that object. Knowing the physical
  address (if it leaked) is not enough once an IOMMU is present; v0.1
  still identity-maps, so this is **policy complete, mechanism incomplete**.

Until IOMMU + ring-3 land, treat isolation as “the cap tables do the right
thing” — which is the part we can unit-test today — not “the hardware
cannot cheat.”

## Covert channels

Scheduler timing, DRAM bank contention, and NPU occupancy are classic
side channels. v0.1 does not mitigate them. A production AI-chip OS would
need cache coloring / bank partitioning and probably a deterministic
wave scheduler for high-assurance tenants.
