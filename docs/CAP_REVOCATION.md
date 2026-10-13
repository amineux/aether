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

This is a bounded software invariant, not a kernel-global CNode broadcast or
formal verification. A caller that omits a table still gets the documented
named-table behavior: descendants in that unnamed table are outside the walk.
