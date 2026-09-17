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

The combined buyer stdout is `make red-team` ([DILIGENCE.md](DILIGENCE.md)):
wrong-SID/CrossCut plus SoftCmdFirewall, SoftSFI, SoftNoI-IS, and
PASID/SVA, each as `[redteam] attack=… result=refused`.

## Blast hops (red-team needle)

`PartitionProfile::admit_hops` is unit-tested in `partition.rs`. The sell
surface is host red-team only — **not** a fourth `[blast]` serial line:

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Two tenants / two slices; in-budget hops admit; over `max_hops` refuse | `run_blast_hops_demo` → `PartitionError::BlastRadius` | `[redteam] attack=blast-hops result=refused` |

CrossCut / wrong-SID stay on `attack=wrong-sid-crosscut`. No World CapTable
split, no new syscall, no opcode churn.



## Blast nodes (red-team needle)

`PartitionProfile::admit_nodes` is unit-tested in `partition.rs`. The sell
surface is host red-team only — **not** a fourth `[blast]` serial line and
**not** a hops rehash (hops stays `attack=blast-hops`):

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Two tenants / two slices; in-budget nodes admit; over `max_nodes` refuse | `run_blast_nodes_demo` → `PartitionError::BlastRadius` | `[redteam] attack=blast-nodes result=refused` |

CrossCut / wrong-SID stay on `attack=wrong-sid-crosscut`. Blast hops stay on
`attack=blast-hops`. No CapTable split, no new syscall, no opcode churn.

## Bank color (red-team needle)

`admit_wave` is unit-tested in `color.rs`. The sell surface is host
red-team only — existing path; **not** a new isolator:

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Same-color Compute admits; foreign bank refuse; Exchange still OK | `run_bank_color_demo` → `ColorError::ForeignBank` | `[redteam] attack=bank-color result=refused` |

CrossCut / wrong-SID stay on `attack=wrong-sid-crosscut`. Blast hops stay
on `attack=blast-hops`. No CapTable split, no new syscall, no opcode churn.

## Uncolored compute (red-team needle)

`admit_wave(..., color=None)` is unit-tested in `color.rs`. The sell surface
is host red-team only — existing path; **not** a ForeignBank / bank-color
rehash (that stays on `attack=bank-color`):

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Colored Compute admits; uncolored Compute refuse; Exchange with color still OK | `run_uncolored_compute_demo` → `ColorError::Uncolored` | `[redteam] attack=uncolored-compute result=refused` |

CrossCut / wrong-SID stay on `attack=wrong-sid-crosscut`. Blast hops stay on
`attack=blast-hops`. Bank color stays on `attack=bank-color`. No CapTable
split, no new syscall, no opcode churn.

## QoS credits (red-team needle)

[`Timeline::submit`](../core/src/fence.rs) meters [`QosBudget::credits`](../core/src/partition.rs).
In-budget submits admit; `in_flight >= credits` → [`PartitionError::CreditExhausted`](../core/src/partition.rs).
Complete / timeout frees a credit and admit resumes. Host red-team only —
**not** EventRing theater, not a second charge API, not a `[blast]` serial line:

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Two tenants / two slices; in-budget submits admit; over credits refuse; complete/timeout resume | `run_qos_credits_demo` → `PartitionError::CreditExhausted` | `[redteam] attack=qos-credits result=refused` |

CrossCut / wrong-SID stay on `attack=wrong-sid-crosscut`. Blast hops stays on
`attack=blast-hops`. Bank color stays on `attack=bank-color`. No CapTable
split, no new syscall, no opcode churn.

## Outside slice (red-team needle)

`PartitionProfile::admit_chiplet` is unit-tested in `partition.rs`. The sell
surface is host red-team only — existing path; **not** hops / qos / CrossCut /
bank-color:

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Two tenants / two chiplet slices; own chiplet admits; foreign chiplet refuse | `run_outside_slice_demo` → `PartitionError::OutsideSlice` | `[redteam] attack=outside-slice result=refused` |

CrossCut / wrong-SID stay on `attack=wrong-sid-crosscut`. Blast hops stays on
`attack=blast-hops`. Bank color stays on `attack=bank-color`. QoS credits stays
on `attack=qos-credits`. No CapTable split, no new syscall, no opcode churn.


## Silent remote (red-team needle)

[`map_place`](../core/src/space.rs) / HAL `map_fabric` are unit-tested in
`space.rs` / `hal`. The sell surface is host red-team only — existing path;
**not** CXL productization, UNIFIED-as-default, BAR0, or SoftNPU ops.
`MEM_FULL` never implies `UNIFIED`:

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Local map admits; silent remote `(place, local)` refuse; UNIFIED not default | `run_silent_remote_demo` → `SpaceError::SilentRemoteLoad` | `[redteam] attack=silent-remote result=refused` |

CrossCut / wrong-SID stay on `attack=wrong-sid-crosscut`. Outside-slice stays
on `attack=outside-slice`. No CapTable split, no new syscall, no opcode churn.


## TypedWindow SID (red-team needle)

`IommuMap::map_window_sid` is unit-tested in `window.rs`. The sell surface is
host red-team only — existing TypedWindow pin/map path; **exploration stub**,
**not** CXL.mem silicon / QEMU CXL / BAR0. Sibling foreign pin is
`MapError::CrossTenant`:

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Matching SID admits; mismatched SID refuse; foreign tenant pin refuse | `run_typed_window_sid_demo` → `MapError::WrongStream` (+ `CrossTenant`) | `[redteam] attack=typed-window-sid result=refused` |

CrossCut / wrong-SID DMA stay on `attack=wrong-sid-crosscut`. Outside-slice
stays on `attack=outside-slice`. No CapTable split, no new syscall, no opcode
churn, no CXL.mem productization.

## SoftSFI tensor (red-team / softsfi serial needle)

`SoftOp::Tensor` is unit-tested in `softsfi.rs` as `SfiError::Unmodeled`.
Sell surface matches heap style — **not** a bump allocator, not CapTable /
SoftNPU opcode churn:

| Proof | Mechanism | Grep |
| --- | --- | --- |
| Named tensor / TMA-shaped opcode refuse | `run_softsfi_demo` → `SoftOp::Tensor` → `SfiError::Unmodeled` | `[softsfi] tensor=refused` |

Heap stays on `[softsfi] heap=refused`. No CapTable split, no BAR0, no new syscall.

SID-at-submit (Host1x-shaped) is a separate clip: [`run_sid_submit_demo()`](../core/src/sid.rs),
serial `[sid]`. Bind-at-map is not enough on the Soft-CP / IreeShapedCp
path. See [ACCEL.md](ACCEL.md).
