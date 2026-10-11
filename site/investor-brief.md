# Aether — investor conversation brief

Updated 2 October 2026. Research-stage venture.
https://amineux.github.io/aether/

Formatted, shareable version: https://amineux.github.io/aether/investor-brief.html

## Thesis and initial buyer
Build a capability-based isolation layer for shared AI accelerators. Start with
small accelerator companies building NPU runtimes. Validate whether workload
isolation or interference creates an urgent, costly integration problem.

## Initial offer and potential business model
A bounded evaluation and integration pilot: one workload, one environment,
a baseline, agreed acceptance criteria and a fixed end date.
Potential progression: paid evaluation → reusable integration → annual software/support.
Pricing, demand and repeatability remain hypotheses. Public core: MIT OR Apache-2.0.

## Evidence today
Rust research kernel and software capability/tenant models. Host tests, named
refusal demos and QEMU checks for x86, RISC-V and ARM. Evidence collector records
commit, environment, commands and logs. CI and RustSec audit passed for PR #170.

- Implementation: https://github.com/amineux/aether
- Validated CI: https://github.com/amineux/aether/actions/runs/37059229950
- Dependency audit: https://github.com/amineux/aether/actions/runs/37059229955
- Evidence index: https://github.com/amineux/aether/blob/main/docs/business/INVESTOR_EVIDENCE.md

## Next 90 days
Weeks 1–3: target 10–15 discovery conversations from 30 companies.
Gate: three independent repeated problems and two technical follow-ups.
Weeks 4–6: customer-shaped proof. Gate: independent reproduction and buyer relevance.
Weeks 7–13: paid pilot and a second similar opportunity. Gate: payment and reuse evidence.
These are proposed milestones, not achieved traction.

Investment would fund customer validation, an agreed runtime/hardware integration
and independent reproduction. Amount and budget are not announced; define them
against the evaluation scope and required access.

## Existing mechanisms and the opportunity hypothesis
NVIDIA MIG provides hardware partitioning and isolation on supported GPUs:
https://docs.nvidia.com/datacenter/tesla/mig-user-guide/introduction.html
Linux SVA/PASID supports shared addressing and process-aware device transactions
on suitable platforms: https://docs.kernel.org/arch/x86/sva.html
Our inference is that isolation is a real systems concern. Aether must establish
an unmet NPU-team need and measurable value against an existing customer baseline.
It has not displaced these mechanisms or demonstrated performance superiority.

## Risks and diligence
No verified customers, revenue, hardware validation or production certification
are claimed. Soft SMMU is software; partner NPU driver is a no-op stub;
IREE/PJRT-shaped interfaces are research shims, not production plugins.
No formal verification or demonstrated commercial moat.
Issue #161 (unchecked raw unmap helpers) is fixed in the Soft SMMU: unmap now
requires a Memory+MAP capability and a foreign tenant's pin is refused with
`CrossTenant`. The bounded seeded fuzz now exercises these unmap paths. That is
bounded test evidence, not formal verification; Soft SMMU is software:
https://github.com/amineux/aether/issues/161

Founder-led by repository maintainer amineux: https://github.com/amineux
Start a conversation through the maintainer's GitHub profile. Keep proprietary
workloads and commercial information out of public issues.
