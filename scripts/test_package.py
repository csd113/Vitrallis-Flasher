import os
import pathlib
import tempfile
import unittest
from unittest.mock import patch
import zipfile
from ci import publish_zip


class PackagingTests(unittest.TestCase):
    def test_old_license_timestamps_and_complete_zip(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            source = root / 'package'
            source.mkdir()
            license_file = source / 'LICENSE'
            license_file.write_bytes(b'fixture notice')
            os.utime(license_file, (1, 1))
            archive = root / 'result.zip'
            publish_zip(source, archive)
            with zipfile.ZipFile(archive) as bundle:
                self.assertIsNone(bundle.testzip())
                self.assertEqual(bundle.read('package/LICENSE'), b'fixture notice')

    def test_existing_archive_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            source = root / 'package'
            source.mkdir()
            archive = root / 'result.zip'
            archive.write_bytes(b'user data')
            with self.assertRaises(FileExistsError):
                publish_zip(source, archive)
            self.assertEqual(archive.read_bytes(), b'user data')
            self.assertEqual(list(root.glob('.package-*')), [])

    def test_failure_leaves_no_published_zip(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            source = root / 'package'
            source.mkdir()
            (source / 'fixture').write_text('fixture')
            archive = root / 'result.zip'
            with patch.object(zipfile.ZipFile, 'write', side_effect=OSError('fixture failure')):
                with self.assertRaises(OSError):
                    publish_zip(source, archive)
            self.assertFalse(archive.exists())
            self.assertEqual(list(root.glob('.package-*')), [])

class ArchitectureTests(unittest.TestCase):
    def test_foreign_binary_is_not_mislabeled(self):
        import struct
        from ci import verify_binary
        with tempfile.TemporaryDirectory() as temporary:
            path = pathlib.Path(temporary) / 'fixture'
            data = bytearray(64)
            data[:6] = b'\x7fELF\x02\x01'
            struct.pack_into('<H', data, 18, 183)
            path.write_bytes(data)
            verify_binary(path, 'aarch64-unknown-linux-gnu')
            with self.assertRaises(ValueError):
                verify_binary(path, 'x86_64-unknown-linux-gnu')
            with self.assertRaises(ValueError):
                verify_binary(path, 'aarch64-apple-darwin')


if __name__ == '__main__':
    unittest.main()
