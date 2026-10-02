import hashlib
import io
import json
import pathlib
import tarfile
import tempfile
import unittest

import assemble
from build import ROOT

RELEASE = '6.12.107+deb13-chip'


def canonical(value):
    return (json.dumps(value, sort_keys=True, indent=2) + '\n').encode()


def scan_fixture():
    return {
        'files': {
            'etc/fstab': b'# UNCONFIGURED FSTAB FOR BASE SYSTEM\n',
            'etc/systemd/journald.conf': b'# commented\n[Journal]\n#Storage=auto\n',
            'usr/lib/systemd/system/tmp.mount': b'[Mount]\nOptions=mode=1777,size=50%%,nr_inodes=1m\n',
        },
        'system_links': [['etc/systemd/system/multi-user.target.wants/cron.service', '/usr/lib/systemd/system/cron.service']],
        'journal_dropins': 0,
        'zram_configs': 0,
    }


class AssemblePureTests(unittest.TestCase):
    def test_real_locks_load_and_resolve(self):
        inputs, lock = assemble.load_locks()
        sources = assemble.role_sources(lock, inputs)
        self.assertEqual(set(sources), set(assemble.physical.ROLES))
        needed = assemble.needed_assets(lock, inputs, sources)
        for name in ('pocketchip-rootfs-2026-09-23.tar.gz', 'sunxi-spl.bin', 'u-boot-dtb.bin', 'u-boot-sunxi-with-spl.bin', 'initrd.uimage'):
            self.assertIn(name, needed)
        self.assertEqual(sources['rootfs']['name'], inputs['rootfs']['asset'])

    def test_selected_rootfs_disagreement_is_rejected(self):
        inputs, lock = assemble.load_locks()
        broken = json.loads(json.dumps(inputs))
        broken['rootfs']['sha256'] = '0' * 64
        sources = assemble.role_sources(lock, broken)
        with self.assertRaises(assemble.BuildError):
            assemble.needed_assets(lock, broken, sources)

    def test_canonical_json_is_sorted_and_newline_terminated(self):
        data = assemble.canonical_json({'b': 1, 'a': [2]})
        self.assertTrue(data.endswith(b'\n'))
        self.assertEqual(data, b'{\n  "a": [\n    2\n  ],\n  "b": 1\n}\n')

    def test_package_inventory_counts_and_sorts(self):
        status = (
            b'Package: zebra\nStatus: install ok installed\nVersion: 2\nArchitecture: armhf\n\n'
            b'Package: alpha\nStatus: install ok installed\nVersion: 1\nArchitecture: all\n'
        )
        scan = {'files': {'var/lib/dpkg/status': status}, 'dpkg_info': {'zebra.list': 'a' * 64, 'zebra.md5sums': 'b' * 64, 'alpha.list': 'c' * 64, 'alpha.md5sums': 'd' * 64}}
        inventory, data = assemble.package_inventory(scan, {'rootfs': {'asset': 'x.tar.gz', 'sha256': 'e' * 64, 'kernel': {'release': RELEASE}}})
        self.assertEqual(inventory['package_count'], 2)
        self.assertEqual([record['name'] for record in inventory['packages']], ['alpha', 'zebra'])
        self.assertEqual(inventory['packages'][1]['list_sha256'], 'a' * 64)
        self.assertEqual(data, canonical(inventory))
        self.assertEqual(inventory['dpkg_status_sha256'], hashlib.sha256(status).hexdigest())

    def test_pad_uboot_matches_a_pinned_pin(self):
        payload = b'u-boot' * 100
        padded = payload + b'\x00' * (4194304 - len(payload))
        inputs = {'derived': {'u-boot-dtb-padded.bin': {'size': len(padded), 'sha256': hashlib.sha256(padded).hexdigest()}}}
        self.assertEqual(assemble.pad_uboot(payload, inputs), padded)
        inputs['derived']['u-boot-dtb-padded.bin']['sha256'] = '0' * 64
        with self.assertRaises(assemble.BuildError):
            assemble.pad_uboot(payload, inputs)
        with self.assertRaises(assemble.BuildError):
            assemble.pad_uboot(b'x' * (4194304 + 1), inputs)

    def test_storage_audit_accepts_clean_state_and_rejects_swap(self):
        audit = assemble.storage_audit(scan_fixture(), {'CONFIG_ZRAM': 'm', 'CONFIG_UBIFS_FS': 'y'})
        self.assertEqual(audit['nand_backed_swap'], 'prohibited')
        self.assertFalse(audit['applied'])
        self.assertEqual(audit['policy_sha256'], hashlib.sha256(assemble.canonical_json(assemble.storage.storage_policy())).hexdigest())
        broken = scan_fixture()
        broken['files']['etc/fstab'] = b'/dev/mtdblock2 none swap sw 0 0\n'
        with self.assertRaises(assemble.BuildError):
            assemble.storage_audit(broken, {})
        broken = scan_fixture()
        broken['system_links'].append(['etc/systemd/system/swap.target.wants/dev-mtdblock2.swap', '/dev/mtdblock2'])
        with self.assertRaises(assemble.BuildError):
            assemble.storage_audit(broken, {})
        broken = scan_fixture()
        broken['files']['etc/systemd/journald.conf'] = b'[Journal]\nStorage=persistent\n'
        with self.assertRaises(assemble.BuildError):
            assemble.storage_audit(broken, {})

    def test_compare_sets_detects_any_difference(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            for name in ('build-1', 'build-2'):
                (base / name).mkdir()
                (base / name / 'file').write_bytes(b'same')
            self.assertTrue(assemble.compare_sets(base / 'build-1', base / 'build-2')['identical'])
            (base / 'build-2/file').write_bytes(b'different')
            self.assertFalse(assemble.compare_sets(base / 'build-1', base / 'build-2')['identical'])
            (base / 'build-2/file').write_bytes(b'same')
            (base / 'build-2/extra').write_bytes(b'x')
            self.assertFalse(assemble.compare_sets(base / 'build-1', base / 'build-2')['identical'])

    def test_scan_rootfs_collects_required_state(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode='w:gz') as tar:
            def add(name, content, mode=0o644):
                info = tarfile.TarInfo(name)
                info.size = len(content)
                info.mode = mode
                tar.addfile(info, io.BytesIO(content))

            add('./etc/fstab', b'# none\n')
            add('./etc/systemd/journald.conf', b'# none\n')
            add('./usr/lib/systemd/system/tmp.mount', b'[Mount]\nOptions=mode=1777\n')
            add(f'./boot/vmlinuz-{RELEASE}', b'kernel')
            add(f'./boot/dtbs/{RELEASE}/sun5i-r8-chip.dtb', b'dtb')
            add('./boot/boot.scr', b'scr')
            add(f'./boot/config-{RELEASE}', b'CONFIG_MTD=y\n')
            add('./usr/lib/firmware/nextthingco/chip/early/x-chip-pocketchip.dtbo', b'dtbo')
            add('./var/lib/dpkg/status', b'Package: x\nStatus: install ok installed\nVersion: 1\nArchitecture: all\n')
            add('./var/lib/dpkg/info/x.list', b'/x\n')
            add('./var/lib/dpkg/info/x.md5sums', b'abc  /x\n')
            module_dir = tarfile.TarInfo(f'./usr/lib/modules/{RELEASE}')
            module_dir.type = tarfile.DIRTYPE
            module_dir.mode = 0o755
            tar.addfile(module_dir)
            link = tarfile.TarInfo('./etc/systemd/system/cron.service')
            link.type = tarfile.SYMTYPE
            link.linkname = '/usr/lib/systemd/system/cron.service'
            tar.addfile(link)
        archive = pathlib.Path(tempfile.mkdtemp()) / 'rootfs.tar.gz'
        archive.write_bytes(buffer.getvalue())
        try:
            scan = assemble.scan_rootfs(archive, RELEASE, min_members=1)
        finally:
            archive.unlink()
            archive.parent.rmdir()
        self.assertEqual(scan['files'][f'boot/vmlinuz-{RELEASE}'], b'kernel')
        self.assertEqual(scan['files']['var/lib/dpkg/status'].splitlines()[0], b'Package: x')
        self.assertEqual(scan['dpkg_info']['x.list'], hashlib.sha256(b'/x\n').hexdigest())
        self.assertIn(['etc/systemd/system/cron.service', '/usr/lib/systemd/system/cron.service'], scan['system_links'])
        self.assertTrue(scan['module_dir'])
        self.assertEqual(scan['types']['symlink'], 1)

    def test_scan_rootfs_rejects_unsafe_members(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode='w:gz') as tar:
            info = tarfile.TarInfo('../escape')
            info.size = 1
            tar.addfile(info, io.BytesIO(b'x'))
        archive = pathlib.Path(tempfile.mkdtemp()) / 'rootfs.tar.gz'
        archive.write_bytes(buffer.getvalue())
        try:
            with self.assertRaises(assemble.BuildError):
                assemble.scan_rootfs(archive, RELEASE, min_members=1)
        finally:
            archive.unlink()
            archive.parent.rmdir()

    def test_role_artifact_names_are_version_qualified(self):
        inputs, lock = assemble.load_locks()
        sources = assemble.role_sources(lock, inputs)
        names = assemble.role_artifact_names(inputs, sources)
        self.assertEqual(names['kernel'], f'vmlinuz-{RELEASE}')
        self.assertEqual(names['spl-hynix'], 'spl-hynix.nand')
        self.assertEqual(names['rootfs'], inputs['rootfs']['asset'])


if __name__ == '__main__':
    unittest.main()
