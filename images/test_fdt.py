import struct
import unittest

from fdt import FDT_MAGIC, Fdt, FdtError


class FdtBuilder:
    def __init__(self):
        self.structure = bytearray()
        self.strings = bytearray()
        self.offsets = {}

    def _string(self, name):
        if name not in self.offsets:
            self.offsets[name] = len(self.strings)
            self.strings += name.encode() + b'\x00'
        return self.offsets[name]

    def _align(self):
        while len(self.structure) % 4:
            self.structure += b'\x00'

    def begin(self, name):
        self.structure += struct.pack('>I', 1) + name.encode() + b'\x00'
        self._align()

    def end(self):
        self.structure += struct.pack('>I', 2)

    def prop(self, name, value):
        self.structure += struct.pack('>III', 3, len(value), self._string(name)) + value
        self._align()

    def finish(self):
        self.structure += struct.pack('>I', 9)
        off_struct = 40 + 16
        off_strings = off_struct + len(self.structure)
        total = off_strings + len(self.strings)
        return (
            struct.pack('>10I', FDT_MAGIC, total, off_struct, off_strings, 40, 17, 16, 0, len(self.strings), len(self.structure))
            + b'\x00' * 16
            + bytes(self.structure)
            + bytes(self.strings)
        )


def sample_tree():
    builder = FdtBuilder()
    builder.begin('')
    builder.prop('compatible', b'vitrallis,test\x00')
    builder.begin('__symbols__')
    builder.prop('pwm', b'/soc/pwm\x00')
    builder.prop('i2c1', b'/soc/i2c@1c2ac00\x00')
    builder.end()
    builder.begin('soc')
    builder.begin('pwm')
    builder.prop('status', b'okay\x00')
    builder.end()
    builder.end()
    builder.end()
    return builder.finish()


class FdtTests(unittest.TestCase):
    def test_parse_symbols_and_properties(self):
        tree = Fdt(sample_tree())
        self.assertEqual(tree.string('/', 'compatible'), 'vitrallis,test')
        self.assertEqual(tree.symbols(), {'pwm': '/soc/pwm', 'i2c1': '/soc/i2c@1c2ac00'})
        self.assertIsNone(tree.node('/missing'))
        self.assertEqual(tree.string('/soc/pwm', 'status'), 'okay')
        self.assertIsNone(tree.string('/soc/pwm', 'absent'))

    def test_external_fixups(self):
        builder = FdtBuilder()
        builder.begin('')
        builder.begin('__fixups__')
        builder.prop('pwm', b'/soc/pwm:status:0\x00')
        builder.prop('i2c1', b'/soc/i2c@1c2ac00:status:0\x00/a:status:4\x00')
        builder.end()
        builder.end()
        fixups = Fdt(builder.finish()).external_fixups()
        self.assertEqual(sorted(fixups), ['i2c1', 'pwm'])
        self.assertEqual(fixups['pwm'], ['/soc/pwm:status:0'])
        self.assertEqual(len(fixups['i2c1']), 2)

    def test_malformed_blobs_are_rejected(self):
        good = sample_tree()
        with self.assertRaises(FdtError):
            Fdt(b'')
        with self.assertRaises(FdtError):
            Fdt(b'X' * 64)
        truncated = bytearray(good)
        truncated[4:8] = struct.pack('>I', 1)
        with self.assertRaises(FdtError):
            Fdt(bytes(truncated))

    def test_bad_path_lookup_is_rejected(self):
        tree = Fdt(sample_tree())
        with self.assertRaises(FdtError):
            tree.node('/../etc')
        with self.assertRaises(FdtError):
            tree.node('relative')

    def test_duplicate_node_and_unbalanced_structure(self):
        builder = FdtBuilder()
        builder.begin('')
        builder.begin('dup')
        builder.end()
        builder.begin('dup')
        builder.end()
        builder.end()
        with self.assertRaises(FdtError):
            Fdt(builder.finish())
        builder = FdtBuilder()
        builder.begin('')
        with self.assertRaises(FdtError):
            Fdt(builder.finish())


if __name__ == '__main__':
    unittest.main()
