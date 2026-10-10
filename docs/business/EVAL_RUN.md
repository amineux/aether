# Host evaluation harness

`make eval-run` runs `examples/aether-eval-run` on a Linux host. It writes
`build/eval-run.json` and prints the same JSON on stdout. The human
summary is on stderr. Exit status is 0 only when the metadata is
complete, the SoftGreenCtx checks pass, and every named refusal passes.

This is a software model. It does not fill [EVALUATION.md](EVALUATION.md)
or [PILOT.md](PILOT.md) by itself. A customer evaluation still needs an
agreed workload, environment, baseline, and thresholds recorded in those
worksheets before the run.

## What the binary runs

One [`SoftCommandProcessor`](../../drivers/src/fakecp.rs) (the software
NPU command processor) with two tenants on one SoftGreenCtx SM/WQ pool:

- solo: each tenant alone on the full pool (one measurement; A and B match)
- partitioned 70/30: tenant A on the 70% slice, tenant B on the 30% slice
- unpartitioned: both tenants share the pool

The integers are `bw_milli` and `interference_milli` from
`run_greenctx_demo`, cross-checked against `submit_memcpy` on that
shared processor. 1000 is solo full-pool memcpy bandwidth. They are
not FLOPs.

Named refusals call existing library demos and must pass:

| `refusals[].name` | Library | Refusal |
| --- | --- | --- |
| `smmu-unmap-cross-tenant` | `run_smmu_unmap_cross_tenant_demo` | `MapError::CrossTenant`; the other tenant's pin stays mapped |
| `smmu-wrong-stream` | `run_smmu_wrong_stream_demo` | `MapError::WrongStream` on a submit SID mismatch |
| `softcmdfirewall` | `run_firewall_demo` | copy-then-validate holds; an in-place command mutation does not |

`seed` is JSON `null`. This harness does not sample. The tenant-fuzz
seed (`0x5AE7`) belongs to `make red-team`, not to this report.

## JSON fields

| Field | Meaning |
| --- | --- |
| `schema` | `aether-eval-run/v1` |
| `passed` | Metadata present, workload checks true, every refusal passed |
| `git_commit` | `git rev-parse HEAD` |
| `git_dirty` | `true` when `git status --porcelain` is non-empty. The SHA then does not uniquely name the tree. `passed` can still be true; the summary says so |
| `rustc_version` | `rustc --version` |
| `cargo_version` | `cargo --version` |
| `cargo_lock_sha256` | SHA-256 of `Cargo.lock` |
| `seed` | always `null` here |
| `workload.tenants.A` / `.B` | `solo`, `partitioned_70_30`, and `unpartitioned`, each with `bw_milli` and `interference_milli` |
| `workload.shared_cp_matches` | Soft-CP memcpy integers match `run_greenctx_demo` |
| `workload.split_ok` / `interference_ok` / `not_mig` / `migrate_ok` | The demo's own checks. `not_mig` means residual shared-bandwidth tax: partitioned bandwidth stays below solo |
| `refusals` | The three named checks above |
| `claims.not_proven` | Explicit list of what this file does not establish |

`passed: false` when git, rustc, cargo, or the lockfile hash is missing,
when the milli cross-check fails, or when any refusal fails.

## Map onto the evaluation worksheet

[EVALUATION.md](EVALUATION.md) acceptance rows, and the same rows
[PILOT.md](PILOT.md) points at:

| Worksheet criterion | What `eval-run` supplies | What it does not supply |
| --- | --- | --- |
| Unauthorized access refusal | `refusals` for cross-tenant unmap, wrong SID, and SoftCmdFirewall. A pass is a host-library result, not a customer threshold | The agreed threshold, the valid control on the customer's workload, and any attack the customer adds |
| Fault containment | Not measured. Do not mark this row passed from this JSON | A faulty workload running beside a legitimate one |
| Legitimate workload overhead | `workload.tenants.*.bw_milli` and `interference_milli` for solo, 70/30, and unpartitioned. Units are integer milli on a software memcpy model | A customer baseline, a timing method, or a claim versus MIG. `not_mig` records that partitioned milli stays below solo because of the model's shared-bandwidth tax |
| Recovery and integration effort | Not measured | Engineer hours and a recovery procedure |
| Reproduction bundle | `git_commit`, `git_dirty`, `rustc_version`, `cargo_version`, `cargo_lock_sha256`, and the command `make eval-run` | Hardware, QEMU, or a customer signature. `scripts/collect_evidence.py` stores this command's stdout with the other host logs |

Copy the JSON into the worksheet's actual-result column only after the
partner and Aether have agreed the criterion. Leave unrun rows blank.
A software-only run cannot close a hardware-isolation gate.
