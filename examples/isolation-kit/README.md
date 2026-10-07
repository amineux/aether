# aether-isolation-kit

Pre-silicon **tenant-isolation conformance kit** (use case B). Host-only.
Not certification, not a partner or customer result, no hardware or
performance claims.

## What it is

A chip team describes its backend — command/stream format, accelerator,
SMMU, fabric — by implementing one small trait:

```rust
pub trait IsolationBackend {
    fn name(&self) -> &str;
    fn applies(&self, class: &AttackClass) -> bool { true } // default
    fn probe(&self, class: &AttackClass) -> Outcome;        // Refused / Accepted / NotApplicable
}
```

The kit runs Aether's existing named attack classes (`CLASSES`) against the
backend and prints a per-backend matrix of **attack class → refused /
ACCEPTED / n/a** plus a grep-able one-line summary. It adds no isolation
mechanism of its own.

## Run it

```
make isolation-matrix
# or, directly (this crate is its own workspace):
cargo run --manifest-path examples/isolation-kit/Cargo.toml
```

## Shipped adapters

- **`aether-soft`** — the reference adapter. Each class calls the library
  demo that already drives the Soft\* refuse path, so it refuses every
  applicable class: `result=conformant`, `accepted=0`.
- **`weak-sample-example-only`** — EXAMPLE ONLY. A deliberately weak teaching
  stub (not a real backend, not any third party) that enforces arena
  ownership and job shapes but trusts caller-supplied addresses/streams and
  never bounds the fabric. Its matrix shows real accepts
  (`result=NONCONFORMANT`), proving the kit can fail.

## Bringing your own backend

Implement `IsolationBackend` for your adapter, return `NotApplicable` from
`applies` for surfaces you do not have, and call `Matrix::run(&your_backend)`.
`Matrix::summary_line()` gives the stable `[isolation-kit] backend=… result=…`
line; `Matrix::table()` gives the readable grid.

## Relationship to the eval harnesses

Separate on purpose. `examples/aether-eval-run` (PR #189) is a single-backend
host evaluation report; this kit is a multi-backend conformance matrix keyed
on a trait. `tools/runtime-eval` (PR #193) measures CPU invocation latency and
marks isolation unsupported; this kit covers that isolation axis. The summary
line is designed to be registered later as an evidence check feeding the
investor report (PR #190) without changing the kit; that wiring belongs to the
owners of `scripts/collect_evidence.py` and `ci.yml`.
