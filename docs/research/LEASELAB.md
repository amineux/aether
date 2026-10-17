# Aether LeaseLab: locality-aware, epoch-scoped admission

Status: **proposal + executable browser model**, 15 October 2026. Neither a
kernel feature nor a measured accelerator result. Novelty/patentability has
not been investigated. This is a concrete research direction to falsify.

## Problem and proposed mechanism

A decoder can copy KV state to a free tile, execute near its existing state,
or read that state remotely. A short grant avoids a bulk copy only when the
consumer can access the data where it resides; remote reads still cost bytes.
The proposed planner compares completion estimates before selecting a path.
It pairs the decision with a lease tuple:

`(tenant, region, byte range, read/write rights, epoch, completion fence)`.

A future admission implementation would check the tenant, rights, range and
current region epoch before queue submission; revocation would close admission,
advance the epoch, drain/cancel prior work, invalidate device translations and
only then permit reuse. A browser check alone cannot establish these properties.
The included protocol sketch checks tenant, epoch, permissions and range; it
does not model fences, DMA, concurrency or hardware invalidations.

## Existing foundations and new work

- Existing: `core/src/kvfabric.rs` and `examples/kv-fabric/src/main.rs` have a
  software KV-grant demo, a 32-byte record, read-only checks and refusal paths.
- Existing: scoped synchronization, capability ownership and software SMMU
  components described in the repository README.
- New: `site/js/lease-model.mjs` implements the analytical planner and admission
  sketch. `site/simulation.html` makes the assumptions and decisions inspectable.
- Not implemented: kernel planner integration, hardware leases, race-safe epoch
  invalidation, real inference execution or measured throughput/power savings.

## Reproducible analytical model

Let B = KV MiB × 1,048,576; bandwidth G uses decimal GB/s. Transfer T in μs is
B / (G × 1,000). Compute C = 200 μs × read passes R is an arbitrary scenario
assumption held equal across alternatives. Home contention q is a fraction;
wait W = C × q / (1 − q) is a heuristic, not a queueing measurement. Setup H
is a user-controlled assumption.

| Path | Completion estimate (μs) | Cross-fabric bytes |
| --- | --- | --- |
| Copy to a free tile | T + C | B |
| Local lease | H + W + C | 32 |
| Remote lease | H + R × T + C | 32 + R × B |

Select the minimum, with ties favoring copying. Report copy time / selected
time. Local memory traffic is excluded from every fabric-byte count. The
32-byte grant is an illustrative control-plane cost inspired by the toy demo,
not a real hardware packet specification. Copy-path control costs are omitted.

With R >= 1 and H >= 0, the remote path cannot beat copying in this model.
It is shown to expose the cost of remote reads, not as a secretly disadvantaged
competitor. Overlap, limited destination capacity, caches, asymmetric bandwidth
and shared remote reuse could change that outcome; none is modeled here.
The planner speedup is at least 1 by construction because copying is itself
an option. It is not a prediction that a real planner can never regress.

Default: 256 MiB, 64 GB/s, 4 passes, 25% contention, 20 μs setup. Copy takes
4,994.304 μs; local lease takes approximately 1,086.667 μs. These are computed
scenario values. Busy-tile and small-payload presets intentionally select copy.
An exported JSON contains parameters, all alternatives, selected path, model
version, assumptions and the protocol trace if the experiment has been run.

Run checks: `node --test scripts/test_leaselab.mjs`.

The admission sketch rejects malformed grant/request records before checking
authority. Tenant identifiers must be nonempty strings; permissions must be
explicit booleans; grant/current epochs must be nonnegative safe integers;
and grant byte counts must be positive safe integers. Request offsets and
lengths must be safe integers with a nonnegative offset and positive length.
The range check uses subtraction to avoid losing integer precision at the
largest supported byte count. Invalid records return a refusal, not permission
inferred from coercion or missing fields. The dedicated LeaseLab CI workflow
runs the analytical tests, malformed-input regressions, and an exhaustive small
range check against an exact-integer oracle. This is still only a protocol
sketch: epochs do not implement quiescence, prevent wrap/reuse, or invalidate DMA.

## Validation and rejection plan

1. Secure a runtime team's trace and environment; establish baseline copying,
   existing runtime locality optimizations and equal isolation requirements.
2. Measure real setup, local/remote transfer, compute and queue delays. Replay
   the identical workload under all valid strategies; record p50/p99 latency,
   bytes, throughput, fairness and failures. Include planner overhead.
3. Test stale lease replay, tenant mismatch, read-only writes, boundary overflow,
   epoch wrap, concurrent revoke/submit, stalled DMA, device reset and neighbor
   progress. Memory reuse must wait for confirmed quiescence, not a timeout.
4. Reject or revise if existing runtime placement matches the result, if setup
   erases the benefit, if p99/fairness regresses, or if hardware cannot enforce
   invalidation without globally stopping unrelated tenants.

Commercial hypothesis: an accelerator runtime team could evaluate a reusable
admission contract and measurement harness. No customer, partner, silicon
support, revenue or industry-wide impact is established by this model.
