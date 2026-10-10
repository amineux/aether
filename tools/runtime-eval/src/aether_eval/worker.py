"""Bounded child process: compile/load, verify, measure and checkpoint."""
import concurrent.futures
import math
import pathlib
import random
import sys
import threading
import time

from .common import digest, environment, write_json
from .scenario import read_json, validate

LIMITS = [
    'CPU workloads through the real IREE runtime; no GPU/NPU hardware measurements.',
    'Two runtime contexts share one CPU device in one process; no tenant security boundary.',
    'Aether kernel and IreeHalCmd are not on this execution path.',
    'Generated image preprocessing and matrix projection inputs; no pretrained camera or embedding model.',
    'Host invocation latency includes transfers and output materialization; excludes compilation and correctness checks.',
    'Concurrent host requests do not prove simultaneous device execution. Timing varies with host load.',
]


def stats(values):
    if not values or any(not math.isfinite(x) or x <= 0 for x in values):
        raise ValueError('Latency samples must be positive finite values')
    ordered = sorted(values)
    return {'count': len(values), 'min_ms': ordered[0], 'max_ms': ordered[-1],
            'mean_ms': sum(values) / len(values), 'p95_ms': ordered[math.ceil(.95 * len(values))-1]}


def summarize(scenario, samples):
    summary = {}
    for job in scenario['workloads']:
        name = job['name']
        phases = {p: [s['latency_ms'] for s in samples if s['workload'] == name and s['phase'] == p]
                  for p in ('solo', 'shared')}
        expected = scenario['samples'] * scenario['rounds']
        if any(len(v) != expected for v in phases.values()):
            raise ValueError(f'Missing measurement samples for {name}')
        values = {p: stats(v) for p, v in phases.items()}
        values['p95_slowdown'] = values['shared']['p95_ms'] / values['solo']['p95_ms']
        values['per_round_p95_slowdown'] = [
            stats([s['latency_ms'] for s in samples if s['workload'] == name and s['phase'] == 'shared' and s['round'] == r])['p95_ms'] /
            stats([s['latency_ms'] for s in samples if s['workload'] == name and s['phase'] == 'solo' and s['round'] == r])['p95_ms']
            for r in range(scenario['rounds'])]
        if 'deadline_ms' in job:
            values['deadline_ms'] = job['deadline_ms']
            values['deadline_misses'] = {p: sum(v > job['deadline_ms'] for v in vals) for p, vals in phases.items()}
        threshold = scenario.get('max_p95_slowdown')
        values['latency_gate'] = {'status': 'not_requested' if threshold is None else
                                  ('passed' if values['p95_slowdown'] <= threshold else 'failed'),
                                  'max_p95_slowdown': threshold}
        summary[name] = values
    return summary


def execute(output, replay=False):
    from .iree_cpu import Session, make_config, prepare
    scenario = validate(read_json(output / 'scenario.json'))
    jobs = scenario['workloads']
    artifacts = output / 'artifacts'
    artifacts.mkdir(exist_ok=True)
    if not replay:
        for i, job in enumerate(jobs):
            prepare(job, scenario['seed'] + i, artifacts)
    checksums = {'scenario.json': digest(output / 'scenario.json')}
    for job in jobs:
        for ext in ('mlir', 'vmfb', 'npz'):
            path = artifacts / f'{job["name"]}.{ext}'
            checksums[path.relative_to(output).as_posix()] = digest(path)
    write_json(output / 'artifacts.json', {'schema': 'aether-artifacts/v1', 'sha256': checksums})
    config = make_config()
    sessions = [Session(job, artifacts, config) for job in jobs]
    checks = []
    for job, session in zip(jobs, sessions):
        checks.append({'workload': job['name'], 'check': 'numerical_correctness', 'passed': session.correct(session.invoke())})
        refusal = session.reject_bad_shape()
        checks.append({'workload': job['name'], 'check': 'invalid_shape_rejected', **refusal})
        checks.append({'workload': job['name'], 'check': 'valid_request_after_rejection', 'passed': session.correct(session.invoke())})
    samples = []
    def batch(index, phase, round_number, barrier=None):
        session, job = sessions[index], jobs[index]
        for _ in range(scenario['warmup']):
            if not session.correct(session.invoke()):
                raise ValueError(f'Warmup correctness failed: {job["name"]}')
        local = []
        for sample in range(scenario['samples']):
            if barrier:
                barrier.wait(timeout=min(30, scenario['timeout_seconds']))
            start = time.perf_counter_ns()
            result = session.invoke()
            elapsed = (time.perf_counter_ns() - start) / 1e6
            if not session.correct(result):
                raise ValueError(f'Measured correctness failed: {job["name"]}')
            local.append({'workload': job['name'], 'phase': phase, 'round': round_number,
                          'sample': sample, 'latency_ms': elapsed})
        return local
    rng = random.Random(scenario['seed'])
    orders = []
    for round_number in range(scenario['rounds']):
        phases = ['solo', 'shared']
        rng.shuffle(phases)
        orders.append(phases)
        for phase in phases:
            if phase == 'solo':
                indexes = [0, 1]
                rng.shuffle(indexes)
                for index in indexes:
                    samples.extend(batch(index, phase, round_number))
            else:
                barrier = threading.Barrier(2)
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                    futures = [pool.submit(batch, i, phase, round_number, barrier) for i in (0, 1)]
                    for future in futures:
                        samples.extend(future.result())
            write_json(output / 'samples.json', samples)
    summary = summarize(scenario, samples)
    passed = all(c['passed'] for c in checks) and all(v['latency_gate']['status'] != 'failed' for v in summary.values())
    result = {'schema': 'aether-runtime-eval/v1', 'complete': True, 'passed': passed,
              'adapter': 'iree-cpu', 'environment': environment(), 'phase_order': orders,
              'checks': checks, 'summary': summary, 'limitations': LIMITS,
              'hardware_isolation': {'status': 'unsupported'},
              'kernel_policy_enforcement': {'status': 'unsupported'},
              'replay': replay, 'sample_file': 'samples.json', 'percentile_method': 'nearest rank'}
    write_json(output / 'result.json', result)
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(execute(pathlib.Path(sys.argv[1]), '--replay' in sys.argv[2:]))
