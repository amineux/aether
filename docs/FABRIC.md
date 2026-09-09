# Fabric IPC

The fabric is the **only** IPC in Aether. There are no signals, no
`AF_UNIX`, no global named ports, and no “just write this PA.” If two
tasks communicate, a capability was minted or transferred.

## Messages

```
MsgHeader
  dest          EndpointId     object table, not a CPtr
  badge         u64            set at mint; receiver sees sender's badge
  flags         SYNC | ASYNC | GRANT | REPLY | TREE_OFFLOAD | RING_RESERVE
  n_caps        0..4           caps moved/copied with the message
  payload_len   ≤ 64 bytes     control plane only
  route         ChipletRoute   die / chiplet / tile / hop_hint
  sender_tenant TenantId
  flow          FlowClass      Gradient | Curl | Harmonic
  phase         Phase          Compute | Exchange | Barrier
```

Bulk tensor data does **not** ride in the payload. It rides in a Memory
cap attached to the message (`GRANT`) or already mapped to an accel queue.

### Sync vs async

- **ASYNC**: enqueue and return. If the queue is full, `QueueFull`
  (a blocking send waiter list is the next cut).
- **SYNC**: same queue; the flag tells the scheduler the sender is waiting
  for a matching `recv` (or a reply). v0.1 records the flag; a real block
  is only meaningful once we have multiple runnable threads.

### Chiplet route tags

`ChipletRoute { die, chiplet, tile, hop_hint }` is ignored by the v0.1
single-package router except for being copied end-to-end. The point is the
**ABI**: a mesh, EMIB, or UALink hop can steer on the header without
parsing tensors. Silicon partners should treat these four bytes as
architectural.

### Hodge class (FlowHodgeQuota)

`flow` plus `TREE_OFFLOAD` / `RING_RESERVE` is enforced in `Fabric::send`
(`core/src/hodge.rs`). Gradient may tree-offload; curl and harmonic must
not. See [CUT.md](CUT.md).

An `OperatorKernelHandle` (`CapKind::OperatorKernel`) is a compiled
collective bound to one of those classes. Tree topology implies
`TREE_OFFLOAD` and therefore refuses Curl / Harmonic at bind time.
Inject writes the handle's class and flags into the header; a
mismatched class is refused before enqueue. This is a cap, not a
compiler. See [CUT.md](CUT.md).

`SparsifiedCollective` may wrap that handle (or a FlowClass header)
with an integer milli energy and a threshold. Harmonic components
strictly below the threshold are dropped before `Fabric::send`
(quota untouched). Gradient and Curl are unchanged. Harmonic+TREE
is still refused — Drop does not skip Hodge policy. Integer
fixed-point only; not an eigensolve. See [CUT.md](CUT.md).

### SoftNoI-IS (admit, not topology)

SoftChipletSync may advertise a per-tenant Interference Score on a
**fake** shared Network-on-Interposer (`core/src/noi.rs`). Soft-CP
XQueue admit refuses when projected `IS = max T_solo / T_con` exceeds
the budget (canonical 1.5×). DMA / collective descriptors may carry a
software `FlowClass` tag at submit (`CollectiveKind::fabric_class`:
allreduce/tree → Gradient, ring-exchange → Curl, persistent →
Harmonic). Curl also needs reserved ring capacity. Same demand can
admit as Gradient and refuse as Curl — class is an admit input, not a
renamed IS. Not a `CpCmd` / path-B vendor header. PARL / NoI
inspiration ([arXiv:2510.24113](https://arxiv.org/abs/2510.24113)).
This is **runtime admit control**, not PARL topology synthesis, not
UniCNet, not optimal NoI design, not FLOPs. See [ACCEL.md](ACCEL.md).

### Spectral cuts

Placement is not only affinity hints. A `SpectralCut` cap binds a job to
one side of the package graph. Cross-cut tile/bank pairs are refused.
See [CUT.md](CUT.md).

## Endpoints

An endpoint is a kernel object with a small circular queue (8 messages).
Creating one returns an `EndpointId` and a cap in the creator's table.
`send` looks up the destination object (the sender needs a cap to *some*
endpoint that names that id — in v0.1 the built-in init holds both ends;
a later cut checks the sender's `CPtr` on every send).

## Cap transfer

`Message::attach_cap` plus `MsgFlags::GRANT` is how a tensor arena moves
from a runtime to an NPU queue:

1. Tenant A allocates an arena, holds a Memory cap.
2. A derives `READ|WRITE|MAP` (cannot escalate).
3. A sends a GRANT message to the NPU driver's endpoint.
4. The kernel inserts the cap into the driver's table with the **driver's
   tenant id** (or a trusted driver identity).
5. A's original slot is emptied if the transfer was a move.

See [SECURITY.md](SECURITY.md) for the isolation argument.

## Why not shared memory IPC?

On a coherent SMP, shared memory is cheap. On a package where the NPU's
view of HBM is not in the CPU's coherence domain, “we both have the
pointer” is a bug. The fabric makes the transfer **visible** so a later
IOMMU / cache-maintenance hook has a place to run.

Memory is a typed place: tile SRAM, HBM, CXL region—never a single address space by default.
A `FabricAddr` is `(place, local)`. Crossing a place is an Exchange
phase, not a load. Chiplets extend the NoC; UCIe is transport, not the
programming model. `TypedWindow` (`Hbm` / `CxlMemStub` / `Dram`) is an
exploration stub Soft SMMU can pin with a SID — CXL.mem nouns only,
not a HDM decoder. See [WINDOW.md](WINDOW.md).
