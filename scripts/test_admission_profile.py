import struct
import unittest

from admission_profile import decode_stack_sizes


class StackMetadataTests(unittest.TestCase):
    def test_zero_and_multibyte_sizes(self):
        data = struct.pack('<Q', 0x1000) + b'\x00' + struct.pack('<Q', 0x2000) + b'\xc8\x0b'
        self.assertEqual({0x1000: 0, 0x2000: 1480}, decode_stack_sizes(data))

    def test_truncation_and_overflow_refuse(self):
        for data in (b'\x00', struct.pack('<Q', 1), struct.pack('<Q', 1) + b'\x80',
                     struct.pack('<Q', 1) + b'\xff' * 9 + b'\x02'):
            with self.subTest(data=data), self.assertRaises(ValueError):
                decode_stack_sizes(data)

    def test_conflicting_metadata_refuses(self):
        with self.assertRaises(ValueError):
            decode_stack_sizes(struct.pack('<Q', 1) + b'\x01' + struct.pack('<Q', 1) + b'\x02')
