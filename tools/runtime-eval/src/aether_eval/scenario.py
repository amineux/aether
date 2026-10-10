"""Bounded, declarative scenarios. No shell commands or plugin imports in JSON."""
import json
import math
import re

DEFAULT = {
    'schema': 'aether-scenario/v1', 'adapter': 'iree-cpu', 'seed': 42,
    'samples': 10, 'rounds': 3, 'warmup': 2, 'timeout_seconds': 120,
    'workloads': [
        {'name': 'image-preprocess', 'operation': 'normalize', 'shape': [256, 256], 'deadline_ms': 20},
        {'name': 'feature-projection', 'operation': 'matmul', 'shape': [64, 128, 64]},
    ],
}


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'Duplicate field: {key}')
        result[key] = value
    return result


def read_json(path):
    return json.loads(path.read_text(encoding='utf-8'), object_pairs_hook=unique,
                      parse_constant=lambda x: (_ for _ in ()).throw(ValueError(f'Invalid number: {x}')))


def validate(data):
    if not isinstance(data, dict):
        raise ValueError('Scenario must be a JSON object')
    allowed = set(DEFAULT) | {'max_p95_slowdown'}
    if set(data) - allowed:
        raise ValueError(f'Unknown scenario fields: {sorted(set(data) - allowed)}')
    if data.get('schema') != 'aether-scenario/v1' or data.get('adapter') != 'iree-cpu':
        raise ValueError('Supported schema/adapter: aether-scenario/v1 / iree-cpu')
    for key, low, high in [('seed', 0, 2**32-1), ('samples', 3, 1000), ('rounds', 1, 20),
                           ('warmup', 0, 100), ('timeout_seconds', 1, 3600)]:
        value = data.get(key)
        if type(value) is not int or not low <= value <= high:
            raise ValueError(f'{key} must be an integer in [{low}, {high}]')
    def positive(value, key):
        if type(value) not in (int, float) or not math.isfinite(value) or value <= 0:
            raise ValueError(f'{key} must be finite and positive')
    if 'max_p95_slowdown' in data:
        positive(data['max_p95_slowdown'], 'max_p95_slowdown')
    jobs = data.get('workloads')
    if not isinstance(jobs, list) or len(jobs) != 2:
        raise ValueError('Exactly two workloads are required')
    names = set()
    for job in jobs:
        if not isinstance(job, dict) or set(job) - {'name', 'operation', 'shape', 'deadline_ms'}:
            raise ValueError('Invalid workload fields')
        name = job.get('name')
        if not isinstance(name, str) or not re.fullmatch(r'[a-z][a-z0-9-]{0,39}', name) or name in names:
            raise ValueError('Workload names must be unique lowercase names (max 40 characters)')
        names.add(name)
        op, shape = job.get('operation'), job.get('shape')
        if op not in ('normalize', 'matmul'):
            raise ValueError('Supported operations: normalize, matmul')
        if not isinstance(shape, list) or len(shape) != (2 if op == 'normalize' else 3):
            raise ValueError('normalize shape is [height,width]; matmul shape is [m,k,n]')
        if any(type(x) is not int or not 2 <= x <= 1024 for x in shape):
            raise ValueError('Dimensions must be integers between 2 and 1024')
        if 'deadline_ms' in job:
            positive(job['deadline_ms'], 'deadline_ms')
    return data
