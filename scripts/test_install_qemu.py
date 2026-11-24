"""QEMU setup must fail closed and switch mirrors only before dpkg runs."""
import contextlib
import io
import pathlib
import tempfile
import unittest
from unittest.mock import patch

import install_qemu as setup


class InstallQemuTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.patch = patch.object(setup.tempfile, "mkdtemp", return_value=self.tmp.name)
        self.patch.start()
        self.addCleanup(self.patch.stop)
        self.calls = []

    def run_install(self, codes, guest="riscv64", release=None, host="x86_64"):
        outcomes = iter(codes)

        def execute(command):
            self.calls.append(command)
            return next(outcomes)

        with contextlib.redirect_stdout(io.StringIO()):
            setup.install(guest, release or {"ID": "ubuntu", "VERSION_CODENAME": "noble"}, host, execute)
        with self.assertRaises(StopIteration):
            next(outcomes)

    def assert_offline(self, command):
        self.assertIn("--no-download", command)
        self.assertNotIn("timeout", command)
        self.assertNotIn("--download-only", command)

    def test_primary_success_downloads_before_install_for_each_guest(self):
        for guest, packages in setup.PACKAGES.items():
            self.calls = []
            self.run_install([0, 0, 0, 0], guest)
            update, download, install, version = self.calls
            self.assertIn("60s", update)
            self.assertIn("APT::Update::Error-Mode=any", update)
            self.assertIn("120s", download)
            self.assertIn("--kill-after=5s", download)
            self.assertIn("--download-only", download)
            self.assertEqual(download[-len(packages):], list(packages))
            self.assert_offline(install)
            self.assertEqual(install[-len(packages):], list(packages))
            self.assertEqual(version, [f"qemu-system-{guest}", "--version"])
            self.assertFalse((self.root / "sources.list").exists())

    def test_timed_out_download_switches_to_isolated_signed_sources(self):
        self.run_install([0, 124, 0, 0, 0, 0])
        update, download, install = self.calls[2:5]
        for command in [update, download, install]:
            self.assertIn(f"Dir::Etc::sourcelist={self.root / 'sources.list'}", command)
            self.assertIn("Dir::Etc::sourceparts=-", command)
            self.assertIn(f"Dir::State::lists={self.root / 'lists'}", command)
        self.assertIn("60s", update)
        self.assertIn("120s", download)
        self.assert_offline(install)
        source = (self.root / "sources.list").read_text()
        self.assertEqual(len(source.splitlines()), 3)
        for line in source.splitlines():
            self.assertIn("[signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://", line)
        self.assertIn("archive.ubuntu.com/ubuntu noble-updates main universe", source)
        self.assertIn("security.ubuntu.com/ubuntu noble-security main universe", source)
        self.assertEqual(self.root.stat().st_mode & 0o777, 0o755)
        self.assertTrue((self.root / "lists/partial").is_dir())

    def test_update_failure_does_not_download_from_incomplete_runner_indexes(self):
        self.run_install([100, 0, 0, 0, 0])
        self.assertEqual(len(self.calls), 5)
        self.assertEqual(self.calls[1][-1], "update")
        self.assertIn("--download-only", self.calls[2])

    def test_failed_fallback_update_never_installs(self):
        with self.assertRaisesRegex(RuntimeError, "acquisition failed"):
            self.run_install([124, 100])
        self.assertEqual(len(self.calls), 2)
        self.assertFalse(any("--no-download" in c for c in self.calls))

    def test_failed_fallback_download_never_installs(self):
        with self.assertRaisesRegex(RuntimeError, "acquisition failed"):
            self.run_install([0, 124, 0, 100])
        self.assertEqual(len(self.calls), 4)
        self.assertFalse(any("--no-download" in c for c in self.calls))

    def test_offline_install_failure_is_not_retried_as_a_mirror_problem(self):
        with self.assertRaisesRegex(RuntimeError, "offline QEMU package installation failed"):
            self.run_install([0, 0, 100])
        self.assertEqual(len(self.calls), 3)
        self.assert_offline(self.calls[-1])
        self.assertFalse((self.root / "sources.list").exists())

    def test_missing_or_broken_emulator_is_a_failure(self):
        with self.assertRaisesRegex(RuntimeError, "executable check failed"):
            self.run_install([0, 0, 0, 1])

    def test_unsupported_host_and_malformed_codename_never_invoke_apt(self):
        cases = [({"ID": "debian", "VERSION_CODENAME": "noble"}, "x86_64"),
                 ({"ID": "ubuntu"}, "x86_64"),
                 ({"ID": "ubuntu", "VERSION_CODENAME": "noble\ndeb bad"}, "x86_64"),
                 ({"ID": "ubuntu", "VERSION_CODENAME": "noble"}, "aarch64")]
        for release, host in cases:
            with self.subTest(release=release, host=host), self.assertRaises(ValueError):
                self.run_install([], release=release, host=host)
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
