# Contributing to Aether

## Coordinating people and coding agents

Use this workflow for Grok, Codex, and human contributions. GitHub is the shared
source of truth; agents do not automatically communicate with each other.

1. Fetch `origin/main` and read open PRs, their latest commits, checks, and review
   discussion before starting. Reuse work already in progress.
2. Use a separate branch and PR for each task. State the files and API boundaries
   the task changes in the PR description. If another PR owns the same work,
   review it or build on its branch by agreement rather than duplicate it.
3. Before pushing, fetch again and merge current `origin/main` into the task
   branch. Preserve both changes when resolving conflicts: demo functions,
   report fields, all-pass gates, emitted lines, Makefile greps, and docs must
   stay consistent. Never choose an entire side merely to remove conflict markers.
4. Push normally to your task branch. Do not force-push shared branches or push
   directly to `main`. If a normal push is rejected, fetch and reconcile the
   intervening commits first.
5. Review each other's diffs for correctness and regressions. Record concrete
   findings on the PR; resolve them with code or an evidence-backed explanation.
   Never imply an agent or person reviewed work unless they actually did.
6. Merge only after conflicts are resolved and CI passes for the latest PR
   revision against current `main`. Green checks from an earlier base do not
   validate a later conflict resolution.

## Validation and evidence

Run `cargo test --workspace --locked`, `make diligence-demo`, `make red-team`,
and `python3 scripts/collect_evidence.py --output /tmp/aether-evidence` for
security/demo changes. Kernel or driver changes also need the CI QEMU jobs for
x86_64, RISC-V, and AArch64. Retain the emitted refusal checks for every demo.
Report missing tools and checks honestly; do not describe a shim as an actual
`make` run. CI can supply the missing validation, tied to its exact commit.

Keep claims aligned with demonstrated evidence. Soft SMMU tests are software
checks, not hardware isolation or formal verification. Preserve existing
limitations and do not invent partner validation or commercial traction.
