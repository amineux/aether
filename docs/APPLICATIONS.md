# Aether application map

Aether is a research prototype for capability-secured scheduling and memory
across heterogeneous accelerator tiles. Its useful question is not “which
models can it run?” but “which resource or data handoff needs an OS-enforced
owner, scope, deadline, or refusal?”

The examples below separate behavior exercised by the current host prototype
from product claims that still require partner hardware, drivers, and workload
measurements.

## Applications with an in-tree demonstration

| Application | Why Aether fits | What to run | Evidence and boundary |
| --- | --- | --- | --- |
| **Disaggregated LLM serving** — prefill and decode on separate accelerators, with tenant-specific KV windows | Pass a narrow, revocable capability for one sequence’s KV range instead of granting the consumer broad access to memory or weights. | `make kv-fabric` | The host demo checks read-only scope, tenant/SID refusal, bounds, revoke, and deadline isolation. Byte counts are toy values; it does not move a real remote KV tensor or measure tokens/s or TTFT. |
| **Multi-tenant accelerator inference** — share a package across model-serving tenants | Bind jobs and memory to tenant capabilities, bank colors, spatial slices, and stream IDs; reject cross-tenant mappings and unauthorized placement. | `make diligence-demo` and `make red-team` | Exercises host-side refuse paths and software models. This is not hardware isolation, a confidential GPU, or a production MIG replacement. |
| **Compiler/runtime integration for a custom NPU** — connect an existing compiler stack to a package-specific command processor | Keep compiler-facing device, memory, executable, buffer, and event concepts above a frozen packet ABI, then let a silicon partner supply its command mapping. | `make partner-hello`; `make design-win-standin` | The consumer and IREE HAL-shaped stand-in run against the software command processor. They are not an upstream PJRT plugin, an IREE driver, a vendor backend, or a signed partner integration. |
| **Workload admission on a shared fabric** — decide whether another tenant’s workload should enter a contended package | Use software interference and fabric-class budgets to admit or refuse work before contention crosses configured limits. | `make red-team` | Demonstrates admission decisions with modeled integer rates and classes. It is not a topology optimizer, performance counter, or measured throughput controller. |

## Promising next applications

These are engineering directions suggested by the existing mechanisms, not
features the repository currently delivers end to end.

| Direction | Aether mechanisms to extend | Evidence needed before calling it a product |
| --- | --- | --- |
| **Mixed-criticality edge inference** for robotics or industrial vision: keep a safety-relevant perception job isolated from best-effort language or analytics work | Deadline-aware tile scheduling, bank affinity, spatial partitioning, and explicit refusal when a cut or budget cannot be honored | A real accelerator driver, a defined deadline/SLO contract, repeatable latency-under-contention measurements, and failure-injection on target hardware |
| **Private model serving** for multiple customers on one accelerator package | Tenant-tagged model/KV ownership, capability revocation, per-stream mappings, and red-team cross-tenant checks | Hardware-enforced DMA isolation, key and reset lifecycle, side-channel assessment, and independently reviewed threat model; software SMMU is not a security boundary against a device that ignores it |
| **Composable inference appliances** mixing CPU, NPU, GPU, and domain-specific ASIC tiles | Frozen command packet, typed memory places, fabric routes, fences, and partner-supplied opcode mappings | One actual device implementation, conformance tests against the packet contract, recovery behavior, and end-to-end application benchmarks |
| **Disaggregated model pipelines** that move activations or other intermediate state between chiplets | Narrow, sequence-scoped capability grants and ownership transfer patterned on the KV example | Extend the prototype beyond its KV-specific model, validate coherency and transport behavior on silicon, and measure the cost of remote data movement |

## Suggested first partner evaluation

For a team with a custom inference accelerator, start with one serving workload
and one concrete boundary: a tenant’s KV range, a DMA stream, or a deadline
class. Run the host clips, map the partner’s command and stream-ID semantics to
the frozen packet, then repeat the same checks on the device. The first success
criterion is a correctly refused unauthorized handoff; performance claims come
only after hardware measurements.

See [`SELL_PACK.md`](SELL_PACK.md), [`DILIGENCE.md`](DILIGENCE.md), and
[`DESIGN_WIN.md`](DESIGN_WIN.md) for the current demo and partner intake.
