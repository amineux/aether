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
balanced masks. On the QEMU 2-chiplet graph the min-Φ split *is* the
chiplet cut (weak inter-die edges).

Tasks bind via `Job.cut_id`. `TileScheduler::pick` scores a violating
tile as impossible (`i32::MIN`) — same as a CPU tile trying to run an
NPU wave.

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

## Related (ROADMAP, not in v0.1 code)

See [ROADMAP.md](ROADMAP.md): `AffinityLaplacian`, `OperatorKernelHandle`,
`SparsifiedCollective`.
