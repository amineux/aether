"""Install, diagnose, run and replay external runtime evaluations."""
import argparse
import datetime
import json
import pathlib
import shutil
import sys

from .common import atomic_text, digest, environment, run_process, write_json
from .report import render
from .scenario import DEFAULT, read_json, validate
from .worker import LIMITS


def copy_replay(source, output):
    scenario = validate(read_json(source / 'scenario.json'))
    manifest = read_json(source / 'artifacts.json')
    names = ['scenario.json'] + [f'artifacts/{j["name"]}.{ext}' for j in scenario['workloads'] for ext in ('mlir', 'vmfb', 'npz')]
    if not isinstance(manifest, dict) or manifest.get('schema') != 'aether-artifacts/v1' or not isinstance(manifest.get('sha256'), dict) or set(manifest['sha256']) != set(names):
        raise ValueError('Incomplete or unsupported replay artifact manifest')
    for name in names:
        path = source / name
        if not path.is_file() or path.is_symlink() or digest(path) != manifest['sha256'][name]:
            raise ValueError(f'Missing or changed replay artifact: {name}')
    (output / 'artifacts').mkdir()
    for name in names:
        # Copy and verify the copy to detect source changes during copying.
        target = output / name
        shutil.copyfile(source / name, target)
        if digest(target) != manifest['sha256'][name]:
            raise ValueError(f'Artifact changed during replay copy: {name}')
    return scenario


def evaluate(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {'schema': 'aether-runtime-eval/v1', 'complete': False, 'passed': False,
              'created_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'environment': environment(), 'limitations': LIMITS}
    write_json(output / 'result.json', result)
    render(result, output)
    try:
        if args.action == 'replay':
            scenario = copy_replay(args.bundle.resolve(), output)
            result['replayed_from'] = str(args.bundle.resolve())
        else:
            scenario = validate(read_json(args.scenario.resolve()))
            write_json(output / 'scenario.json', scenario)
        command = [sys.executable, '-m', 'aether_eval.worker', str(output)]
        if args.action == 'replay':
            command.append('--replay')
        code, stdout, stderr = run_process(command, scenario['timeout_seconds'])
        atomic_text(output / 'worker.stdout.log', stdout)
        atomic_text(output / 'worker.stderr.log', stderr)
        worker_result = read_json(output / 'result.json')
        if code in (0, 1) and worker_result.get('complete') is True:
            worker_result['created_at_utc'] = result['created_at_utc']
            worker_result['replayed_from'] = result.get('replayed_from')
            result = worker_result
            if code != 0:
                result['passed'] = False
        else:
            result['error'] = f'Worker exited with code {code}; see worker.stderr.log. Partial samples, if any, are retained.'
        result['worker_exit_code'] = code
        result['worker_command'] = command
        result['completed_at_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        write_json(output / 'result.json', result)
        render(result, output)
        print(f'{output / "report.html"}: {"passed" if result.get("passed") else "needs attention"}')
        return 0 if result.get('passed') and result.get('complete') else 1
    except Exception as exc:
        result.update(error=str(exc), passed=False, complete=False)
        write_json(output / 'result.json', result)
        render(result, output)
        print(f'Evaluation failed: {exc}. Report: {output / "report.html"}', file=sys.stderr)
        return 2


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    init = sub.add_parser('init', help='Write a two-workload CPU scenario')
    init.add_argument('--output', type=pathlib.Path, default=pathlib.Path('scenario.json'))
    sub.add_parser('doctor', help='Check the actual IREE CPU runtime and pinned dependency versions')
    run = sub.add_parser('run', help='Compile, validate and measure a scenario')
    run.add_argument('scenario', type=pathlib.Path)
    run.add_argument('--output', type=pathlib.Path, required=True)
    replay = sub.add_parser('replay', help='Execute a trusted bundle using saved binaries and inputs')
    replay.add_argument('bundle', type=pathlib.Path)
    replay.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    try:
        if args.action == 'init':
            # Exclusive create: never clobber a user's scenario.
            with args.output.open('x', encoding='utf-8') as stream:
                json.dump(DEFAULT, stream, indent=2)
                stream.write('\n')
            print(args.output)
            return 0
        if args.action == 'doctor':
            info = environment()
            try:
                from .iree_cpu import make_config
                make_config()
                info['iree_cpu_available'] = True
            except (ImportError, RuntimeError, ValueError) as exc:
                info.update(iree_cpu_available=False, error=str(exc),
                            install='python -m pip install "./tools/runtime-eval[iree]"')
            print(json.dumps(info, indent=2))
            return 0 if info['iree_cpu_available'] else 2
        return evaluate(args)
    except (OSError, ValueError) as exc:
        print(f'aether-eval: {exc}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
