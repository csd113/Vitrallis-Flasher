import unittest

import uimage


class UImageTests(unittest.TestCase):
    def test_build_and_parse_roundtrip(self):
        payload = b'#!/bin/sh\necho hello\n'
        blob = uimage.build({'type': uimage.UIMAGE_TYPE_SCRIPT, 'timestamp': 1234}, payload)
        header, parsed = uimage.parse(blob)
        self.assertEqual(header['type'], uimage.UIMAGE_TYPE_SCRIPT)
        self.assertEqual(header['timestamp'], 1234)
        self.assertEqual(parsed, payload)
        self.assertEqual(uimage.script_text(parsed), payload.decode())

    def test_crcs_are_verified(self):
        blob = bytearray(uimage.build({'type': 2}, b'kernel-payload'))
        blob[10] ^= 0xFF
        with self.assertRaises(uimage.UImageError):
            uimage.parse(bytes(blob))
        blob = bytearray(uimage.build({'type': 2}, b'kernel-payload'))
        blob[70] ^= 0xFF
        with self.assertRaises(uimage.UImageError):
            uimage.parse(bytes(blob))
        with self.assertRaises(uimage.UImageError):
            uimage.parse(bytes(blob[:40]))

    def test_script_text_handles_the_flash_kernel_prefix(self):
        body = b'# script body\nbootz 0x42000000 - 0x43000000\n'
        prefixed = len(body).to_bytes(4, 'big') + b'\x00' * 4 + body
        self.assertEqual(uimage.script_text(prefixed), body.decode())
        inconsistent = b'\xff\xff\xff\xff' + b'\x00' * 4 + body
        with self.assertRaises(UnicodeDecodeError):
            uimage.script_text(inconsistent)
        self.assertEqual(uimage.script_text(body), body.decode())


if __name__ == '__main__':
    unittest.main()
