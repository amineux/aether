# Buyer validation — accelerator-runtime teams

Status: **unvalidated hypothesis**. This process records evidence; it does not
record a partner, revenue, or a design win. No buyer responses are supplied here.
“Isolation is the product” in the sell pack is the value hypothesis to test.

Use [SELL_PACK.md](SELL_PACK.md) for honest demo claims and
[DESIGN_WIN.md](DESIGN_WIN.md) for the technical mapping. Copy the
[conversation record](buyer-validation/conversation-template.md) per independent
buyer team. Keep private specs, contact details, prices, and correspondence in a
buyer-approved private location; reference them by restricted evidence ID here.
Do not commit confidential opcode tables to this public repository.

## Call questions (ask before prescribing a port)

Start with questions 1–5, show only the demo relevant to their answer, then
complete the technical mapping and commercial follow-up. A short first call may
leave fields unknown; unknown is never yes. Offer a deeper session with their
runtime and device-security owners rather than rushing through the worksheet.

| # | Exact question | Evidence to request |
| --- | --- | --- |
| 1 | Which accelerator, runtime version, workload, and deployment are you responsible for, and who owns the runtime and device-security decisions? | Named accountable roles, target revision, deployment diagram or reviewed summary. |
| 2 | Describe the last cross-tenant fault, unsafe command, or isolation limitation you encountered. What happened, how often, and who was affected? | Dated incident, reproduction, or owner-confirmed limitation; distinguish incidents from hypothetical risk. |
| 3 | What does that problem cost in engineering time, delayed deployment, failures, or lost customer requirements? What is the current baseline? | Measured value and units, or explicitly labeled estimate with source and confidence. |
| 4 | How do you solve it today, including SDK, Linux/IOMMU, virtualization, or internal tooling, and why is that insufficient? | Current architecture and specific unmet requirement; “current solution is enough” is valid evidence. |
| 5 | What must improve, by how much, on what target and workload, for you to adopt anything new? Which regression would make you reject it? | Buyer-approved acceptance metrics, thresholds, test method, deadline, and rejection criteria. |
| 6 | Can your command-processor owner share or review an authoritative opcode/descriptor table? Who owns it, what revision is it, and what are we allowed to retain? | Authorized table, redacted excerpt, or owner-reviewed mapping with provenance; public IREE nouns do not qualify. |
| 7 | Which opcode names, operand layouts, dtypes, executable handles, and transfer/dispatch semantics must work first? Where does research v1 fail to represent them? | DESIGN_WIN sections 1–2 plus explicit unsupported requirements; do not force their names into research values. |
| 8 | What are your real SID/PASID/STE/CD budgets and lifetimes per tenant? What hardware prevents a device from DMAing outside its domain? | DESIGN_WIN section 3 and hardware enforcement/reset/revocation description reviewed by the security owner. |
| 9 | Which memory spaces, capacities, address widths, coherence rules, DMA mappings, and buffer lifetimes must the workload use? | DESIGN_WIN section 4 plus required semantics and limits. |
| 10 | How many queues are required, with what ordering, submission/completion model, concurrency, and backpressure? | DESIGN_WIN section 5 and queue contract; research v1 remains one mailbox. |
| 11 | What are the event/fence visibility scopes, ordering guarantees, timeout, cancellation, and device-reset behavior? | DESIGN_WIN section 6 plus buyer-required failure semantics. |
| 12 | Which host OS, SDK, compiler/runtime API, hardware access, licenses, and support obligations would an evaluation require? | Integration boundary, dependencies, available test environment, and blocking access constraints. |
| 13 | Who can sponsor an evaluation, who controls budget, and how is a purchase approved? Is funding allocated, exploratory, or unavailable? | Sponsor and budget-owner roles; funding status, amount/range/currency if disclosed, procurement steps and target date. |
| 14 | Would you evaluate a reusable runtime/isolation component, or pay for a scoped integration/security/bring-up service? What deliverable and commercial terms would justify payment? | Buyer-stated preference, scope, willingness-to-pay evidence and conditions; enthusiasm alone is insufficient. |
| 15 | What will each side deliver next, by which date, and who accepts the result? If you will not proceed, can you confirm the reason in writing? | Buyer-confirmed next action or attributed written no, with scope and reopen condition. |

## What counts as a qualified design conversation

All five gates must have dated, attributable evidence. Missing evidence means
**discovery**, even if the demo runs or a worksheet is filled. Qualification is
an internal assessment, separate from a signed contract or design win.

1. **Accountability:** a real accelerator-runtime team and an identified runtime
   decision owner participated or reviewed the record; authority is stated.
2. **Pain:** a concrete current workload and unmet problem, baseline/impact,
   current alternative, and buyer-owned success/rejection criteria are recorded.
3. **Technical substance:** an authorized real table/excerpt or CP-owner-reviewed
   mapping covers opcodes, SID budget, memory spaces, queues, and event/fence
   scope. Every field is answered or an explicit gap has an owner and due date.
   A blocker may qualify the conversation but cannot pass a product pilot gate.
4. **Adoption path:** an evaluation sponsor, budget decision path, integration
   boundary, target environment, and adoption blockers are recorded. Budget may
   be unknown for qualification if a named owner agrees to resolve it by a date;
   a funded pilot requires confirmed funding, not this exception.
5. **Reciprocity:** the buyer confirms the reviewed summary and a dated next
   action (table review, test access, acceptance review, procurement), or gives
   an attributable written no. An outbound email or an unaccepted calendar
   proposal is not buyer commitment.

A **qualified rejection** passes the evidence gates and is explicitly a no;
it is learning, not an active opportunity. A brief written no without those
gates is still useful rejection evidence, but not a qualified conversation.
Silence, GitHub stars, public stand-ins, fixture passes, NDA signatures, and
friendly introductions are not qualification or purchase evidence.

## Map claims to buyer tests

| Sell-pack claim / ask | What today proves | What the buyer must validate |
| --- | --- | --- |
| Isolation / blast radius | Software two-tenant refuse demos (`make red-team`). | Their threat model and target hardware enforcement; real DMA can bypass Soft SMMU. |
| Frozen packet / opcode table | Research 96-byte IreeHalCmd, supported toy operations, refuse checks. | Authoritative descriptors and semantics fit, or a documented gap worth funding. |
| Events / fence counts | Software timeline behavior and counts, not latency. | Required ordering, reset behavior, visibility and overhead on their target. |
| Optional path-A IOVA | Host software IOVA/wrong-SID proof (`make accel-test`). | Actual integration feasibility and device address-domain enforcement. |
| Table or written no | A useful technical discovery outcome. | Decision authority, costly pain, adoption path, and reciprocal commitment. |

Do not infer production safety, performance, SDK interoperability, or a PJRT
plugin from these demos. Preserve every non-claim in the sell pack.

## Minimum decision record

The conversation template is the minimum record. Each decision-relevant answer
needs a source/date, confirmation status, and either a value or **unknown** with
an owner/due date. Separate buyer quotes, our interpretation, and our proposal.
The technical TOML fixture remains separate: it checks research packet validity,
not qualification, willingness to pay, or hardware compatibility.

## Product versus services decision gates

These are proposed operating rules for this validation cycle, not observed
traction or a buyer commitment. Start a **six-week** cycle when outreach starts;
record the start/review dates and target **five independent runtime teams**.
Log every attempted contact and response, including silence, so the sample is
visible. Independence means separate purchasing decisions, not five engineers
at one company. Missing access or too few replies means insufficient evidence,
not a market-wide rejection. The six-week review is earlier commercial triage;
it does not replace the existing 2028 signed-list-or-ABI-freeze horizon.

| Decision | Minimum evidence at review | Authorized next scope |
| --- | --- | --- |
| Continue toward product evaluation | At least two independent qualified positive conversations share the same costly problem and reusable integration boundary; at least one buyer confirms a funded evaluation path, sponsor, target access, acceptance criteria and dated next action; no unresolved safety or integration blocker prevents that evaluation. | Scope one bounded pilot. A funded path is not a signed order; secure terms before promising paid delivery. |
| Pivot toward services | Product gate fails; at least one qualified buyer confirms budget and a purchase path for a specific integration, isolation assessment, or bring-up deliverable with acceptance criteria and access; needs are bespoke or reusable component demand is unconfirmed. | Propose a scoped paid service with explicit deliverables, exclusions and ownership; keep reusable research frozen. |
| Continue discovery, time-boxed | Neither commercial gate passes, but evidence gaps have buyer-owned dated actions or access has limited the sample. | Choose one additional cycle with explicit gap owners and review date; no speculative packet/port work. |
| Pause product investment | Neither gate passes and there are no credible dated evidence actions after the review/extension, or reviewed requirements cannot safely be met within an affordable scope. | Record reasons and reopen conditions; preserve the research prototype without marketing it as validated demand. |

At review, record independent team counts, qualified positive/rejection counts,
shared requirements, funded paths, contrary evidence, delivery-cost estimate,
chosen decision, owner, review date and reopen conditions. Product and services
can both be supported: select one primary allocation and explain the tradeoff.
No outcome is selected until real records exist. Do not turn a written no from
one team into a conclusion about the entire market.

## Freeze and follow-up

A buyer artifact never changes packet offsets. Before proposing v2, reference
the authorized real table, buyer requirement, incompatibility, acceptance test,
and commercial priority. Any actual packet change must update
[ACCEL.md](ACCEL.md), `drivers/src/ireecp.rs`, and host pack/unpack together in
the same PR, with compatibility/refuse tests. A port remains gated on a real
partner ask that path B cannot satisfy. Do not invent an ISA to fill gaps.

Within two working days of a call, prepare the evidence summary for buyer
review, label unconfirmed statements, and record the next action or written no.
Contacting anyone is a separate action requiring authorization; this artifact
itself sends no messages and supplies no buyer answers.
