# Investor evidence index

As of 2026-10-05: research prototype. This index establishes no verified
customers, revenue, hardware validation or independent security certification.
Replace unknowns only with dated, attributable evidence.

| Claim | Evidence to attach | Current status |
| --- | --- | --- |
| Software model enforces named boundaries | SHA + host tests + diligence/red-team logs | Implemented; fresh run required for each shared bundle |
| A partner can reproduce a host evaluation locally | `make eval-run` JSON (commit, toolchain, Cargo.lock hash, SoftGreenCtx milli bandwidth, named refusals) | Software model only. No customer, no hardware, no advantage versus MIG. Field map: [EVAL_RUN.md](EVAL_RUN.md) |
| QEMU subsets boot | SHA + CI run + architecture logs | Existing CI gates; not silicon validation |
| Buyer has urgent problem | Redacted independent interviews, incidents and cost | Unverified |
| Customer can integrate | Agreed environment + reproduction + engineer feedback | Unverified |
| Customer will pay | Signed bounded scope and payment evidence | Unverified |
| Core is reusable | Second similar opportunity + integration-hours comparison | Unverified |
| Hardware boundary is effective | Real environment, threat model and fault tests | Unverified |

## Business narrative to fill from discovery

Buyer / problem / baseline / quantified advantage / what is purchased / why
now / repeatable core / acquisition path / main alternative / adoption blocker.
Distinguish measured advantage from expected advantage.

## Funding milestone to fill

Amount and runway assumptions; named milestone; owner; hardware and integration
access required; engineering/customer work budget; acceptance evidence; dates;
failure decision. Keep forecasts distinct from bookings and collected revenue.

## Diligence bundle

Include [MATURITY.md](../MATURITY.md), [SECURITY.md](../SECURITY.md), a fresh
host evidence bundle, relevant QEMU CI links, evaluation records, private
permissioned commercial evidence and an open-risk register with owners.
Logs prove only the command and environment recorded. Failed runs stay visible.
No repository artifact substitutes for external customer or hardware evidence.

## Evidence freshness

Every commit SHA cited in this folder or in
[`docs/SCOREBOARD.md`](../SCOREBOARD.md) is checked by
`python3 scripts/check_evidence_freshness.py`. CI runs it as the
`evidence-freshness` check of `scripts/collect_evidence.py` in the
host-evidence job (no cargo, a few seconds), so a stale SHA fails the evidence
bundle. The rule:

- The evidence commit is the `commit_sha` that
  `scripts/collect_evidence.py` writes into `manifest.json`
  (`--manifest <bundle>/manifest.json`), or HEAD in CI, which is the commit the
  host-evidence job measures in the same run.
- A cited SHA must exist and be on that commit's history. A typo, a
  branch-only SHA or a rebased-away SHA fails.
- Allowed lag: a cited SHA may be at most **20 first-parent commits** (merged
  PRs on `main`) behind the evidence commit. A docs-only merge moves the count
  by one and cannot fail by itself; past 20, re-measure and refresh the SHA.
- A GitHub Actions run link must sit on a line that also cites its commit SHA.
- A line marked `evidence-lag: historical` cites history, not the measured
  state, and is exempt.
