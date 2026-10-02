# Maturity and 90-day execution plan

Assessment date: 2026-10-02. Owner: project maintainer. Start the 90-day
clock when discovery begins; these are decision gates, not delivered results.

Aether is a research prototype with reproducible software isolation demos.
Hardware isolation, a production runtime backend, customer demand, and revenue
are not established by this repository. Historical roadmap entries marked
landed describe research implementations, not commercial readiness.

## Current evidence and gaps

| Area | Available evidence | Gate still open |
| --- | --- | --- |
| Capability model | `core/src/caps.rs`, `core/src/caps_props.rs`, host tests | Independent review; no formal verification claim |
| Isolation demonstrations | Diligence and red-team clips; `docs/SECURITY.md` | Fault containment on an agreed customer environment |
| Accelerator integration | SoftNPU, Soft SMMU, IREE/PJRT-shaped host interfaces | Real driver/runtime integration; partner driver is a stub |
| Reproducibility | Cargo workspace, golden demo checks, QEMU CI | Independent reproduction with dated logs and commit SHA |
| Buyer demand | Discovery and design-win templates | Repeated urgent problems from independent buyers |
| Commercial readiness | Proposed bounded pilot offer | Signed scope, budget owner, payment and acceptance evidence |

## Execution gates

| Window | Work | Exit evidence | If the gate fails |
| --- | --- | --- | --- |
| Days 1–3 | Test small NPU/accelerator runtime teams as the initial buyer; name user, budget owner, problem and paid value | One written buyer hypothesis; first ten prospects | Narrow the problem before adding features |
| Weeks 1–3 | Build 30-company prospect list; aim for 10–15 discovery conversations | Three independent teams describe the same consequential problem; two technical follow-ups | Change buyer/problem; do not count compliments as demand |
| End of week 3 | Score urgency, access, feasibility and repeatability | One selected problem, with interview references and a rejected-alternatives note | Continue discovery if evidence is ambiguous |
| Weeks 4–6 | One workload, environment and baseline; agree success criteria before running proof | Another engineer reproduces results; prospective customer confirms relevance | Record failures and missing access; reduce scope |
| Weeks 7–10 | Offer a bounded paid integration pilot | Signed scope, named engineers, acceptance tests, fee and payment milestones | Record purchasing blocker and dated next commitment |
| Weeks 10–13 | Offer the same core to a second buyer | Reuse assessment, integration hours and a second qualified opportunity | Decide explicitly between services and a repeatable product |

Use [DISCOVERY.md](business/DISCOVERY.md), [EVALUATION.md](business/EVALUATION.md)
and [PILOT.md](business/PILOT.md) for each opportunity. Keep customer names,
contact details, interview notes, pricing and agreements in a private workspace.
Only publish permissioned, redacted evidence.

## Weekly operating review

Track qualified conversations, independent repeated problems, technical
follow-ups, evaluations with written criteria, commitments, payments,
integration hours, and measured improvement against a baseline. Start counts
at zero until backed by records. Assign an owner and next-action date to each
opportunity. For a three-hour session, reserve roughly 60 minutes for customer
work, 90 for evaluation-linked engineering and 30 for evidence and planning.

Every engineering task must cite a customer requirement or a measured technical
risk. Prioritize reproducibility and boundary failures before new architectures.
Choose real hardware/runtime work only after securing access and acceptance
criteria; avoid promising integrations based on the no-op partner driver.

At day 90: a paid pilot plus a second similar opportunity supports continued
product investment. Strong technical engagement with slow procurement needs
dated commitments. Interest without access, urgency or budget calls for a new
buyer/problem. Unrelated custom work calls for a deliberate services decision.

## Investor conversations

Use [INVESTOR_EVIDENCE.md](business/INVESTOR_EVIDENCE.md) as the evidence index.
Early conversations can describe the thesis and open risks. A fundraising
milestone must name the evidence the money will buy, its cost and its owner.
Do not describe a worksheet, stand-in, software benchmark or letter of interest
as a paying customer, hardware validation or production certification.
