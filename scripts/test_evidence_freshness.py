"""Evidence freshness: cited SHAs must be on main and within the allowed lag."""
import contextlib
import io
import json
import os
import pathlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import check_evidence_freshness as fresh
import collect_evidence as evidence


def git(repo, *args):
    # Stable fixture SHAs: time-dependent seven-character prefixes can be
    # all digits or all letters and intentionally skipped by the doc scanner.
    env = dict(os.environ, GIT_AUTHOR_DATE="2000-01-01T00:00:00+0000",
               GIT_COMMITTER_DATE="2000-01-01T00:00:00+0000")
    return subprocess.run(["git", *args], cwd=repo, check=True,
                          capture_output=True, text=True, env=env).stdout.strip()


class FreshnessTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(self.tmp.name)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.invalid")
        git(self.repo, "config", "user.name", "t")
        (self.repo / "docs/business").mkdir(parents=True)
        self.commits = []
        for i in range(5):
            (self.repo / f"f{i}").write_text(str(i))
            git(self.repo, "add", "-A")
            git(self.repo, "commit", "-q", "-m", f"c{i}")
            self.commits.append(git(self.repo, "rev-parse", "HEAD"))
        self.patch = patch.object(fresh, "ROOT", self.repo)
        self.patch.start()

    def tearDown(self):
        self.patch.stop()
        self.tmp.cleanup()

    def write(self, text):
        (self.repo / "docs/SCOREBOARD.md").write_text(text, encoding="utf-8")

    def run_check(self, max_lag=2):
        with contextlib.redirect_stdout(io.StringIO()):
            return fresh.check(self.commits[-1], max_lag, self.repo)

    def test_recent_sha_passes(self):
        self.write(f"measured on `main` @ `{self.commits[-2][:7]}`\n")
        self.assertEqual(self.run_check(), (1, []))

    def test_sha_beyond_allowed_lag_fails(self):
        self.write(f"measured on `{self.commits[0][:7]}`\n")
        seen, errors = self.run_check(max_lag=2)
        self.assertEqual(seen, 1)
        self.assertIn("merges behind", errors[0])

    def test_unknown_sha_fails(self):
        self.write("measured on `deadbee1`\n")
        _, errors = self.run_check()
        self.assertIn("not a commit", errors[0])

    def test_run_link_without_sha_fails(self):
        self.write("CI green: https://github.com/o/r/actions/runs/123\n")
        _, errors = self.run_check()
        self.assertIn("without a commit SHA", errors[0])

    def test_elided_hash_and_exempt_line_are_ignored(self):
        self.write(f"sha256 `e8b1c817…5a02aa`\nold `{self.commits[0][:7]}` evidence-lag: historical\n")
        self.assertEqual(self.run_check(), (0, []))

    def test_manifest_commit_sha_is_the_evidence_commit(self):
        manifest = self.repo / "manifest.json"
        manifest.write_text(json.dumps({"commit_sha": self.commits[2]}))
        self.assertEqual(fresh.evidence_commit(str(manifest)), self.commits[2])
        manifest.write_text(json.dumps({}))
        with self.assertRaises(SystemExit):
            fresh.evidence_commit(str(manifest))


class CollectRecordsShaTests(unittest.TestCase):
    def test_manifest_records_commit_sha(self):
        sha = "a" * 40
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "bundle"
            with patch("sys.argv", ["collect_evidence", "--output", str(output)]), \
                 patch.object(evidence, "run", side_effect=lambda c, t:
                              (0, sha + "\n" if c[:2] == ["git", "rev-parse"] else "ok", "")), \
                 patch.object(evidence, "COMMANDS", [("host-tests", ["cargo", "test"])]), \
                 contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(evidence.main(), 0)
            manifest = json.loads((output / "manifest.json").read_text())
        self.assertEqual(manifest["commit_sha"], sha)


if __name__ == "__main__":
    unittest.main()
