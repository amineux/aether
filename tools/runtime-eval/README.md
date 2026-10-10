# Aether external-runtime evaluation toolkit

Run **real IREE-compiled CPU workloads** alone and together, check numerical
results and input rejection, and keep the exact binaries/inputs for replay.
This is Aether's first external-runtime adapter. It uses IREE's Python APIs;
it is not a new IREE HAL driver and does not route work through Aether's kernel.
The existing Rust `make eval-run` work evaluates Aether's software model and
remains separate. No `IreeHalCmd` packet or kernel ABI change is required.

## First run (Linux x86_64, Python 3.12+)

From the repository root:

```bash
python3 -m venv /tmp/aether-eval-venv
. /tmp/aether-eval-venv/bin/activate
python -m pip install -c tools/runtime-eval/constraints.txt './tools/runtime-eval[iree]'
aether-eval doctor
aether-eval init --output /tmp/aether-scenario.json
aether-eval run /tmp/aether-scenario.json --output /tmp/aether-first-run
# Open /tmp/aether-first-run/report.html in your browser.
aether-eval replay /tmp/aether-first-run --output /tmp/aether-replayed
```

Use new output paths on subsequent runs; existing results are never overwritten.
Installation downloads the real IREE compiler/runtime and NumPy. Kernel, Rust,
QEMU, GPU access and vendor credentials are not required. Other hosts are not
validated yet. The toolkit is installable from source; it is not published to PyPI.
Without the `[iree]` extra, `init`, scenario validation and report code remain
available, and `doctor` explains the missing adapter dependencies.

## Application example

Two generated workloads represent parts of an edge application:

- **Image preprocessing:** convert a grayscale frame's float32 pixel values
  from 0–255 into normalized 0–1 values.
- **Feature projection:** multiply a batch of feature vectors by a projection
  matrix while the foreground preprocessing runs.

These are real computations, not recorded timing fixtures. Inputs are synthetic;
there is no camera, pretrained image classifier, embedding model or customer
workload in this example. Add a partner's real workload before drawing application
performance conclusions. The adapter boundary lives in `iree_cpu.py`; a new
backend must declare and test its supported checks rather than inherit passes.

## Configure the experiment

Edit the JSON created by `init`:

| Field | Meaning |
| --- | --- |
| `seed` | Seed for generated input data and phase ordering |
| `samples` | Measured invocations per workload, phase and round (3–1000) |
| `rounds` | Repeated rounds with randomized solo/shared phase order (1–20) |
| `warmup` | Unmeasured calls before each batch (0–100) |
| `timeout_seconds` | Whole worker deadline, including compilation (1–3600) |
| `workloads[].shape` | `[height,width]` for normalize; `[m,k,n]` for matmul (2–1024 per dimension) |
| `workloads[].deadline_ms` | Optional threshold for reporting deadline misses; descriptive, not a pass/fail gate |
| `max_p95_slowdown` | Optional CI gate on shared p95 / solo p95 for both workloads |

Default: 10 samples × 3 rounds × 2 phases × 2 workloads = 120 measured calls.
No performance gate is enabled by default. Adding `"max_p95_slowdown": 2.0`
requires both observed ratios to be at most 2.0. Set thresholds from repeated
measurements on a controlled runner; shared CI timing is noisy.

Unknown fields, duplicate keys, unsupported operations, unbounded sizes and
non-finite numeric values are rejected. JSON cannot specify shell commands.

## What is measured

One IREE `local-task` device is shared by two separate runtime contexts in one
process. Solo batches invoke one workload at a time; shared batches use two host
threads synchronized before each measured request. This evaluates concurrent
submissions, not isolation between processes or proof of simultaneous execution.

The monotonic clock spans invocation through output materialization on the host.
It includes input/output transfers and Python/runtime call overhead. Compilation,
loading, warmup and NumPy correctness checking are excluded. Nearest-rank p95,
mean, minimum, maximum, counts and per-round ratios are recorded with all raw
samples. CPU resources are not reserved; power state and other host workloads
can affect the result. It is not a hardware throughput benchmark.

Every measured output is compared with a NumPy reference (rtol/atol 1e-4).
Each workload also checks malformed-shape rejection and a valid invocation after
that refusal. Only the expected IREE invalid-argument shape error counts as a
successful refusal; an arbitrary crash does not.

## Evidence and replay

Each directory contains scenario, result/environment, raw samples, worker logs,
MLIR sources, compiled `.vmfb` modules, input/reference `.npz` arrays and their
SHA-256 manifest. Replay checks hashes, copies the artifacts, and executes the
same compiled modules and arrays without recompilation. It makes new timing
measurements; exact latency reproduction is not promised. The saved code still
requires a compatible CPU platform and IREE version. Package versions and source
commit/changes are recorded when the installed package resides in a checkout.

Replay **only trusted bundles**: VMFBs contain executable code. Checksums detect
accidental alteration; they do not authenticate an author or certify safety.
Hashing does not prove a test ran. Commercial data should not be committed.

A timeout kills the worker process group on Linux and preserves its diagnostics
and completed measurement batches. Failures retain a non-passing JSON/HTML report.
Exit codes: 0 = checks and requested gates passed; 1 = worker/check/gate failure;
2 = setup, scenario or artifact error. A process interrupted by the user retains
its initial non-passing report.

**Hardware tenant isolation and Aether kernel policy enforcement are unsupported
on this adapter**, always labeled as such. This toolkit supplies no claim of
production readiness, a security certification, or superiority to other runtimes.

## Tests

```bash
AETHER_IREE_TEST=1 python -m unittest discover -s tools/runtime-eval/tests -v
```

The integration test actually compiles, runs, checks, replays and exercises a
failing latency gate. CI installs the adapter and always enables this test.
Unit tests also cover malformed scenarios, tampered artifacts, missing samples,
timeouts and output preservation. No GPU hardware is needed.

API references: [IREE Python bindings](https://iree.dev/reference/bindings/python/)
and [CPU deployment](https://iree.dev/guides/deployment-configurations/cpu/).
