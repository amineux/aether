# Arena admission invariants

Arena initialization now requires one nonempty physical window per bank ID.
Windows must have a representable half-open end and must not overlap, even
across different banks. Adjacent and unsorted disjoint windows remain valid.
Duplicate IDs return `ArenaError::UnknownBank`; zero, overflowing and overlapping
windows return `ArenaError::BadSize`. This prevents double-allocation of the same
physical memory and later arithmetic overflow from malformed configuration.
It does not claim these trusted boot configuration values were tenant-controlled.

`ArenaAllocator::alloc_with_cap` creates an arena and its Memory capability
together. It stages a bounded allocator copy, attempts the allocation and cap
mint, and publishes the allocator only after mint succeeds. Cap mint itself
has no failure side effects. Failure preserves free spans, arenas, object IDs
and capability-table contents/generation. An uncolored request takes the cap
table owner's tenant; explicitly foreign colors are refused.

This is a kernel/trusted-host creation API. The caller must already control
the allocator. It grants `MEM_FULL` on the new object; it is not a way for an
untrusted caller to manufacture authority. Exclusive mutable borrows cover
both structures. The kernel holds its World lock across the operation and
updates `last_arena` only after success. Existing syscall numbers and error
channels remain unchanged. No heap allocation or new dependencies are required.

`SYS_MAP` now looks up the arena by its capability's object ID and checks its
owner, so allocating another arena no longer makes an older live arena cap
unusable. The physical pin still derives exclusively from the owned arena.
This does not expand `SYS_UNMAP`, change the software DMA implementation, or
remove the existing last-arena policy shortcut in accelerator submission.

Ten host tests cover geometry boundaries, transaction success/failure, 128
full-cap-table refusals with exact state preservation, retry, capacity recovery
and older-cap lookup. The userspace boot regression allocates two arenas, then
maps the older one and the newer one. Every architecture's QEMU recipe requires
`[init] older arena capability survives allocation` before it can pass.

The cap-table-full regression is a host boundary check; the current demo has a
smaller arena table than cap table, so this is not evidence of a reachable
userspace cap-exhaustion exploit. Tests and QEMU are software validation, not
hardware isolation or formal verification. Arena freeing and capability
revocation remain separate operations for their existing trusted callers.
