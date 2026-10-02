# Bounded evaluation and proof record

Status: unfilled template. Owner / opportunity ID / date:

Agree this record with the prospective customer's engineer before executing.
Use [DESIGN_WIN.md](../DESIGN_WIN.md) for the detailed interface worksheet;
its research stand-in is not customer evidence.

## Scope and acceptance

- One problem and workload:
- Environment: hardware model/revision, firmware, driver, runtime, OS:
- Boundary under test and trusted components:
- Existing approach used as baseline:
- Fixed start/end date and named reproduction engineer:
- Customer-provided workload, hardware access and engineering time:
- Explicitly excluded attacks, integrations and production claims:

| Criterion | Method and baseline | Threshold agreed before run | Actual result / artifact |
| --- | --- | --- | --- |
| Unauthorized access refusal | Cross-tenant and wrong-SID cases, plus valid control | | |
| Fault containment | Faulty workload while legitimate workload runs | | |
| Legitimate workload overhead | Identical workload/config with and without evaluated component | | |
| Recovery and integration effort | Recovery procedure; logged engineer hours | | |

For timing, record units, sample count, warm-up, repeats, variability and raw
data; use comparable configurations. Software memcpy or research opcodes must
not be labeled accelerator throughput. A software-only run cannot close a
hardware-isolation gate. Mark unsupported cases untested rather than passed.

## Reproduction bundle

Record commit SHA, dirty-tree status, toolchain, dependency lockfile,
commands, environment, stdout/stderr, exit codes, baseline configuration,
raw measurements and known failures. `python scripts/collect_evidence.py`
captures a host test/demo bundle; it does not run hardware or QEMU evaluation.
Run the existing `make qemu-*-ci` gates separately on a suitable Linux host.

Attach reproduction engineer/date/result and customer feedback confirming
whether the outcome addresses the stated problem. Record deviations and
retest decisions. Outcome: pass / fail / inconclusive, with supporting artifacts.
