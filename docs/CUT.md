# ChipletSpectralCut and FlowHodgeQuota

Research-prototype interfaces. v0.1 does **not** run a floating-point
eigensolver or a production traffic engineer. It does encode the
**capability surface and refusal rules** a silicon OS would want.

## ChipletSpectralCut

A cut is a kernel object (`CapKind::SpectralCut`) naming a balanced
bipartition of the package affinity graph (tiles + HBM banks, weighted
on-die vs EMIB edges).

```
require(cptr, SpectralCut, BIND)
    → allow_place(tile, bank)
        refuse CrossCut if tile and bank are on opposite sides
        refuse ConductanceExceeded if Φ(cut) > bound (mint-time)
```

**Intended construction:** Fiedler vector of `L = D − A` (or a multicut
when k > 2). Sign-split, then optionally improve by Kernighan–Lin.
**v0.1:** integer conductance
`Φ = 1000 · cut(S,V\S) / min(vol S, vol V\S)` and, for n ≤ 8, enumerate
balanced masks (`SpectralCut::min_balanced`). On the QEMU 2-chiplet
graph the min-Φ split *is* the chiplet cut (weak inter-die edges).

`AffinityLaplacian` (`core/src/laplacian.rs`) is the first-class `L`
object. It exposes Rayleigh (`rayleigh_milli`), a Fiedler-ish power
iteration + sign-split (`fiedler_mask`), and heat / commute-time
distance helpers. `SpectralCut::from_fiedler` builds a cut from that
mask. Placement still enumerates for n≤8; the Laplacian is what a
later large-n eigensolve would feed. Arithmetic is integer /
milli-fixed-point — not a production eigensolver.

Tasks bind via `Job.cut_id`. `TileScheduler::pick` scores a violating
tile as impossible (`i32::MIN`) — same as a CPU tile trying to run an
NPU wave. A `PartitionProfile` is the spatial/QoS object the scheduler
and accel also bind; the cut is the graph bipartition, the partition is
the isolation quota. Both are capabilities.

QEMU topology (static):

```
chiplet 0: CPU0, NPU2, bank0     (strong)
chiplet 1: CPU1, GPU3, bank1     (strong)
EMIB:      CPU0—CPU1, NPU—GPU, bank0—bank1  (weak)
```

## FlowHodgeQuota

Every fabric header carries `FlowClass { Gradient, Curl, Harmonic }`.
`Fabric::send` admits the class against a per-link quota **before**
enqueue.

| Class | May TREE_OFFLOAD? | Ring reserve | Cap badge bit |
| --- | --- | --- | --- |
| Gradient | yes (allreduce/broadcast tree) | optional | `CLASS_GRADIENT` |
| Curl | **no** (`CurlOnTree`) | yes | `CLASS_CURL` |
| Harmonic | **no** (`HarmonicTreeReduce`) | n/a | `CLASS_HARMONIC` |

`CapKind::FlowQuota` badge is the authorized class mask. WRITE required.
Tenant B without the cap cannot authorize Harmonic.

### Why harmonic must not be tree-reduced

A harmonic component is a persistent cycle (non-trivial homology). Folding
it onto a spanning tree:

1. Identifies distinct cycle edges as one tree buffer. Two harmonic
   collectives can then hold each other's credit — **deadlock**.
2. Destroys the homology class. Barrier / pipeline progress that assumed
   a cycle no longer holds.

That refusal is enforced even on QEMU's single virtual interconnect.

## Related

- **AffinityLaplacian** — implemented (integer prototype). See above.
- Still on the roadmap: `OperatorKernelHandle`, `SparsifiedCollective`
  ([ROADMAP.md](ROADMAP.md)).
