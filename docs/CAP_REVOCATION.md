# Bounded capability revocation

`CapTable::revoke_in` revokes one capability and every descendant found in
the explicitly named capability tables. Aether does not maintain a hidden
kernel-global table registry, so callers must name every table whose derived
capabilities should be included.

The no-std walk uses a fixed kill set of `CAP_SLOTS * 2` nodes. This exactly
covers two full tables, including the root capability. A larger lineage is
refused as `CapError::RevokeTooLarge`.

Revocation is transactional with respect to this bound:

1. Look up the root without removing it.
2. Build the complete transitive kill set across all named tables.
3. If the set overflows, return `RevokeTooLarge` with every slot unchanged.
4. Only after the walk reaches a fixed point, empty all recorded slots.

This prevents a bounded-walk overflow from reporting success after deleting
only part of a derivation tree. Tests exercise the exact 64-node boundary and
a 96-node, three-table lineage. The oversized case verifies that the root,
intermediate grants, destination descendants, and occupancy of all three
tables remain unchanged after refusal.

The 65-node boundary regression visits the grandchild table before the
parent table, so overflow is detected only on a later fixed-point pass.
Three repeated refusals preserve every capability field and each table's
mint counter. Removing the single excess leaf then permits a clean 64-node
retry while preserving 31 unrelated endpoint capabilities in the third table.

This is a bounded software invariant, not a kernel-global CNode broadcast or
formal verification. A caller that omits a table still gets the documented
named-table behavior: descendants in that unnamed table are outside the walk.

Production capability tables share one atomic generation allocator for their
process/boot lifetime. A separate same-tenant table therefore cannot mint a new
cap with the same `(tenant, generation)` as another table. Revoking a root while
naming such a table preserves unrelated caps and removes only the root's actual
descendants. Same-tenant copy/derive handoffs remain supported.

A cloned table preserves identities of existing capabilities. Naming both
snapshots in a revoke can remove that shared authority in both; future mints in
either table get fresh generations. Host regressions cover independent tables,
same-tenant grants with grandchildren, clones and concurrent generation issuance.
The shared allocator refuses after `u32::MAX`; new tables cannot restart it.
These are software lifetime guarantees, not persistent identities across reboot,
a global revoke registry, or a hardware-isolation claim.

## Retaining root authority and kernel endpoint revocation

`revoke_descendants_in` shares the same bounded plan with `revoke_in` but keeps
the selected root identity (including aliases of that identity). It removes only
descendants in the named tables, without consuming mint generations. A walk that
exceeds 64 nodes still refuses before mutation. Exact-bound and 65-node refusal
regressions also cover this operation; the root can grant again into freed slots.
The deleting `revoke`/`revoke_in` APIs retain their original behavior.

SYS_CAP_REVOKE (13) exposes this operation only for Endpoint+GRANT in the current
boot registry and names both tables. World then cancels blocked receives whose
root no longer holds any READ capability on the saved endpoint, returning NoCap
without copying or consuming a queued message. It retains other endpoint
lineages and accelerator waiters. The walk and cancellation share World's
interrupt-disabled lock on the current BSP-only userspace scheduler.

The x86 boot protocol revokes a WRITE grant, reuses its slot for READ, cancels
that blocked read, and grants WRITE into the freed slot again. The peer checks
its untouched output and continued private IPC before sending a distinct message.
This adds neither endpoint destruction nor memory/DMA revocation, general
process lifetime cleanup or a hardware-isolation claim. See [ABI](ABI.md) and
[security boundaries](SECURITY.md) for the syscall and waiter semantics.
