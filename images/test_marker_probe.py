import hashlib
import pathlib
import struct
import tempfile
import unittest
from unittest import mock

from fdt import Fdt
from marker_probe import append_probe, build, validate_preservation, PARTITIONS, PROBE_NAME
from test_fdt import FdtBuilder


def tree(probe=False, probe_first=False, writable=False, slc=False):
    b = FdtBuilder()
    for name in ['', 'soc', 'nand-controller@1c03000', 'nand@0', 'partitions']:
        b.begin(name)
    b.prop('#address-cells', struct.pack('>I', 2))
    b.prop('#size-cells', struct.pack('>I', 2))
    names = ['partition@0', 'partition@400000', 'partition@800000', 'partition@c00000', 'partition@1000000']
    if probe:
        names.insert(0 if probe_first else len(names), PROBE_NAME)
    for name in names:
        b.begin(name)
        if name == PROBE_NAME:
            b.prop('label', b'BBM.probe\0')
            b.prop('reg', struct.pack('>4I', 0, 947912704, 0, 4194304))
            if not writable:
                b.prop('read-only', b'')
            if slc:
                b.prop('slc-mode', b'')
        else:
            b.prop('label', name.encode() + b'\0')
            b.prop('reg', struct.pack('>4I', 0, int(name.split('@')[1], 16), 0, 4194304))
        b.end()
    for _ in range(5):
        b.end()
    return b.finish()


class MarkerProbeTests(unittest.TestCase):
    def test_append_is_deterministic_preserves_indices_and_exposes_only_read_only_block(self):
        original = tree()
        with mock.patch('marker_probe.BASE_SHA256', hashlib.sha256(original).hexdigest()):
            first = append_probe(original)
            self.assertEqual(first, append_probe(original))
        validate_preservation(original, first)
        partitions = Fdt(first).node(PARTITIONS)
        self.assertEqual(list(partitions.children)[-1], PROBE_NAME)
        self.assertEqual(len(partitions.children), 6)
        probe = partitions.children[PROBE_NAME]
        self.assertEqual(probe.properties['read-only'], b'')
        self.assertNotIn('slc-mode', probe.properties)
        self.assertEqual(probe.properties['reg'], struct.pack('>4I', 0, 947912704, 0, 4194304))

    def test_overlay_style_prepend_cannot_renumber_existing_mtd_devices(self):
        with self.assertRaisesRegex(ValueError, 'partition order changed'):
            validate_preservation(tree(), tree(probe=True, probe_first=True))

    def test_writable_or_slc_probe_is_rejected(self):
        for changed in [tree(probe=True, writable=True), tree(probe=True, slc=True)]:
            with self.assertRaises(ValueError):
                validate_preservation(tree(), changed)

    def test_unpinned_input_fails_before_output_creation(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory).resolve() / 'base.dtb'
            base.write_bytes(tree())
            output = pathlib.Path(directory).resolve() / 'output'
            with self.assertRaisesRegex(ValueError, 'unpinned'):
                build(base, output)
            self.assertFalse(output.exists())

    def test_existing_probe_or_changed_layout_cannot_be_appended_again(self):
        original = tree(probe=True)
        with mock.patch('marker_probe.BASE_SHA256', hashlib.sha256(original).hexdigest()):
            with self.assertRaises(ValueError):
                append_probe(original)

    def test_existing_output_and_symlink_paths_are_preserved(self):
        original = tree()
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory).resolve()
            base = root / 'base.dtb'
            base.write_bytes(original)
            output = root / 'output'
            output.mkdir()
            sentinel = output / 'user-file'
            sentinel.write_text('keep')
            with mock.patch('marker_probe.BASE_SHA256', hashlib.sha256(original).hexdigest()):
                with self.assertRaises(FileExistsError):
                    build(base, output)
                self.assertEqual(sentinel.read_text(), 'keep')
                link = root / 'link.dtb'
                link.symlink_to(base)
                with self.assertRaisesRegex(ValueError, 'symlink'):
                    build(link, root / 'new')
                alias = root / 'output-link'
                alias.symlink_to(output)
                with self.assertRaisesRegex(ValueError, 'symlink'):
                    build(base, alias)


if __name__ == '__main__':
    unittest.main()
