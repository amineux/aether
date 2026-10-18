"""The scoreboard reproducer must fail closed on incomplete evidence."""
import contextlib
import hashlib
import io
import pathlib
import tempfile
import unittest
from unittest.mock import patch

import verify


class VerifyTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.scoreboard = pathlib.Path(self.directory.name) / "SCOREBOARD.md"
        self.redteam = "attack result=refused\n"
        self.values = dict(refused="1", variants="2", named="1", allowlisted="1",
                           redteam_sha256=hashlib.sha256(self.redteam.encode()).hexdigest())
        self.write_values(self.values)
        self.patcher = patch.object(verify, "SCOREBOARD", self.scoreboard)
        self.patcher.start()
        self.addCleanup(self.patcher.stop)

    def write_values(self, values):
        self.scoreboard.write_text("verify " + " ".join(f"{k}={v}" for k, v in values.items()) + "\n")

    def command(self, command):
        if command == verify.HOST_TESTS:
            return 0, "test result: ok. 3 passed; 0 failed; 0 ignored\n", ""
        if command == verify.REDTEAM:
            return 0, self.redteam, ""
        if command == verify.COVERAGE:
            return 0, "[error-coverage] variants=2 named=1 allowlisted=1\n", ""
        if command == verify.DILIGENCE:
            return 0, (verify.ROOT / "examples/diligence-demo/expected.txt").read_text(), ""
        self.fail(f"unexpected command {command}")

    def invoke(self, command=None):
        out, err = io.StringIO(), io.StringIO()
        with patch.object(verify, "run", side_effect=command or self.command) as run, \
             contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = verify.main()
        return code, out.getvalue(), err.getvalue(), run

    def test_valid_contract_passes(self):
        code, out, _, run = self.invoke()
        self.assertEqual(code, 0)
        self.assertIn("scoreboard_match=true", out)
        self.assertEqual(run.call_count, 5)

    def test_every_required_field_is_mandatory(self):
        for key in self.values:
            with self.subTest(key=key):
                self.write_values({k: v for k, v in self.values.items() if k != key})
                code, out, err, run = self.invoke()
                self.assertEqual(code, 1)
                self.assertNotIn("scoreboard_match=true", out)
                self.assertIn(key, err)
                run.assert_not_called()

    def test_invalid_contracts_fail_before_commands(self):
        valid = self.scoreboard.read_text().strip()
        for line in ["", "verify", valid + "\n" + valid, valid + " refused=1",
                     valid + " extra=1", valid + " broken", valid.replace("refused=1", "refused=-1"),
                     valid.replace("variants=2", "variants=3"),
                     valid.replace(self.values["redteam_sha256"], "not-a-hash")]:
            with self.subTest(line=line):
                self.scoreboard.write_text(line + "\n")
                code, out, err, run = self.invoke()
                self.assertEqual(code, 1)
                self.assertIn("[verify] FAIL", err)
                self.assertNotIn("scoreboard_match=true", out)
                run.assert_not_called()

    def test_optional_host_count_is_compared(self):
        for count, expected in [("3", 0), ("4", 1)]:
            with self.subTest(count=count):
                self.write_values(dict(self.values, host_tests_passed=count))
                self.assertEqual(self.invoke()[0], expected)

    def test_unreadable_scoreboard_fails_before_commands(self):
        self.scoreboard.unlink()
        code, _, err, run = self.invoke()
        self.assertEqual(code, 1)
        self.assertIn("[verify] FAIL", err)
        run.assert_not_called()

    def test_host_summaries_are_summed_across_suites_and_streams(self):
        self.write_values(dict(self.values, host_tests_passed="5"))
        def command(args):
            if args == verify.HOST_TESTS:
                return (0, "test result: ok. 3 passed; 0 failed\n",
                        "test result: ok. 2 passed; 0 failed\n")
            return self.command(args)
        self.assertEqual(self.invoke(command)[0], 0)

    def test_failed_host_summary_fails_even_with_zero_exit(self):
        def command(args):
            return (0, "test result: FAILED. 3 passed; 1 failed\n", "") if args == verify.HOST_TESTS else self.command(args)
        self.assertEqual(self.invoke(command)[0], 1)

    def test_missing_host_summary_fails(self):
        def command(args):
            return (0, "", "") if args == verify.HOST_TESTS else self.command(args)
        self.assertEqual(self.invoke(command)[0], 1)

    def test_zero_host_tests_fails(self):
        def command(args):
            return (0, "test result: ok. 0 passed; 0 failed\n", "") if args == verify.HOST_TESTS else self.command(args)
        self.assertEqual(self.invoke(command)[0], 1)

    def test_nonzero_command_exits_fail(self):
        for target in [verify.HOST_TESTS, verify.DILIGENCE, verify.REDTEAM, verify.COVERAGE]:
            with self.subTest(target=target):
                def command(args):
                    code, out, err = self.command(args)
                    return (7 if args == target else code), out, err
                self.assertEqual(self.invoke(command)[0], 1)

    def test_nondeterministic_redteam_fails(self):
        calls = 0
        def command(args):
            nonlocal calls
            code, out, err = self.command(args)
            if args == verify.REDTEAM:
                calls += 1
                if calls == 2:
                    out += "changed\n"
            return code, out, err
        self.assertEqual(self.invoke(command)[0], 1)

    def test_measurement_mismatch_fails(self):
        self.write_values(dict(self.values, refused="2"))
        self.assertEqual(self.invoke()[0], 1)

    def test_missing_diligence_output_fails(self):
        def command(args):
            return (0, "", "") if args == verify.DILIGENCE else self.command(args)
        self.assertEqual(self.invoke(command)[0], 1)


if __name__ == "__main__":
    unittest.main()
