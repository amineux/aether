"""Evidence must never turn failed or incomplete commands into a passing bundle."""
import contextlib
import io
import json
import pathlib
import os
import subprocess
import sys
import time
import tempfile
import unittest
from unittest.mock import patch

import collect_evidence as evidence


class EvidenceTests(unittest.TestCase):
    def capture(self, result, commands=None):
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "bundle"
            with patch("sys.argv", ["collect_evidence", "--output", str(output)]), \
                 patch.object(evidence, "run", side_effect=result), \
                 patch.object(evidence, "COMMANDS", commands or [("host-tests", ["cargo", "test"])]), \
                 contextlib.redirect_stdout(io.StringIO()):
                code = evidence.main()
            return code, json.loads((output / "manifest.json").read_text())

    def test_nonzero_exit_preserves_failure(self):
        code, manifest = self.capture(lambda command, timeout:
                                      (7, "partial output", "failure") if command[-1] == "test"
                                      else (0, "metadata", ""))
        self.assertEqual(code, 1)
        self.assertTrue(manifest["complete"])
        self.assertFalse(manifest["passed"])
        self.assertEqual(manifest["checks"][0]["exit_code"], 7)

    def test_missing_golden_output_fails_even_with_zero_exit(self):
        code, manifest = self.capture(lambda command, timeout: (0, "", ""),
                                      [("diligence", ["cargo", "run"])])
        self.assertEqual(code, 1)
        self.assertTrue(manifest["checks"][0]["missing_expected_lines"])

    def test_success_is_marked_complete(self):
        code, manifest = self.capture(self.clean_result)
        self.assertEqual(code, 0)
        self.assertTrue(manifest["complete"])
        self.assertTrue(manifest["passed"])

    @staticmethod
    def clean_result(command, timeout):
        if command == ['git', 'rev-parse', 'HEAD']:
            return 0, 'a' * 40, ''
        if command == ['git', 'status', '--porcelain']:
            return 0, '', ''
        return 0, 'ok', ''

    def test_dirty_start_or_end_refuses_without_losing_logs(self):
        for dirty_call in (1, 2):
            calls = 0
            def result(command, timeout):
                nonlocal calls
                if command == ['git', 'status', '--porcelain']:
                    calls += 1
                    return 0, ' M core/src/lib.rs' if calls == dirty_call else '', ''
                return self.clean_result(command, timeout)
            with self.subTest(dirty_call=dirty_call):
                code, manifest = self.capture(result)
                self.assertEqual(1, code)
                self.assertTrue(manifest['complete'])
                self.assertFalse(manifest['passed'])
                self.assertTrue(manifest['checks'][0]['passed'])
                self.assertIn('logs', manifest['checks'][0])
                self.assertTrue(manifest['provenance_issues'])

    def test_invalid_commit_refuses_even_when_git_exits_zero(self):
        def result(command, timeout):
            if command == ['git', 'rev-parse', 'HEAD']:
                return 0, 'not-a-commit', ''
            return self.clean_result(command, timeout)
        code, manifest = self.capture(result)
        self.assertEqual(1, code)
        self.assertFalse(manifest['passed'])

    def test_missing_metadata_fails_bundle(self):
        code, manifest = self.capture(lambda command, timeout:
                                      (1, "", "git unavailable") if command[0] == "git"
                                      else (0, "ok", ""))
        self.assertEqual(code, 1)
        self.assertFalse(manifest["passed"])

    def test_eval_run_command_is_collected(self):
        names = [name for name, _command in evidence.COMMANDS]
        self.assertIn("eval-run", names)
        command = dict(evidence.COMMANDS)["eval-run"]
        self.assertEqual(command[0], "cargo")
        self.assertIn("--locked", command)
        self.assertIn("aether-eval-run", command)

    def test_missing_executable_is_recorded(self):
        with patch("subprocess.Popen", side_effect=FileNotFoundError("missing cargo")):
            code, stdout, stderr = evidence.run(["cargo"], 10)
        self.assertEqual(code, 1)
        self.assertIn("missing cargo", stderr)


class ProcessTests(unittest.TestCase):
    def test_real_command_captures_both_streams_and_exit_code(self):
        code, out, err = evidence.run([sys.executable, '-c',
                                     'import sys; print("out"); print("err", file=sys.stderr); sys.exit(7)'], 5)
        self.assertEqual(7, code)
        self.assertIn('out', out)
        self.assertIn('err', err)

    def test_timeout_preserves_partial_output(self):
        code, out, err = evidence.run([sys.executable, '-c',
                                     'import sys,time; print("partial", flush=True); print("diagnostic", file=sys.stderr, flush=True); time.sleep(30)'], 0.3)
        self.assertEqual(124, code)
        self.assertIn('partial', out)
        self.assertIn('diagnostic', err)
        self.assertIn('timed out', err)

    @unittest.skipUnless(os.name == 'posix', 'POSIX process-group lifetime')
    def test_timeout_kills_child_that_inherits_output_pipes(self):
        # If only the parent dies, the child keeps the pipes open for 30s.
        start = time.monotonic()
        code, out, err = evidence.run([sys.executable, '-c',
            'import subprocess,sys,time; subprocess.Popen([sys.executable,"-c","import time; time.sleep(30)"]); print("spawned",flush=True); time.sleep(30)'], 0.5)
        self.assertEqual(124, code)
        self.assertIn('spawned', out)
        self.assertLess(time.monotonic() - start, 5)

    def test_atomic_write_failure_leaves_previous_manifest_intact(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'manifest.json'
            path.write_text('{"complete": false}')
            with patch('evidence_io.os.replace', side_effect=OSError('disk failure')):
                with self.assertRaises(OSError):
                    evidence.atomic_write_text(path, '{"complete": true}')
            self.assertEqual('{"complete": false}', path.read_text())
            self.assertEqual([path], list(path.parent.iterdir()))

    def test_commit_change_fails_even_when_working_tree_is_clean(self):
        calls = 0
        def result(command, timeout):
            nonlocal calls
            if command == ['git', 'rev-parse', 'HEAD']:
                calls += 1
                return 0, ('a' if calls == 1 else 'b') * 40, ''
            return 0, '', ''
        code, data = EvidenceTests().capture(result)
        self.assertEqual(1, code)
        self.assertFalse(data['passed'])
        self.assertNotEqual(data['metadata']['commit']['stdout'], data['metadata']['commit_end']['stdout'])


if __name__ == "__main__":
    unittest.main()
