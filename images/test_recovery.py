import stat
import pathlib
import tempfile
import unittest

import recovery


class RecoveryArchiveTests(unittest.TestCase):
    def test_restoration_rejects_unpinned_short_oversized_and_symlink_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'candidate'
            for data in [b'', b'bad image', bytes(recovery.RESTORATION_BYTES), bytes(recovery.RESTORATION_BYTES + 1)]:
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    recovery.restoration_bytes(path)
            link = pathlib.Path(directory) / 'link'
            link.symlink_to(path)
            with self.assertRaises(ValueError):
                recovery.restoration_bytes(link)

    def archive(self, *entries):
        return b''.join(entries) + recovery.record('TRAILER!!!', b'', 0, 0)

    def test_newc_roundtrip_and_data_are_preserved(self):
        raw = recovery.record('usr/bin/tool', b'ELF-data', stat.S_IFREG | 0o755, 1)
        records = recovery.cpio_entries(self.archive(raw))
        self.assertEqual(records, [('usr/bin/tool', stat.S_IFREG | 0o755, b'ELF-data', raw)])

    def test_paths_duplicates_truncation_and_appended_payload_fail(self):
        good = recovery.record('init', b'boot', stat.S_IFREG | 0o755, 1)
        for archive in [good[:100], good[:-1], self.archive(good, good), self.archive(good) + b'payload']:
            with self.assertRaises(ValueError):
                recovery.cpio_entries(archive)
        for name in ['/etc/shadow', '../init', 'usr/../init', 'usr//init', 'init\0extra']:
            with self.assertRaises(ValueError):
                recovery.record(name, b'', stat.S_IFREG | 0o600, 1)

    def test_escaping_link_is_rejected_and_usr_merge_is_accepted(self):
        link = recovery.record('lib', b'usr/lib', stat.S_IFLNK | 0o777, 1)
        recovery.cpio_entries(self.archive(link))
        for target in ['../../outside', '/', '\0']:
            link = recovery.record('lib', target.encode(), stat.S_IFLNK | 0o777, 1)
            with self.assertRaises(ValueError):
                recovery.cpio_entries(self.archive(link))

    def test_declared_expansion_and_name_bounds_fail_before_read(self):
        raw = bytearray(recovery.record('init', b'', stat.S_IFREG | 0o755, 1))
        for index, value in [(6, recovery.MAX_FILE + 1), (11, 5000), (12, 1)]:
            bad = raw.copy()
            bad[6 + index * 8:14 + index * 8] = f'{value:08x}'.encode()
            with self.assertRaises(ValueError):
                recovery.cpio_entries(self.archive(bad))


if __name__ == '__main__':
    unittest.main()
