# Two-tenant blast radius (diligence clip)

A **one-week clip**, not a track. v0.1 does not run a hardware SMMU or
a second ring-3 tenant. Host tests and the kernel self-check execute
the same [`run_blast_demo()`](../core/src/blast.rs). Serial: `[blast]`.

## Proof

Tenant A and B each mint Memory, Activity, and SpectralCut.

1. **Two tenants.** B does not `holds` A's Memory / Activity / Cut.
   Cross-tenant `mint` is `CapError::CrossTenant`.
2. **CrossCut.** Same-side NPU+bank0 binds. Tile 1 + bank 0 is
   `CutError::CrossCut`. B cannot `bind_place` A's cut (`NotBound`).
3. **Wrong SID.** A's arena pins on SID-A (`chiplet0|tile2`). Walk of
   that IOVA on unbound SID-B is `StreamAbort`. After B binds SID-B
   for B's arena, walking A's IOVA on SID-B is `WrongStream`.

```
[blast] tenant A=1 B=2 Memory/Activity/Cut refuse  ok
[blast] SpectralCut CrossCut refuse  ok
[blast] Soft SMMU wrong SID abort  ok
[blast] two-tenant blast radius sealed
```

CI greps those four lines. No new syscall. SoftNPU I32 is unchanged.
No FLOP numbers. Soft SMMU is still software. The partner host clip
`make diligence-demo` prints the same `[blast]` lines without QEMU.

SID-at-submit (Host1x-shaped) is a separate clip: [`run_sid_submit_demo()`](../core/src/sid.rs),
serial `[sid]`. Bind-at-map is not enough on the Soft-CP / IreeShapedCp
path. See [ACCEL.md](ACCEL.md).
