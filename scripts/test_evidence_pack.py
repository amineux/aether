"""Unit tests for scripts/evidence_pack.py (no cargo needed)."""
import io
import pathlib
import sys
import tarfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import evidence_pack  # noqa: E402


class EvidencePackTest(unittest.TestCase):
    def test_tar_is_byte_identical_and_owner_free(self):
        files = {"b.log": b"two\n", "a.log": b"one\n"}
        first = evidence_pack.deterministic_tar_gz(dict(files), 1700000000)
        second = evidence_pack.deterministic_tar_gz(dict(reversed(list(files.items()))), 1700000000)
        self.assertEqual(first, second)
        with tarfile.open(fileobj=io.BytesIO(first), mode="r:gz") as tar:
            members = tar.getmembers()
        self.assertEqual([m.name for m in members],
                         ["aether-evidence/a.log", "aether-evidence/b.log"])
        for m in members:
            self.assertEqual((m.uid, m.gid, m.uname, m.gname, m.mtime),
                             (0, 0, "", "", 1700000000))

    def test_discovery_has_not_offered_block(self):
        text = evidence_pack.DISCOVERY.read_text(encoding="utf-8")
        block = evidence_pack.not_offered_block(text)
        self.assertTrue(block.startswith("What it does **not** offer:"))
        self.assertIn("real models", block)
        self.assertIn("throughput", evidence_pack.not_proven_md(block))

    def test_missing_block_fails(self):
        with self.assertRaises(SystemExit):
            evidence_pack.not_offered_block("no such paragraph\n")


if __name__ == "__main__":
    unittest.main()
