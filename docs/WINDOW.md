# Typed memory windows (Exploration E)

**Falsifier: not a calendar milestone.** This is an honest software stub.
CXL.mem is **inspiration for typed fabric memory**, not a product claim.

## What this is

A host/kernel range Soft SMMU / `AccelDevice` can pin with a stream ID
and Memory+MAP rights:

```text
TypedWindow { base, len, kind: Hbm | CxlMemStub | Dram, sid }
```

- `IommuMap::map_window` / `unmap_window` (and `map_window_sid`).
- Wrong SID → `MapError::WrongStream`.
- Foreign-tenant pin → `MapError::CrossTenant`.
- SpectralCut `bind_window` / `allow_window` → `CutError::CrossCut` if
  the window belongs to another tenant.
- Optional `EventRing` lines: `WindowMap`, `WindowRefuse`.
- Host tests (`cargo test -p aether-core window`). QEMU CXL
  (`cxl-type3`, CXL.host) is **not** used.

`CxlMemStub` maps to the existing typed place `MemorySpace::CxlRegion`.
Coherent remote load is still refused without `UNIFIED`.

## What this is not

- Not a CXL.mem Host-managed Device Memory decoder
- Not QEMU `cxl-type3` / CXL.host / `.mem` silicon
- Not cache-coherent fabric memory
- Not a SpecForge Y2H1 “CXL region objects” check-off
- Not a reason to say Aether “has CXL”

See [YEAR2_PLAN.md](YEAR2_PLAN.md) (KILL as milestones) and
[ROADMAP.md](ROADMAP.md). Hardware CXL.mem still requires partner silicon
and a real window decoder; this tree does not pretend it landed.
