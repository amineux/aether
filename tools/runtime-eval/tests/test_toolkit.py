import copy
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from aether_eval import cli
from aether_eval.common import digest, write_json, run_process
from aether_eval.report import render
from aether_eval.scenario import DEFAULT, read_json, validate
from aether_eval.worker import stats, summarize


class ScenarioTests(unittest.TestCase):
    def test_default_valid(self):
        validate(copy.deepcopy(DEFAULT))

    def test_bad_values(self):
        for key, value in [('samples', True), ('samples', 0), ('rounds', 100), ('seed', -1),
                           ('timeout_seconds', 0), ('adapter', 'shell'), ('max_p95_slowdown', float('nan'))]:
            with self.subTest(key=key, value=value):
                data = copy.deepcopy(DEFAULT)
                data[key] = value
                with self.assertRaises(ValueError):
                    validate(data)

    def test_path_names_and_duplicate_jobs_rejected(self):
        for name in ('../escape', 'image-preprocess'):
            data = copy.deepcopy(DEFAULT)
            data['workloads'][1]['name'] = name
            with self.assertRaises(ValueError):
                validate(data)

    def test_unknown_field_and_oversized_shape(self):
        data = copy.deepcopy(DEFAULT)
        data['command'] = 'do something'
        with self.assertRaises(ValueError):
            validate(data)
        data.pop('command')
        data['workloads'][0]['shape'] = [1025, 2]
        with self.assertRaises(ValueError):
            validate(data)

    def test_duplicate_json_and_nan_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'scenario.json'
            for raw in ('{"seed":1,"seed":2}', '{"samples":NaN}'):
                path.write_text(raw)
                with self.assertRaises(ValueError):
                    read_json(path)


class MeasurementTests(unittest.TestCase):
    def samples(self):
        scenario = copy.deepcopy(DEFAULT)
        scenario.update(samples=3, rounds=1, max_p95_slowdown=1.5)
        samples = [{'workload': j['name'], 'phase': p, 'round': 0, 'latency_ms': x}
                   for j in scenario['workloads'] for p, vals in [('solo', [1, 2, 3]), ('shared', [2, 4, 6])] for x in vals]
        return scenario, samples

    def test_percentile_and_gate(self):
        scenario, samples = self.samples()
        result = summarize(scenario, samples)
        for entry in result.values():
            self.assertEqual(2, entry['p95_slowdown'])
            self.assertEqual('failed', entry['latency_gate']['status'])
            self.assertEqual([2], entry['per_round_p95_slowdown'])

    def test_missing_samples_never_pass(self):
        scenario, samples = self.samples()
        with self.assertRaises(ValueError):
            summarize(scenario, samples[:-1])

    def test_invalid_latencies(self):
        for samples in ([], [0], [-1], [float('inf')]):
            with self.assertRaises(ValueError):
                stats(samples)

    def test_no_threshold_is_not_requested(self):
        scenario, samples = self.samples()
        scenario.pop('max_p95_slowdown')
        self.assertEqual('not_requested', summarize(scenario, samples)['image-preprocess']['latency_gate']['status'])


class FailureTests(unittest.TestCase):
    def test_timeout_preserves_diagnostics(self):
        code, out, err = run_process([sys.executable, '-c', 'import time; print("before",flush=True); time.sleep(30)'], .3)
        self.assertEqual(124, code)
        self.assertIn('before', out)
        self.assertIn('timed out', err)

    def test_report_escapes_failure_and_never_claims_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory)
            render({'error': '<script>alert(1)</script>'}, path)
            html = (path / 'report.html').read_text()
            self.assertNotIn('<script>', html)
            self.assertNotIn('CHECKS PASSED', html)
            self.assertIn('unsupported', html)

    def test_invalid_scenario_retains_failure_report(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            scenario = root / 'bad.json'
            scenario.write_text('{broken')
            with patch('sys.argv', ['aether-eval', 'run', str(scenario), '--output', str(root / 'out')]):
                self.assertEqual(2, cli.main())
            self.assertFalse(read_json(root / 'out/result.json')['passed'])
            self.assertTrue((root / 'out/report.html').exists())

    def test_replay_rejects_tampering_before_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = pathlib.Path(directory) / 'source', pathlib.Path(directory) / 'target'
            source.mkdir(); target.mkdir(); (source / 'artifacts').mkdir()
            write_json(source / 'scenario.json', DEFAULT)
            hashes = {'scenario.json': digest(source / 'scenario.json')}
            for job in DEFAULT['workloads']:
                for ext in ('mlir', 'vmfb', 'npz'):
                    name = f'artifacts/{job["name"]}.{ext}'
                    (source / name).write_text('data')
                    hashes[name] = digest(source / name)
            write_json(source / 'artifacts.json', {'schema': 'aether-artifacts/v1', 'sha256': hashes})
            (source / 'artifacts/image-preprocess.vmfb').write_text('changed')
            with self.assertRaisesRegex(ValueError, 'changed'):
                cli.copy_replay(source, target)

    def test_existing_output_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch('sys.argv', ['aether-eval', 'run', 'missing.json', '--output', directory]):
                self.assertEqual(2, cli.main())


@unittest.skipUnless(os.environ.get('AETHER_IREE_TEST') == '1', 'enable real IREE integration with AETHER_IREE_TEST=1')
class RealIreeTests(unittest.TestCase):
    def test_compile_measure_replay_and_regression_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            scenario = copy.deepcopy(DEFAULT)
            scenario.update(samples=3, rounds=2, warmup=1)
            scenario['workloads'][0]['shape'] = [8, 8]
            scenario['workloads'][1]['shape'] = [8, 8, 8]
            write_json(root / 'scenario.json', scenario)
            def call(*args):
                proc = subprocess.run([sys.executable, '-m', 'aether_eval.cli', *map(str, args)], capture_output=True, text=True, timeout=120)
                self.assertEqual(0, proc.returncode, proc.stdout + proc.stderr)
            call('run', root / 'scenario.json', '--output', root / 'first')
            result = read_json(root / 'first/result.json')
            self.assertTrue(result['passed'])
            self.assertEqual(6, len(result['checks']))
            self.assertTrue(all(c['passed'] for c in result['checks']))
            self.assertEqual('unsupported', result['hardware_isolation']['status'])
            self.assertEqual(24, len(read_json(root / 'first/samples.json')))
            call('replay', root / 'first', '--output', root / 'second')
            self.assertTrue(read_json(root / 'second/result.json')['replay'])
            self.assertEqual(read_json(root / 'first/artifacts.json'), read_json(root / 'second/artifacts.json'))
            scenario['max_p95_slowdown'] = 1e-12
            write_json(root / 'scenario.json', scenario)
            proc = subprocess.run([sys.executable, '-m', 'aether_eval.cli', 'run', str(root / 'scenario.json'), '--output', str(root / 'regression')], capture_output=True, text=True, timeout=120)
            self.assertEqual(1, proc.returncode, proc.stdout + proc.stderr)
            failed = read_json(root / 'regression/result.json')
            self.assertTrue(failed['complete'])
            self.assertFalse(failed['passed'])
            self.assertTrue(all(v['latency_gate']['status'] == 'failed' for v in failed['summary'].values()))
