import gzip
import hashlib
import io
import pathlib
import tarfile
import tempfile
import unittest

from repack import RepackError, repack_archive

LONG_NAME = './' + ('long/' * 25) + 'file.txt'


def build_source(path, with_special=True):
    with tarfile.open(str(path), 'w|gz', format=tarfile.PAX_FORMAT) as tar:
        root = tarfile.TarInfo('.')
        root.type = tarfile.DIRTYPE
        root.mode = 0o755
        root.uid = root.gid = 0
        tar.addfile(root)
        directory = tarfile.TarInfo('./etc')
        directory.type = tarfile.DIRTYPE
        directory.mode = 0o755
        tar.addfile(directory)
        for name, content in (('./etc/a.conf', b'alpha'), ('./etc/b.conf', b'beta')):
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = 0o640
            info.uid, info.gid = 1000, 1000
            info.uname, info.gname = 'user', 'group'
            tar.addfile(info, io.BytesIO(content))
        long_info = tarfile.TarInfo(LONG_NAME)
        long_info.size = 5
        tar.addfile(long_info, io.BytesIO(b'longy'))
        symlink = tarfile.TarInfo('./etc/a.link')
        symlink.type = tarfile.SYMTYPE
        symlink.linkname = '../a.conf'
        tar.addfile(symlink)
        hardlink = tarfile.TarInfo('./etc/hard.conf')
        hardlink.type = tarfile.LNKTYPE
        hardlink.linkname = './etc/a.conf'
        tar.addfile(hardlink)
        if with_special:
            device = tarfile.TarInfo('./dev-node')
            device.type = tarfile.CHRTYPE
            device.devmajor = 1
            device.devminor = 3
            device.mode = 0o666
            tar.addfile(device)


def members(path):
    with tarfile.open(path, 'r|gz') as archive:
        return [
            {
                'name': member.name,
                'type': member.type,
                'mode': member.mode,
                'uid': member.uid,
                'gid': member.gid,
                'mtime': member.mtime,
                'uname': member.uname,
                'gname': member.gname,
                'linkname': member.linkname,
                'devmajor': member.devmajor,
                'devminor': member.devminor,
                'size': member.size,
                'sha256': hashlib.sha256(archive.extractfile(member).read()).hexdigest() if member.isfile() else None,
            }
            for member in archive
        ]


class ArchiveRepackTests(unittest.TestCase):
    def test_repack_is_deterministic_and_normalizes_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            source = base / 'source.tar.gz'
            build_source(source)
            first, second = base / 'first.tar.gz', base / 'second.tar.gz'
            summary = repack_archive(source, first, 1700000000)
            self.assertEqual(summary, repack_archive(source, second, 1700000000))
            self.assertEqual(hashlib.sha256(first.read_bytes()).hexdigest(), summary['sha256'])
            self.assertEqual(first.read_bytes()[4:8], b'\x00\x00\x00\x00')
            self.assertEqual(summary['members'], 8)
            self.assertEqual(summary['bytes'], 14)
            before, after = members(source), members(first)
            self.assertEqual([record['name'] for record in before], [record['name'] for record in after])
            for record in after:
                self.assertEqual(record['mtime'], 1700000000)
                self.assertEqual(record['uname'], '')
                self.assertEqual(record['gname'], '')
            self.assertEqual([record['type'] for record in before], [record['type'] for record in after])
            self.assertEqual([record['sha256'] for record in before], [record['sha256'] for record in after])
            self.assertEqual([record['mode'] for record in before], [record['mode'] for record in after])
            self.assertEqual([record['uid'] for record in before], [record['uid'] for record in after])
            self.assertEqual([record['devmajor'] for record in before], [record['devmajor'] for record in after])
            self.assertEqual([record['linkname'] for record in before], [record['linkname'] for record in after])

    def test_unsafe_paths_and_special_files_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            source = base / 'source.tar.gz'
            with tarfile.open(str(source), 'w|gz') as tar:
                info = tarfile.TarInfo('../escape')
                info.size = 1
                tar.addfile(info, io.BytesIO(b'x'))
            with self.assertRaises(RepackError):
                repack_archive(source, base / 'out.tar.gz', 1)
            with tarfile.open(str(source), 'w|gz') as tar:
                fifo = tarfile.TarInfo('./fifo')
                fifo.type = tarfile.FIFOTYPE
                tar.addfile(fifo)
            with self.assertRaises(RepackError):
                repack_archive(source, base / 'out.tar.gz', 1)

    def test_unsupported_pax_metadata_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            source = base / 'source.tar.gz'
            with tarfile.open(str(source), 'w|gz', format=tarfile.PAX_FORMAT) as tar:
                info = tarfile.TarInfo('./file')
                info.size = 1
                info.pax_headers = {'SCHILY.xattr.user.test': '1'}
                tar.addfile(info, io.BytesIO(b'x'))
            with self.assertRaises(RepackError):
                repack_archive(source, base / 'out.tar.gz', 1)

    def test_duplicate_members_and_bad_arguments_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            source = base / 'source.tar.gz'
            with tarfile.open(str(source), 'w|gz') as tar:
                for _ in range(2):
                    info = tarfile.TarInfo('./same')
                    info.size = 1
                    tar.addfile(info, io.BytesIO(b'x'))
            with self.assertRaises(RepackError):
                repack_archive(source, base / 'out.tar.gz', 1)
            build_source(base / 'clean.tar.gz')
            with self.assertRaises(RepackError):
                repack_archive(base / 'clean.tar.gz', base / 'out.tar.gz', 0)
            with self.assertRaises(RepackError):
                repack_archive(base / 'clean.tar.gz', base / 'clean.tar.gz', 1)
            repack_archive(base / 'clean.tar.gz', base / 'out.tar.gz', 1)
            with self.assertRaises(RepackError):
                repack_archive(base / 'clean.tar.gz', base / 'out.tar.gz', 1)


if __name__ == '__main__':
    unittest.main()
