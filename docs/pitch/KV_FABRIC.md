# KV fabric — four minutes for a GPU-package CTO

Research prototype. Say this out loud. Then run one command.

Not an NVIDIA partnership. Not NVLink. Not a CUDA graph. Not MIG.
Not confidential computing. Not a measured TTFT. Soft SMMU is
software. The byte counts below are a toy page so the arithmetic is
exact. The invariant does not get more true at a larger size.

Site: [https://amineux.github.io/aether/#kv](https://amineux.github.io/aether/#kv).

```bash
make kv-fabric
# or: cargo run -p aether-kv-fabric
```

## The sentence

You already partition the SMs. You still hand the KV cache to decode
as a pointer. When prefill and decode are different GPUs, and a
second tenant shares the HBM bank, the pointer is the bug.

Aether hands decode a **32-byte capability**: read this token window,
of this sequence, until the producer revokes it. Weights never enter
the grant. The neighbor's stream cannot walk the page. When the
sequence ends, the capability dies, including the copy that was
derived from it.

## What the command proves

Same HBM bank. Two tenants. Prefill and decode are two GPUs of
tenant A. Tenant B is the neighbor.

| Check | Result |
| --- | --- |
| Handoff | Decode receives READ\|MAP only. Token 16 of layer 1 attends. |
| Fabric vs copy | Grant record is 32 bytes. The KV page is 131072 bytes and stays put. Weights are 8388608 bytes and are a different object. |
| Write | Decode store → refused. |
| Re-grant | Decode cannot hand the page to a sidecar. |
| Weights | The KV cap does not name the weight object. |
| Stolen id | Neighbor can put A's object id in their own cap table. Attend still refuses: the object knows its tenant. A slot number is not authority. |
| Window | Token 128 (one past the window) → refused. |
| DMA | Decode's translation is not writable. Neighbor SID on decode's IOVA → wrong stream. Weights are not mapped on the decode stream. |
| Deadline | A's wave misses a software tick budget. B's sequence still attends. A miss does not widen A's grant. |
| Revoke | End of sequence empties the decode cap. B still holds B's page. |

Stdout needles (CI greps these):

```
[kv] handoff read-only seq=1 layers=0..4 tokens=0..128
[kv] fabric-bytes=32 copy-bytes=131072 weights-stay=8388608
[kv] attack=forge result=refused
[kv] attack=write result=refused
[kv] attack=regrant result=refused
[kv] attack=weights result=refused
[kv] attack=oob result=refused
[kv] attack=wrong-sid result=refused
[kv] attack=revoke result=refused
[kv] deadline-miss seq=1 neighbor=live
[kv] dma decode-writable=false
[kv] not-nvlink not-cuda not-mig soft-smmu=software
```

## How to say it

**0:00.** Multi-tenant inference on a package already moved the
expensive object. It is not the SM. It is one sequence's KV window,
produced on one GPU and read on another, while the weights stay
resident. MIG and a Green Context name the processors. They do not
name that window, and they do not give it a lifetime.

**1:00.** Run `make kv-fabric`. Point at `fabric-bytes=32` and
`weights-stay=8388608`. The fabric moved authority. It did not move
the tensor.

**2:00.** Walk the refuses in order: forge, write, regrant, weights,
out of window, wrong stream, revoke. Stop on forge. Say: naming the
page inside your own cap table is still not enough. The object is
painted with a tenant. That is the bug class behind a KV
use-after-free and a cross-tenant snapshot.

**3:00.** Deadline line. A's wave is dropped. B, on the same bank,
still attends. Then the non-claims, in one breath: software SMMU,
toy sizes, no silicon, no FLOPs. The ask is the opcode table for
their command processor, filled into `docs/DESIGN_WIN.md`. A written
no is a good outcome.

## What this is not

- Not a driver for any vendor GPU.
- Not a claim that 32 bytes is the NVLink traffic of a real
  disaggregated serving stack. A real stack still streams the KV
  when the page is remote. The claim is the *authority* is 32 bytes
  and revocable, and the weights are not in it.
- Not hardware isolation. A device that ignores the software tables
  can still DMA. That is why the ask is their stream-id and opcode
  table, not a logo.
