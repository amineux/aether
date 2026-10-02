# Customer discovery worksheet

Copy this worksheet into a private opportunity record. It is a template;
there are no customer commitments implied by its presence in the repository.

## Buyer hypothesis

- Opportunity ID / owner / next-action date:
- Company segment and supported accelerator/runtime environment:
- User (runtime/systems engineer) and buyer (CTO/head of software):
- Hypothesis: safely sharing accelerator resources between workloads causes
  a costly, urgent problem that a bounded integration pilot can address.
- Alternative explanation and evidence that would disprove the hypothesis:

## Interview (understand the problem before showing the demo)

1. When did isolation or interference last cause a problem? What happened?
2. What workaround is deployed today, and who maintains it?
3. What did it cost in engineer hours, incidents, performance or delivery delay?
4. Which workload and environment could be shared for an evaluation?
5. What would prevent adoption of an external component?
6. Who controls budget, and what project deadline creates urgency?
7. What measured result would justify a technical evaluation or purchase?

Record date, participant role, permission to retain/share notes, exact problem,
current baseline, quantified cost, contrary evidence and the agreed next step.
Separate buyer statements from your interpretation. After discovery, show
`make diligence-demo` with its software-only limitations.

## Opportunity selection

Score each criterion 0 (unknown), 1 (stated), or 2 (supported by a concrete
incident, resource or dated commitment). Link the supporting interview record.

| Criterion | Score | Evidence |
| --- | --- | --- |
| Urgency: engineering allocation, budget or deadline | | |
| Access: workload, hardware/interface and named engineer | | |
| Feasibility: bounded proof within weeks | | |
| Repeatability: independent buyers need the same core | | |

A total score is a prioritization aid, not customer validation. Select one
problem only when the repeated-problem and follow-up gate in
[MATURITY.md](../MATURITY.md) is met. Record why other opportunities were paused.

## Private pipeline columns

Opportunity ID, segment, user role, buyer role, stage, problem, incident date,
baseline, urgency evidence, access evidence, budget status, next step, owner,
due date, evaluation record, commercial commitment, integration hours.
Stages: prospect → discovery → technical follow-up → scoped evaluation →
pilot proposed → paid pilot → repeat/closed. A stage advances only on evidence.
