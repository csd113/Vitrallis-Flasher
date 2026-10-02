import hashlib
import json
import pathlib
import unittest

import spl
from build import ROOT


def synthetic_spl():
    data = bytearray(16384)
    data[0:4] = (0xEA000016).to_bytes(4, 'little')
    data[4:12] = b'eGON.BT0'
    data[16:20] = (16384).to_bytes(4, 'little')
    data[20:24] = b'SPL\x02'
    for index in range(24, 16384):
        data[index] = (index * 31) & 0xFF
    return bytes(data)


def synthetic_image(oob, sunxi_spl):
    page = 16384 + oob
    image = bytearray(spl.image_size(oob))
    keystream = spl.scrambler_stream(spl.ECC_REGION)
    for number in range(256):
        offset = number * page
        within = number % 64
        if within < 16:
            chunk = sunxi_spl[within * 1024:(within + 1) * 1024]
        else:
            group = number // 64
            chunk = spl.stream(spl.PAD_LABELS[group], spl.PAD_BYTES)[(within - 16) * 1024:(within - 15) * 1024]
        image[offset:offset + 1024] = bytes(a ^ b for a, b in zip(chunk, keystream))
        image[offset + 16384:offset + 16386] = b'\xff\xff'
    return bytes(image)


class StreamTests(unittest.TestCase):
    def test_stream_is_deterministic_and_label_specific(self):
        first = spl.stream(b'label', 1000)
        self.assertEqual(first, spl.stream(b'label', 1000))
        self.assertEqual(len(first), 1000)
        self.assertNotEqual(first, spl.stream(b'other', 1000))
        self.assertNotEqual(first, spl.stream(b'label', 999))
        with self.assertRaises(ValueError):
            spl.stream(b'', 1)
        with self.assertRaises(ValueError):
            spl.stream(b'label', -1)

    def test_pad_and_entropy_streams_are_pinned(self):
        self.assertEqual(spl.PAD_LABELS[0], b'source-pad-0')
        self.assertEqual(spl.stream(spl.PAD_LABELS[0], 8).hex(), '2a99ab4c5cf133e3')
        self.assertEqual(spl.entropy_stream('hynix')[:8].hex(), '117a161334f778ef')
        self.assertEqual(len(spl.entropy_stream('toshiba')), spl.entropy_bytes('toshiba'))

    def test_build_source_layout(self):
        sunxi = synthetic_spl()
        source = spl.build_source(sunxi)
        self.assertEqual(len(source), spl.TOTAL_SOURCE_BYTES)
        for group in range(4):
            offset = group * spl.SOURCE_BYTES
            self.assertEqual(source[offset:offset + spl.SPL_SIZE], sunxi)
            pad = source[offset + spl.SPL_SIZE:offset + spl.SOURCE_BYTES]
            self.assertEqual(pad, spl.stream(spl.PAD_LABELS[group], spl.PAD_BYTES))
        self.assertNotEqual(source[16 * 1024:64 * 1024], source[80 * 1024:128 * 1024])

    def test_validate_source_spl_rejects_malformed_input(self):
        good = synthetic_spl()
        spl.validate_source_spl(good)
        for broken in (b'', good[:-1], b'X' * 16384):
            with self.assertRaises(spl.SplError):
                spl.validate_source_spl(broken)

    def test_geometry_and_variant_lookup(self):
        self.assertEqual(spl.image_size('hynix'), 256 * (16384 + 1664))
        self.assertEqual(spl.image_size('toshiba'), 256 * (16384 + 1280))
        self.assertEqual(spl.oob_for_variant(1664), 1664)
        self.assertEqual(spl.variant_token(1280), 'toshiba')
        for bad in ('samsung', 0, 42, None):
            with self.assertRaises(spl.SplError):
                spl.oob_for_variant(bad)


class ScramblerTests(unittest.TestCase):
    def test_golden_keystream(self):
        self.assertEqual(spl.scrambler_stream(16).hex(), 'c06f102c0c1dc5c99316edcecd9455af')

    def test_descramble_inverts_scramble(self):
        data = bytes(index % 251 for index in range(spl.ECC_REGION))
        scrambled = bytes(a ^ b for a, b in zip(data, spl.scrambler_stream(len(data))))
        self.assertEqual(spl.descramble(scrambled), data)

    def test_descramble_requires_the_exact_region(self):
        with self.assertRaises(spl.SplError):
            spl.descramble(b'short')


class ImageValidationTests(unittest.TestCase):
    def test_synthetic_image_is_accepted_and_mutations_are_rejected(self):
        sunxi = synthetic_spl()
        for variant, oob in spl.OOB_SIZES.items():
            image = synthetic_image(oob, sunxi)
            spl.validate_image(image, oob, sunxi)
            self.assertEqual(len(image), spl.image_size(oob))
            bad = bytearray(image)
            bad[0] ^= 0x01
            with self.assertRaises(spl.SplError):
                spl.validate_image(bytes(bad), oob, sunxi)
            bad = bytearray(image)
            bad[16384] = 0x00
            with self.assertRaises(spl.SplError):
                spl.validate_image(bytes(bad), oob, sunxi)
            with self.assertRaises(spl.SplError):
                spl.validate_image(image[:-1], oob, sunxi)
            with self.assertRaises(spl.SplError):
                spl.validate_image(image, spl.OOB_SIZES['hynix' if variant == 'toshiba' else 'toshiba'], sunxi)

    def test_locked_spl_pins_match_the_geometry(self):
        lock = json.loads((ROOT / 'images/inputs.lock.json').read_text())
        for variant in ('hynix', 'toshiba'):
            pin = lock['derived']['spl-' + variant]
            self.assertEqual(pin['size'], spl.image_size(variant))
            self.assertEqual(len(pin['sha256']), 64)
        self.assertEqual(len(lock['derived']['u-boot-dtb-padded.bin']['sha256']), 64)


if __name__ == '__main__':
    unittest.main()
