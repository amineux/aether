"""AETHFS01 producer contract: reject ambiguous names before publication."""
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import mkbootfs


class BootFsPackTests(unittest.TestCase):
    def test_directory_payload_and_sector_padding(self):
        image = mkbootfs.pack([("/init", b"abc"), ("/probe", b"defg")])
        self.assertEqual(len(image), 512)
        self.assertEqual(image[:16], b"AETHFS01" + struct.pack("<II", 2, 0))
        self.assertEqual(image[16:32], b"/init" + bytes(11))
        self.assertEqual(image[32:40], struct.pack("<II", 64, 3))
        self.assertEqual(image[40:56], b"/probe" + bytes(10))
        self.assertEqual(image[56:64], struct.pack("<II", 67, 4))
        self.assertEqual(image[64:71], b"abcdefg")
        self.assertEqual(image[71:], bytes(512 - 71))

    def test_full_width_and_punctuation_names(self):
        for name in ["/0123456789abcde", "/A_z-9.bin"]:
            with self.subTest(name=name):
                image = mkbootfs.pack([(name, b"x")])
                self.assertEqual(image[16:32], name.encode("ascii").ljust(16, b"\0"))

    def test_invalid_and_duplicate_names_refuse(self):
        for name in ["bad", "/", "/bad/name", "/bad name", "/bad\0tail",
                     "/é", "/0123456789abcdef", "/bad\n", None]:
            with self.subTest(name=name), self.assertRaises(SystemExit):
                mkbootfs.pack([("/init", b"ok"), (name, b"bad")])
        with self.assertRaisesRegex(SystemExit, "duplicate"):
            mkbootfs.pack([("/init", b"a"), ("/init", b"b")])

    def test_empty_and_oversized_directories_refuse(self):
        for files in [[], [("/init", b"x")] * (mkbootfs.MAX_FILES + 1)]:
            with self.assertRaises(SystemExit):
                mkbootfs.pack(files)

    def test_extent_overflow_refuses_before_allocating_output(self):
        class HugePayload:
            def __len__(self):
                return mkbootfs.MAX_U32

        with self.assertRaisesRegex(SystemExit, "32-bit extents"):
            mkbootfs.pack([("/init", HugePayload())])

    def test_cli_failure_preserves_existing_image(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "init.elf"
            source.write_bytes(b"ELF")
            output = root / "boot.img"
            output.write_bytes(b"existing image")
            result = subprocess.run(
                [sys.executable, str(Path(mkbootfs.__file__)), "--out", str(output),
                 f"/init={source}", f"/init={source}"],
                capture_output=True, text=True, check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("duplicate", result.stderr)
            self.assertEqual(output.read_bytes(), b"existing image")


if __name__ == "__main__":
    unittest.main()
