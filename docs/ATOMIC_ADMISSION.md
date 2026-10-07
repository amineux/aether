# Atomic workload admission

`make atomic-pipeline` runs two tenants through the existing software command
processor. A malformed buffer plan is refused without publishing a successful
prefix. The neighbor's matrix result remains correct, the caller retries a
corrected plan, and cleanup preserves the neighbor's mappings.

The reusable API is `IommuMap::map_batch`, in `core/src/admission.rs`. It accepts
up to `MAX_MAPS` capability/request pairs and returns committed regions in request
order. Failures identify the request index and the existing `MapError` cause.
An empty batch is a no-op; oversized batches fail before staging.

## Why the boundary matters

A workload commonly needs input, weights, output and scratch buffers. Calling
`map` in a loop can admit the first buffers and refuse a later one. Unmapping
the admitted prefix is insufficient to restore stream bindings, page-table
configuration, submit state or cached translations. It also burdens every
caller with a subtly different recovery implementation.

Batch admission stages the existing checks against a private table copy. The
live table is replaced only after every request succeeds. A failure drops the
staged copy; nothing is committed. Rust's exclusive `&mut IommuMap` prevents
interleaved table mutation within this operation. No deferred admission token
can become stale between preparation and commit.

The caller must obtain capabilities through its authorized lookup and validate
buffer extents against owned arenas. Like `map`, this host API accepts a
`Capability` value; it is not a new untrusted syscall and does not turn an
arbitrary capability struct into authority. Existing SMMU checks are reused,
including Memory+MAP rights, tenant/stream ownership, overlap, checked ranges,
SID budgets and finite table capacity. This API does not make existing raw
mapping/reset APIs safe for untrusted callers.

## Demonstrated behavior

- Nine regression tests cover success ordering, failure at every prefix
  position, invalid rights/kinds, foreign streams, overflow, overlapping
  buffers, capacity exhaustion, empty and oversized requests.
- A warm-cache test compares the complete table state before/after failure,
  including private fields, ATC counters and submit SID.
- 256 seeded failure/retry/cleanup cycles preserve an unrelated tenant's live
  mapping and reclaim buffer slots. This is bounded testing, not a proof.
- A sequential negative control leaves a partial mapping behind, showing
  that the rollback assertions distinguish the old calling pattern.
- The executable consumer performs software MatMul before refusal, after
  refusal, after retry, and after cleanup; its output assertions are CI gates.

## Cost and integration limits

The core implementation is `no_std`, allocation-free and safe Rust. Staging
copies one full software SMMU table. Its current size is **38,800 bytes** on the
tested 64-bit host, plus return values and compiler-dependent stack usage.
That is bounded, but it is not a measured performance improvement. The example
prints `size_of::<IommuMap>()` so a changed layout is visible. Account for stack
headroom and measure admission latency before enabling it on a kernel hot path.

This API is consumed by a host example through `SoftCommandProcessor.iommu`.
It is not wired into kernel syscalls, a real IREE HAL driver, hardware SMMU
registers, or accelerator queue submission. It atomically admits mappings;
it does not roll back arena allocation, capability minting, device writes or
already-running jobs. No syscall, opcode, ABI or frozen `IreeHalCmd` changes.

## Application paths

| Application | Buffer group | Next validation needed |
| --- | --- | --- |
| Camera/edge inference | Frame, projection weights, output, scratch | Real model and camera replay; integrate the runtime adapter with the backend |
| Robotics/control | Sensor window, model state, action output | Worst-case admission time and missed-deadline policy |
| Shared accelerator services | Per-request input, weights and output | Kernel-owned capability lookup, extent checks and quota/queue admission |
| Pre-silicon validation | Multiple pin requests plus fault injection | Vendor adapter that tests its actual backend state |

The long-term architecture is one workload admission boundary for resource
reservation and execution, with explicit ownership, bounded recovery and
replayable evidence. This PR implements its software mapping stage. A hardware
implementation needs backend-specific prepare/commit/abort semantics and
completion fencing; copying a Rust table cannot roll back hardware DMA.
