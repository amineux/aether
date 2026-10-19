# Prove it: one command per claim

Each row is a claim this repo already makes, the one command that reproduces
it, the line you should see, and what that line does **not** prove. Every row
is host software only: no hardware, no QEMU, no customer validation, no
performance numbers. `scripts/prove_it.py` runs every runnable row and greps its
expected line; it is called from the `Locked host evidence` CI job (via
`scripts/collect_evidence.py`), so this page fails CI if it goes stale. Without
GNU make, `python3 scripts/prove_it.py` runs the same recipes.

Expected lines are regular expressions matched against the command's output.

| Claim | Command | Expected line | Does not prove |
|---|---|---|---|
| Every scoreboard number reproduces | `make verify` | `^\[verify\] refused=\d+ variants=\d+ named=\d+ allowlisted=\d+ redteam_sha256=[0-9a-f]{64} scoreboard_match=true$` | Anything QEMU-only or hardware; the counts are host clips and source scans |
| 99 named attacks are refused | `make red-team` | `^\[redteam\] attack=softnoi-is result=refused$` | Hardware isolation; Soft SMMU, SoftNoI and the fabric are software models |
| The refusals are a copyable catalogue | `make red-team-json` | `^\[red-team-json\] entries=99 matches_clip=true$` | That any third-party runtime passes these cases |
| Two tenant worlds do not diverge | `make noninterference` | `^\[noninterference\] worlds=2 seeds=512 ops=\d+ divergences=0$` | Timing, cache or power side channels; Kani proofs (`make kani`) are not run here |
| Isolation kit: Aether's models refuse all 15 classes, the weak sample fails | `make isolation-matrix` | `^\[isolation-kit\] backend=aether-soft classes=15 applicable=15 refused=15 accepted=0 n/a=0 result=conformant$` | Hardware isolation, certification, or any real runtime integrated |
| A leave-behind evidence bundle builds | `make evidence-pack` | `^\[evidence-pack\] sha=[0-9a-f]{40} files=\d+ not_proven=listed$` | Customer validation; the bundle is host clip output |
| Error-type coverage is counted, not asserted | `cargo test -p aether-core --test error_coverage -- --nocapture` | `^\[error-coverage\] variants=\d+ named=\d+ allowlisted=\d+$` | That allowlisted entries are reachable by a tenant (see `ALLOWLIST.md`) |
| blocked: eval-run (#189) | — (harness not on main) | — | Real-model workloads: nothing here replays a real model yet |

Not run by `prove_it.py`: QEMU boot targets (`make qemu-ci` and friends, run by
the `cargo test + make qemu` CI jobs) and Kani (`make kani`), which are too
slow for this check.
