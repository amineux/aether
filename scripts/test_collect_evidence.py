"""Evidence must never turn failed or incomplete commands into a passing bundle."""
import contextlib
import io
import json
import pathlib
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
        code, manifest = self.capture(lambda command, timeout: (0, "ok", ""))
        self.assertEqual(code, 0)
        self.assertTrue(manifest["complete"])
        self.assertTrue(manifest["passed"])

    def test_missing_metadata_fails_bundle(self):
        code, manifest = self.capture(lambda command, timeout:
                                      (1, "", "git unavailable") if command[0] == "git"
                                      else (0, "ok", ""))
        self.assertEqual(code, 1)
        self.assertFalse(manifest["passed"])

    def test_missing_executable_is_recorded(self):
        with patch("subprocess.run", side_effect=FileNotFoundError("missing cargo")):
            code, stdout, stderr = evidence.run(["cargo"], 10)
        self.assertEqual(code, 1)
        self.assertIn("missing cargo", stderr)


if __name__ == "__main__":
    unittest.main()
