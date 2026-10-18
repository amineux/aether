# Customer discovery: NPU runtime teams

Aether has **no customer contact yet** (see the Behind section of
[SCOREBOARD.md](../SCOREBOARD.md)). No code closes that gap, so this page is
the plan for the first conversations. It names no companies or people; add
prospects only in a private record, from a real source.

## What the harness offers an NPU runtime team today

Everything below runs on a laptop from `main` with host software models. It is
not hardware isolation, not a benchmark and not a partner result.

- **A conformance kit for their own backend.** `examples/isolation-kit`
  defines an `IsolationBackend` trait (`name`, `applies`, `probe`). A team
  implements it for their runtime and gets a per-class matrix over the 15
  attack classes (`refused` / `ACCEPTED` / `n/a`). The bundled weak sample
  scores `accepted=10`, so the kit can fail. Run: `make isolation-matrix`.
- **A named refusal catalogue.** `make red-team` prints 99 `result=refused`
  lines (98 named attacks plus one seeded tenant fuzz) against the software
  SMMU, NoI, fabric and capability model. Each line is a test case a team can
  copy into its own suite.
- **Determinism and noninterference checks.** Red-team and diligence output is
  byte-identical across runs; `make noninterference` replays 512 seeds of a
  hostile third tenant and checks that two honest tenants get byte-identical
  results whether or not it runs (7 of 7 deliberately broken variants
  caught).
- **A small frozen command interface.** `IreeHalCmd` v1 (96 bytes) and the
  `AccelDevice` trait (23 methods, 5 required) show what a runtime would have
  to provide; `make partner-hello` and `make design-win-standin` exercise them
  without QEMU.
- **One-command reproduction.** `make verify` re-runs the host tests, clips and
  error-coverage matrix and checks the numbers against SCOREBOARD.

What it does **not** offer: real models, measured interference on real
memory, a hardware SMMU, or any throughput number.

## Leave-behind after the conversation

`make evidence-pack` (or `python3 scripts/evidence_pack.py` without GNU make)
writes `build/evidence-pack/aether-evidence-<date>-<sha12>.tar.gz`. It holds the
red-team, noninterference, isolation-matrix and diligence-demo output from the
current commit, the commit SHA, the rustc/cargo/python versions, the "does not
offer" paragraph above as `NOT_PROVEN.md`, a short README saying how to re-run
each clip, and `SHA256SUMS`. It prints
`[evidence-pack] sha=<commit> files=9 not_proven=listed`. Only the file name
carries the date: two runs on the same commit and toolchain produce
byte-identical archives, so a team can rebuild it and compare hashes. Send it
after the interview, not before. It is host software evidence, not a customer
result, hardware data or a performance number.

## Kinds of teams to talk to

- Runtime or driver teams at small NPU and AI-accelerator start-ups that plan
  to run more than one workload or tenant per device.
- Platform teams putting an NPU into a shared edge or automotive SoC, where a
  faulty model must not corrupt a safety-relevant one.
- Teams porting an open compiler/runtime stack (for example an IREE HAL or
  TVM runtime backend) to new silicon, who own the command-submission path.
- Cloud or inference-serving teams that partition accelerators between
  customers and maintain their own isolation layer.
- Chiplet or IP vendors who must describe tenant isolation to their customers
  before silicon exists.

## Interview questions (problem first, demo last)

1. When did two workloads on the same accelerator last interfere or leak, and
   what did it cost to find and fix?
2. How do you isolate tenants or jobs today (hardware partitions, time
   slicing, one job per device, trust), and who maintains that code?
3. Which boundary worries you most: memory/DMA, command queues, shared caches
   and bandwidth, or firmware?
4. How would you show a customer or auditor today that one tenant cannot
   reach another's memory? What evidence do they ask for?
5. If a conformance kit ran your backend through named attack classes, what
   would it need to cover before you trusted the result?
6. What would stop you from adopting an external isolation component or test
   kit (licence, language, hardware coupling, review effort)?
7. Who owns the budget and the deadline for this, and what result would earn
   a two-week technical evaluation?

## What a yes and a kill look like

**Yes (advance to a scoped evaluation in [EVALUATION.md](EVALUATION.md)):**
the team names a concrete, dated isolation incident or customer requirement;
offers a workload, an interface or an engineer for a bounded trial; and agrees
on a measurable pass/fail before seeing results. One yes is an anecdote; the
gate in [MATURITY.md](../MATURITY.md) needs the same problem from independent
teams.

**Kill (record and stop pursuing this segment):** isolation is "handled by
the hardware" with no open issue; nobody owns the problem or the budget; the
only interest is in throughput numbers Aether does not have; or three
independent teams in a segment describe no recent incident. Write down why,
so the hypothesis can be retired honestly.

## Worksheet (copy per opportunity)

Copy this worksheet into a private opportunity record. It is a template;
there are no customer commitments implied by its presence in the repository.

### Buyer hypothesis

- Opportunity ID / owner / next-action date:
- Company segment and supported accelerator/runtime environment:
- User (runtime/systems engineer) and buyer (CTO/head of software):
- Hypothesis: safely sharing accelerator resources between workloads causes
  a costly, urgent problem that a bounded integration pilot can address.
- Alternative explanation and evidence that would disprove the hypothesis:

### Interview record

Use the seven questions above, problem first.

Record date, participant role, permission to retain/share notes, exact problem,
current baseline, quantified cost, contrary evidence and the agreed next step.
Separate buyer statements from your interpretation. After discovery, show
`make diligence-demo` with its software-only limitations.

### Opportunity selection

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

### Private pipeline columns

Opportunity ID, segment, user role, buyer role, stage, problem, incident date,
baseline, urgency evidence, access evidence, budget status, next step, owner,
due date, evaluation record, commercial commitment, integration hours.
Stages: prospect → discovery → technical follow-up → scoped evaluation →
pilot proposed → paid pilot → repeat/closed. A stage advances only on evidence.
