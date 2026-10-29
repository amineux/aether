#!/usr/bin/env python3
"""Measure host admission latency and compiler-reported x86-64 function frames.

The LLVM metadata measures individual static frames, not peak call-chain usage.
RUSTC_BOOTSTRAP enables metadata emission only in this opt-in profiling build.
"""
import argparse
import hashlib
import json
import math
import os
import pathlib
import struct
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]


def decode_stack_sizes(data):
    """LLVM ELF64 little-endian .stack_sizes: address followed by ULEB128."""
    sizes = {}
    offset = 0
    while offset < len(data):
        if len(data) - offset < 9:
            raise ValueError('Truncated stack-size entry')
        address = struct.unpack_from('<Q', data, offset)[0]
        offset += 8
        size = 0
        for shift in range(0, 70, 7):
            if offset >= len(data):
                raise ValueError('Truncated stack-size integer')
            byte = data[offset]
            offset += 1
            if shift == 63 and byte > 1:
                raise ValueError('Stack-size integer exceeds 64 bits')
            size |= (byte & 127) << shift
            if not byte & 128:
                break
        else:
            raise ValueError('Unterminated stack-size integer')
        if address in sizes and sizes[address] != size:
            raise ValueError('Conflicting stack-size entries')
        sizes[address] = size
    return sizes


def command(args, env=None):
    return subprocess.check_output(args, cwd=ROOT, env=env, text=True, timeout=300)


def capture(output, samples):
    flags = '-Z emit-stack-sizes'
    env = dict(os.environ, RUSTC_BOOTSTRAP='1', RUSTFLAGS=flags,
               CARGO_TARGET_DIR=str(output / 'build'))
    source = command(['git', 'rev-parse', 'HEAD']).strip()
    changes = command(['git', 'status', '--porcelain'])
    rustc = command(['rustc', '-Vv'])
    command(['cargo', 'build', '--release', '--locked', '-p', 'aether-core',
             '--example', 'admission_profile'], env)
    binary = output / 'build/release/examples/admission_profile'
    header = binary.read_bytes()[:20]
    if header[:6] != b'\x7fELF\x02\x01' or int.from_bytes(header[18:20], 'little') != 62:
        raise ValueError('Profiling supports little-endian x86-64 ELF only')
    raw = output / 'stack-sizes.bin'
    command(['objcopy', '--dump-section', f'.stack_sizes={raw}', str(binary)])
    sizes = decode_stack_sizes(raw.read_bytes())
    symbols = command(['nm', '-nC', str(binary)])
    (output / 'symbols.txt').write_text(symbols)
    frames = {}
    for line in symbols.splitlines():
        fields = line.split(maxsplit=2)
        if len(fields) != 3:
            continue
        for name in ('map_batch', 'map_batch_with_scratch'):
            if fields[2].startswith('aether_core::admission::') and fields[2].endswith('::' + name):
                if name in frames:
                    raise ValueError('Ambiguous admission symbol')
                frames[name] = sizes[int(fields[0], 16)]
    if set(frames) != {'map_batch', 'map_batch_with_scratch'}:
        raise ValueError('Missing compiler frame metadata')
    measured = command([str(binary), str(samples)])
    (output / 'measurements.json').write_text(measured)
    result = json.loads(measured)
    cases = result['cases']
    if {(c['api'], c['outcome']) for c in cases} != {
            (a, o) for a in ('owned_staging', 'caller_scratch') for o in ('success_3', 'refusal_at_2')} or len(cases) != 4:
        raise ValueError('Incomplete latency cases')
    for case in cases:
        values = case['latency_ns']
        if len(values) != samples or any(type(v) is not int or v <= 0 for v in values):
            raise ValueError('Invalid latency samples')
        ordered = sorted(values)
        case['summary_ns'] = {'min': ordered[0], 'p50': ordered[math.ceil(.5 * samples) - 1],
                              'p95': ordered[math.ceil(.95 * samples) - 1], 'max': ordered[-1]}
    result.update(compiler_frames_bytes=frames, rustc=rustc, rustflags=flags,
                  binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                  source_commit=source, source_changes=changes,
                  scope='Host microbenchmark; static function frames, not peak call-chain stack or WCET. '
                        'Scratch initialization, warmup and cleanup excluded from timing. '
                        'No kernel execution, hardware isolation or performance superiority claim.')
    # Structural regression gate only. Shared-runner latency has no threshold.
    result['passed'] = (frames['map_batch_with_scratch'] < result['table_bytes'] // 4
                        and frames['map_batch_with_scratch'] < frames['map_batch'])
    if command(['git', 'rev-parse', 'HEAD']).strip() != source or command(['git', 'status', '--porcelain']) != changes:
        raise ValueError('Source changed during profiling')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--samples', type=int, default=1000)
    args = parser.parse_args()
    if not 10 <= args.samples <= 100_000:
        parser.error('--samples must be 10..100000')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    try:
        result = capture(output, args.samples)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        result = {'passed': False, 'error': str(exc)}
    (output / 'profile.json').write_text(json.dumps(result, indent=2, allow_nan=False) + '\n')
    print(output / 'profile.json')
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
