# Aether: five-minute investor demo

This is a research-stage pitch backed by a reproducible software evaluation.
It is not a product launch or an assertion that customers have validated demand.

## 0:00 — The problem and buyer

“Accelerator runtime teams need to decide which workloads can share a device,
what memory each tenant can reach, and how failures are contained. Our initial
buyer hypothesis is small NPU companies that need those boundaries without
building every isolation mechanism themselves.”

Ask whether the team has seen this problem in a real deployment. Do not imply
that an interview, design partner or incident exists without a source.

## 0:45 — Show the product hypothesis

“Aether is a capability-based isolation prototype. Today it lets a runtime
engineer exercise an explicit command interface and inspect allowed and refused
operations. The intended commercial offer is an integration evaluation around
one customer workload and environment.”

Open the generated `investor-report.html`. Show the source commit, capture time,
check results and logs. Generate it using the README commands; distribute the
entire evidence directory so links work. It contains no external assets.

## 1:30 — Demonstrate the boundary

Run `make diligence-demo`, then `make red-team`. Show a permitted submission
and completion in the first clip, then a named refused cross-tenant operation
in the second. Explain the observed operation and error, not the number of tests.
The report runs the Makefile red-team golden checks and preserves their exit
status. The command packet is frozen; no vendor integration is implied.

These are deterministic software-model demonstrations. They do not measure
accelerator performance or prove isolation on silicon. Review the security
issues for the exact source revision before describing its security posture.
As of 5 October 2026, PR #183 remains a separate proposed unmap security fix;
this evaluation work does not merge it or represent it as landed on main.

## 3:00 — Explain the commercial test

Propose one bounded evaluation with a runtime team: name the workload,
environment, budget owner, technical owner, baseline, acceptance criteria,
deadline and access requirements. Agree those before implementation.
An example criterion to negotiate is that a prohibited cross-tenant access is
refused while the authorized workload still completes; latency and integration
cost must be measured in that environment rather than inferred from host demos.

Use the existing [evaluation](../business/EVALUATION.md),
[pilot](../business/PILOT.md) and [discovery](../business/DISCOVERY.md) records.
The [DESIGN_WIN worksheet](../DESIGN_WIN.md) captures technical requirements;
a completed worksheet is not a signed design win. Keep commercial records private.

## 4:00 — State the funding milestones

“Investment would fund customer validation, one agreed runtime/hardware
integration, independent reproduction and a paid pilot. We then test reuse
with a second buyer before expanding the platform.”

Set the amount and runway from an actual staffing and integration budget.
Do not quote invented revenue, market size, savings, customer counts or a moat.
The open business gates remain visible in the report even when all host checks pass.

## Focus and cleanup decisions

- The README now provides one buyer, one demo path and one next commercial step.
- Overlapping research catalogs and historical roadmaps are folded into the
  technical reference; they no longer lead the investor entry point.
- No runtime module is deleted merely because it is experimental: dependencies
  and security coverage need an explicit deprecation decision first.
- More toy opcodes, additional architecture breadth and speculative scheduling
  features are deferred from the initial evaluation unless tied to a buyer
  requirement or measured risk.
- Keep the existing customer-validation work and pending security/API work
  separate; this branch changes evidence tooling and presentation only.

## Before sharing

Use a clean committed revision, collect fresh evidence, and resolve any report
warning. Inspect the matching GitHub checks and security discussion. Share a
passing report as host evidence, alongside the remaining customer and hardware
gates. Checksums detect accidental file edits; they do not provide cryptographic
attestation that the author executed the commands.
