import gzip
import hashlib
import io
import json
import pathlib
import struct
import tarfile
import tempfile
import unittest

import fdt
import physical
import spl
from uimage import build as build_uimage

ROOT = pathlib.Path(__file__).resolve().parents[1]
COMMIT = '0' * 40
RELEASE = '6.12.107+deb13-chip'
OOB = {'hynix': 1664, 'toshiba': 1280}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return (json.dumps(value, sort_keys=True, indent=2) + '\n').encode()


def synthetic_spl():
    data = bytearray(16384)
    data[0:4] = (0xEA000016).to_bytes(4, 'little')
    data[4:12] = b'eGON.BT0'
    data[16:20] = (16384).to_bytes(4, 'little')
    data[20:24] = b'SPL\x02'
    for index in range(24, 16384):
        data[index] = (index * 17) & 0xFF
    return bytes(data)


def synthetic_image(oob, sunxi):
    page = 16384 + oob
    image = bytearray(spl.image_size(oob))
    keystream = spl.scrambler_stream(spl.ECC_REGION)
    for number in range(256):
        offset = number * page
        within = number % 64
        if within < 16:
            chunk = sunxi[within * 1024:(within + 1) * 1024]
        else:
            group = number // 64
            chunk = spl.stream(spl.PAD_LABELS[group], spl.PAD_BYTES)[(within - 16) * 1024:(within - 15) * 1024]
        image[offset:offset + 1024] = bytes(a ^ b for a, b in zip(chunk, keystream))
        image[offset + 16384:offset + 16386] = b'\xff\xff'
    return bytes(image)


class FdtFixture:
    def __init__(self):
        self.structure = bytearray()
        self.strings = bytearray()
        self.offsets = {}

    def string(self, name):
        if name not in self.offsets:
            self.offsets[name] = len(self.strings)
            self.strings += name.encode() + b'\x00'
        return self.offsets[name]

    def align(self):
        while len(self.structure) % 4:
            self.structure += b'\x00'

    def begin(self, name):
        self.structure += struct.pack('>I', 1) + name.encode() + b'\x00'
        self.align()

    def end(self):
        self.structure += struct.pack('>I', 2)

    def prop(self, name, value):
        self.structure += struct.pack('>III', 3, len(value), self.string(name)) + value
        self.align()

    def finish(self):
        self.structure += struct.pack('>I', 9)
        off_struct = 56
        off_strings = off_struct + len(self.structure)
        total = off_strings + len(self.strings)
        return (
            struct.pack('>10I', 0xD00DFEED, total, off_struct, off_strings, 40, 17, 16, 0, len(self.strings), len(self.structure))
            + b'\x00' * 16
            + bytes(self.structure)
            + bytes(self.strings)
        )


def dtb_blob(labels):
    builder = FdtFixture()
    builder.begin('')
    builder.begin('__symbols__')
    for label in labels:
        builder.prop(label, f'/soc/{label}'.encode() + b'\x00')
    builder.end()
    builder.end()
    return builder.finish()


def overlay_blob(labels, offsets=None):
    builder = FdtFixture()
    builder.begin('')
    builder.begin('__fixups__')
    for index, label in enumerate(labels):
        builder.prop(label, f'/soc/{label}:status:{index * 4}'.encode() + b'\x00')
    builder.end()
    builder.end()
    return builder.finish()


def rootfs_archive(kernel, dtb, overlay, status_version='6.12.107-1.31', kernel_bytes=None):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w:gz', format=tarfile.PAX_FORMAT) as tar:
        def add(name, content, mode=0o644):
            info = tarfile.TarInfo('./' + name)
            info.size = len(content)
            info.mode = mode
            tar.addfile(info, io.BytesIO(content))

        add(f'boot/vmlinuz-{RELEASE}', kernel_bytes if kernel_bytes is not None else kernel)
        add(f'boot/dtbs/{RELEASE}/sun5i-r8-chip.dtb', dtb)
        add('usr/lib/firmware/nextthingco/chip/early/x-chip-pocketchip.dtbo', overlay)
        status = (
            f'Package: linux-image-{RELEASE}\nStatus: install ok installed\nVersion: {status_version}\nArchitecture: armhf\n\n'
        ).encode()
        add('var/lib/dpkg/status', status)
        for index in range(30001):
            info = tarfile.TarInfo(f'./usr/share/filler/{index:05d}')
            info.type = tarfile.DIRTYPE
            tar.addfile(info)
    return buffer.getvalue()


class PhysicalFixture:
    """Build one valid physical artifact set plus matching lock documents."""

    def __init__(self, directory):
        self.directory = pathlib.Path(directory)
        (self.directory / 'artifacts').mkdir()
        (self.directory / 'inputs').mkdir()
        (self.directory / 'evidence').mkdir()
        self.sunxi = synthetic_spl()
        self.kernel = bytearray(4096)
        self.kernel[0x24:0x28] = (0x016F2818).to_bytes(4, 'little')
        self.kernel[0x28:0x2C] = (0x8000).to_bytes(4, 'little')
        self.kernel[0x2C:0x30] = (4096).to_bytes(4, 'little')
        self.kernel = bytes(self.kernel)
        self.dtb = dtb_blob(['pwm', 'i2c1'])
        self.overlay = overlay_blob(['pwm', 'i2c1'])
        self.uboot_dtb = bytes(index % 251 for index in range(2048))
        self.uimage = build_uimage({'type': 2}, self.uboot_dtb)
        self.uboot = bytearray(self.sunxi + bytes(0x8000 - len(self.sunxi)) + self.uimage)
        self.recovery = build_uimage({'type': 3}, b'initramfs-fixture')
        script = (
            f"setenv bootargs 'console=tty0'\n"
            f"ubifsload 0x42000000 /boot/vmlinuz-{RELEASE}\n"
            f"ubifsload 0x43000000 /boot/dtbs/{RELEASE}/sun5i-r8-chip.dtb\n"
            'bootz 0x42000000 - 0x43000000\n'
        ).encode()
        self.boot_scr = build_uimage({'type': 6}, script)
        self.uboot_nand = self.uboot_dtb + b'\x00' * (4194304 - len(self.uboot_dtb))
        self.rootfs = rootfs_archive(self.kernel, self.dtb, self.overlay)
        self.spls = {variant: synthetic_image(oob, self.sunxi) for variant, oob in OOB.items()}

        files = {
            'uboot': self.uboot,
            'uboot-nand': self.uboot_nand,
            'kernel': self.kernel,
            'dtb': self.dtb,
            'recovery': self.recovery,
            'spl-hynix': self.spls['hynix'],
            'spl-toshiba': self.spls['toshiba'],
            'rootfs': self.rootfs,
        }
        names = {
            'uboot': 'u-boot-sunxi-with-spl.bin',
            'uboot-nand': 'u-boot-dtb-padded.bin',
            'kernel': f'vmlinuz-{RELEASE}',
            'dtb': 'sun5i-r8-chip.dtb',
            'recovery': 'initrd.uimage',
            'spl-hynix': 'spl-hynix.nand',
            'spl-toshiba': 'spl-toshiba.nand',
            'rootfs': 'pocketchip-rootfs-2026-09-23.tar.gz',
        }
        self.records = {}
        for role, data in files.items():
            path = self.directory / 'artifacts' / names[role]
            path.write_bytes(data)
            self.records[role] = {'file': names[role], 'size': len(data), 'sha256': sha256(data)}
        (self.directory / 'inputs/sunxi-spl.bin').write_bytes(self.sunxi)
        (self.directory / 'inputs/u-boot-dtb.bin').write_bytes(self.uboot_dtb)
        (self.directory / 'inputs/boot.scr').write_bytes(self.boot_scr)
        (self.directory / 'inputs/x-chip-pocketchip.dtbo').write_bytes(self.overlay)
        inventory = {'schema_version': 1, 'package_count': 709, 'packages': [], 'rootfs': {'asset': names['rootfs']}}
        self.inventory_bytes = canonical(inventory)
        (self.directory / 'evidence/package-inventory.json').write_bytes(self.inventory_bytes)
        audit = {'nand_backed_swap': 'prohibited', 'applied': False}
        self.audit_bytes = canonical(audit)
        (self.directory / 'evidence/storage-audit.json').write_bytes(self.audit_bytes)
        self.inputs = {
            'schema_version': 3,
            'package_lock_sha256': sha256(self.inventory_bytes),
            'rootfs': {'asset': names['rootfs'], 'size': len(self.rootfs), 'sha256': sha256(self.rootfs)},
        }
        derived = [
            {
                'name': name,
                'from_asset': 'sunxi-spl.bin',
                'transform': 'fixture derivation',
                'size': self.records[role]['size'],
                'sha256': self.records[role]['sha256'],
                'status': 'VERIFIED',
                'notes': 'fixture',
            }
            for role, name in (
                ('uboot-nand', 'u-boot-dtb-padded.bin'),
                ('kernel', f'vmlinuz-{RELEASE}'),
                ('dtb', 'sun5i-r8-chip.dtb'),
                ('recovery', 'initrd.uimage'),
                ('spl-hynix', 'spl-hynix'),
                ('spl-toshiba', 'spl-toshiba'),
            )
        ]
        derived.insert(0, {
            'name': 'rootfs',
            'from_asset': 'sunxi-spl.bin',
            'transform': 'fixture derivation',
            'size': self.records['rootfs']['size'],
            'sha256': self.records['rootfs']['sha256'],
            'status': 'VERIFIED',
            'notes': 'fixture',
        })
        self.lock = {
            'approved_physical_manifest_sha256': [],
            'assets': [],
            'derived_artifacts': derived,
            'physical_roles': [
                {'role': 'rootfs', 'artifact': 'rootfs'},
                {'role': 'uboot', 'artifact': 'u-boot-sunxi-with-spl.bin'},
                {'role': 'uboot-nand', 'artifact': 'u-boot-dtb-padded.bin'},
                {'role': 'kernel', 'artifact': f'vmlinuz-{RELEASE}'},
                {'role': 'dtb', 'artifact': 'sun5i-r8-chip.dtb'},
                {'role': 'recovery', 'artifact': 'initrd.uimage'},
                {'role': 'spl-hynix', 'artifact': 'spl-hynix'},
                {'role': 'spl-toshiba', 'artifact': 'spl-toshiba'},
            ],
        }
        self.lock['assets'].append({
            'name': 'u-boot-sunxi-with-spl.bin',
            'size': self.records['uboot']['size'],
            'sha256': self.records['uboot']['sha256'],
        })
        self.lock['assets'].append({
            'name': 'rootfs',
            'size': self.records['rootfs']['size'],
            'sha256': self.records['rootfs']['sha256'],
        })
        self.manifest = self.make_manifest('hynix')

    def provenance(self, role):
        record = self.records[role]
        return {'kind': 'derived', 'locked_sha256': record['sha256'], 'source': 'fixture-source', 'commit': COMMIT}

    def input_meta(self, name, file, digest):
        return {
            'name': name,
            'file': file,
            'size': pathlib.Path(self.directory / file).stat().st_size,
            'sha256': digest,
            'locked_sha256': digest,
            'kind': 'asset',
            'source': 'fixture-source',
            'commit': COMMIT,
        }

    def make_manifest(self, variant, release_id='fixture-release'):
        oob = OOB[variant]
        artifacts = []
        for role, record in self.records.items():
            artifacts.append({
                'role': role,
                'file': 'artifacts/' + record['file'],
                'size': record['size'],
                'sha256': record['sha256'],
                'format': physical.ROLE_FORMATS[role],
                'provenance': self.provenance(role),
            })
        return {
            'schema_version': 1,
            'release': release_id,
            'board': 'pocketchip',
            'soc': 'allwinner-r8',
            'os': 'debian-13-trixie',
            'architecture': 'armhf',
            'installer_protocol': 1,
            'minimum_flasher': '0.1.0',
            'profile': 'stock',
            'vitrallis': 'not-installed',
            'nand_variant': variant,
            'layout': {
                'page_size': 16384,
                'eraseblock_size': 4194304,
                'oob_size': oob,
                'offsets': dict(physical.BOOT_OFFSETS, rootfs=physical.ROOTFS_OFFSET),
                'slots': {name: physical.BOOT_SLOT for name in physical.BOOT_OFFSETS},
            },
            'artifacts': artifacts,
            'inputs': [
                self.input_meta('sunxi-spl.bin', 'inputs/sunxi-spl.bin', sha256(self.sunxi)),
                self.input_meta('u-boot-dtb.bin', 'inputs/u-boot-dtb.bin', sha256(self.uboot_dtb)),
                self.input_meta('boot.scr', 'inputs/boot.scr', sha256(self.boot_scr)),
                self.input_meta('x-chip-pocketchip.dtbo', 'inputs/x-chip-pocketchip.dtbo', sha256(self.overlay)),
            ],
            'compatibility': {
                'kernel_release': RELEASE,
                'kernel_package': f'linux-image-{RELEASE}',
                'kernel_version': '6.12.107-1.31',
                'dtb_file': 'artifacts/sun5i-r8-chip.dtb',
                'dtb_sha256': sha256(self.dtb),
                'overlay_file': 'inputs/x-chip-pocketchip.dtbo',
                'overlay_sha256': sha256(self.overlay),
                'boot_script_file': 'inputs/boot.scr',
                'boot_script_sha256': sha256(self.boot_scr),
                'overlay_required_labels': ['pwm', 'i2c1'],
            },
            'storage': {
                'policy': 'candidate-not-applied',
                'policy_sha256': 'a' * 64,
                'nand_backed_swap': 'prohibited',
                'applied': False,
                'audit_file': 'evidence/storage-audit.json',
                'audit_sha256': sha256(self.audit_bytes),
            },
            'package_inventory': {
                'file': 'evidence/package-inventory.json',
                'sha256': sha256(self.inventory_bytes),
                'count': 709,
            },
            'toolchain': {
                'container_digest': 'sha256:' + 'b' * 64,
                'debian_snapshot': '20260930T000000Z',
                'spl_tool': {'repository': 'https://github.com/linux-sunxi/sunxi-tools', 'commit': COMMIT, 'files': {'nand-image-builder.c': 'c' * 64}},
            },
            'flash_approved': False,
            'approval': None,
        }


class PhysicalManifestTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._temporary = tempfile.TemporaryDirectory()
        cls.fixture = PhysicalFixture(cls._temporary.name)
        cls.manifest_path = cls.fixture.directory / 'manifest-hynix.json'
        cls.write_manifest(cls.fixture.manifest)

    @classmethod
    def tearDownClass(cls):
        cls._temporary.cleanup()

    @classmethod
    def write_manifest(cls, manifest):
        cls.manifest_path.write_bytes(canonical(manifest))
        return sha256(cls.manifest_path.read_bytes())

    def validate(self, manifest=None, inputs=None, lock=None):
        manifest = self.fixture.manifest if manifest is None else manifest
        self.write_manifest(manifest)
        return physical.validate_manifest(
            self.manifest_path,
            self.fixture.directory,
            self.fixture.inputs if inputs is None else inputs,
            self.fixture.lock if lock is None else lock,
        )

    def mutate(self, change):
        manifest = json.loads(canonical(self.fixture.manifest))
        change(manifest)
        return manifest

    def test_valid_fixture_passes_every_rule(self):
        self.assertEqual(self.validate(), [])

    def test_second_variant_passes(self):
        self.assertEqual(self.validate(self.fixture.make_manifest('toshiba')), [])

    def test_required_roles_and_duplicates(self):
        manifest = self.mutate(lambda value: value['artifacts'].pop())
        errors = self.validate(manifest)
        self.assertTrue(any('eight release roles' in error for error in errors))
        manifest = self.mutate(lambda value: value['artifacts'].__setitem__(1, dict(value['artifacts'][0])))
        self.assertTrue(self.validate(manifest))

    def test_installer_protocol_binding(self):
        manifest = self.mutate(lambda value: value.__setitem__('installer_protocol', 2))
        self.assertTrue(any('installer protocol' in error for error in self.validate(manifest)))
        manifest = self.mutate(lambda value: value.__setitem__('minimum_flasher', '9.9.9'))
        self.assertTrue(any('installer protocol' in error for error in self.validate(manifest)))

    def test_wrong_format_and_hash_are_rejected(self):
        manifest = self.mutate(lambda value: value['artifacts'][0].__setitem__('format', 'gztar'))
        self.assertTrue(any('format' in error for error in self.validate(manifest)))
        manifest = self.mutate(lambda value: value['artifacts'][0].__setitem__('sha256', '0' * 64))
        self.assertTrue(self.validate(manifest))

    def test_size_limits_and_exact_padded_uboot(self):
        oversize = self.fixture.directory / 'artifacts/oversize.dtb'
        oversize.write_bytes(b'\x00' * (physical.ROLE_LIMITS['dtb'] + 1))
        manifest = self.mutate(lambda value: value['artifacts'][3].update({'file': 'artifacts/oversize.dtb', 'size': oversize.stat().st_size, 'sha256': sha256(oversize.read_bytes())}))
        errors = self.validate(manifest)
        self.assertTrue(any('size limit' in error for error in errors))
        manifest = self.mutate(lambda value: value['artifacts'][1].__setitem__('size', 4194303))
        self.assertTrue(self.validate(manifest))

    def test_roles_must_be_distinct(self):
        manifest = self.mutate(lambda value: value['artifacts'][3].update({
            'file': value['artifacts'][2]['file'], 'size': value['artifacts'][2]['size'], 'sha256': value['artifacts'][2]['sha256']
        }))
        self.assertTrue(any('share artifact bytes' in error for error in self.validate(manifest)))

    def test_dtb_labels_and_overlay_fixups(self):
        weak_dtb = dtb_blob(['pwm'])
        (self.fixture.directory / 'artifacts/weak.dtb').write_bytes(weak_dtb)
        manifest = self.mutate(lambda value: value['artifacts'][3].update({
            'file': 'artifacts/weak.dtb', 'size': len(weak_dtb), 'sha256': sha256(weak_dtb)
        }))
        errors = self.validate(manifest)
        self.assertTrue(any('missing overlay labels' in error for error in errors))
        extra_overlay = overlay_blob(['pwm', 'i2c1', 'not-in-dtb'])
        (self.fixture.directory / 'inputs/extra.dtbo').write_bytes(extra_overlay)
        manifest = self.mutate(lambda value: value['inputs'][3].update({
            'file': 'inputs/extra.dtbo', 'size': len(extra_overlay), 'sha256': sha256(extra_overlay), 'locked_sha256': sha256(extra_overlay)
        }))
        manifest['compatibility']['overlay_sha256'] = sha256(extra_overlay)
        errors = self.validate(manifest)
        self.assertTrue(any('absent' in error for error in errors))

    def test_boot_script_compatibility(self):
        wrong = build_uimage({'type': 6}, f'ubifsload 0x42000000 /boot/vmlinuz-6.12.94+deb13-chip\nbootz 0x42000000 - 0x43000000\n'.encode())
        (self.fixture.directory / 'inputs/wrong.scr').write_bytes(wrong)
        manifest = self.mutate(lambda value: value['inputs'][2].update({
            'file': 'inputs/wrong.scr', 'size': len(wrong), 'sha256': sha256(wrong), 'locked_sha256': sha256(wrong)
        }))
        self.assertTrue(any('does not reference' in error for error in self.validate(manifest)))
        not_script = build_uimage({'type': 2}, b'kernel')
        (self.fixture.directory / 'inputs/not-script.scr').write_bytes(not_script)
        manifest = self.mutate(lambda value: value['inputs'][2].update({
            'file': 'inputs/not-script.scr', 'size': len(not_script), 'sha256': sha256(not_script), 'locked_sha256': sha256(not_script)
        }))
        self.assertTrue(any('wrong image type' in error for error in self.validate(manifest)))

    def test_kernel_and_rootfs_identity(self):
        other_kernel = bytes(bytearray(self.fixture.kernel)) + b'x'
        (self.fixture.directory / 'artifacts/other-kernel').write_bytes(other_kernel)
        manifest = self.mutate(lambda value: value['artifacts'][2].update({
            'file': 'artifacts/other-kernel', 'size': len(other_kernel), 'sha256': sha256(other_kernel)
        }))
        errors = self.validate(manifest)
        self.assertTrue(any('rootfs kernel bytes' in error for error in errors))
        bad_rootfs = rootfs_archive(self.fixture.kernel, self.fixture.dtb, self.fixture.overlay, status_version='6.12.94-1.29')
        (self.fixture.directory / 'artifacts/bad-rootfs.tar.gz').write_bytes(bad_rootfs)
        manifest = self.mutate(lambda value: value['artifacts'][7].update({
            'file': 'artifacts/bad-rootfs.tar.gz', 'size': len(bad_rootfs), 'sha256': sha256(bad_rootfs)
        }))
        errors = self.validate(manifest)
        self.assertTrue(any('dpkg kernel package' in error for error in errors))

    def test_spl_variant_and_geometry(self):
        wrong = synthetic_image(OOB['toshiba'], self.fixture.sunxi)
        (self.fixture.directory / 'artifacts/wrong-spl.nand').write_bytes(wrong)
        manifest = self.mutate(lambda value: value['artifacts'][5].update({
            'file': 'artifacts/wrong-spl.nand', 'size': len(wrong), 'sha256': sha256(wrong)
        }))
        self.assertTrue(self.validate(manifest))
        manifest = self.mutate(lambda value: value['layout'].__setitem__('oob_size', 1280))
        self.assertTrue(any('oob_size' in error for error in self.validate(manifest)))

    def test_layout_overlap_and_rootfs_offset(self):
        manifest = self.mutate(lambda value: value['layout']['offsets'].__setitem__('rootfs', 0x800000))
        errors = self.validate(manifest)
        self.assertTrue(any('rootfs must start at 0x1000000' in error for error in errors))
        self.assertTrue(any('overlap' in error for error in errors))
        manifest = self.mutate(lambda value: value['layout']['offsets'].__setitem__('uboot-nand', 0x400000))
        self.assertTrue(any('overlap' in error for error in self.validate(manifest)))

    def test_padded_uboot_must_begin_with_the_locked_payload(self):
        bad = bytearray(self.fixture.uboot_nand)
        bad[10] ^= 0xFF
        (self.fixture.directory / 'artifacts/bad-pad.bin').write_bytes(bytes(bad))
        manifest = self.mutate(lambda value: value['artifacts'][1].update({
            'file': 'artifacts/bad-pad.bin', 'size': len(bad), 'sha256': sha256(bytes(bad))
        }))
        self.assertTrue(any('u-boot-dtb.bin bytes' in error for error in self.validate(manifest)))

    def test_mutable_or_unpinned_inputs_are_rejected(self):
        manifest = self.mutate(lambda value: value['inputs'][0].__setitem__('commit', 'main'))
        self.assertTrue(any('immutable commit' in error for error in self.validate(manifest)))
        manifest = self.mutate(lambda value: value['inputs'][1].__setitem__('locked_sha256', 'd' * 64))
        self.assertTrue(any('not bound' in error for error in self.validate(manifest)))
        manifest = self.mutate(lambda value: value['toolchain'].__setitem__('container_digest', 'latest'))
        self.assertTrue(any('container_digest' in error for error in self.validate(manifest)))

    def test_package_inventory_and_storage_records(self):
        inputs = dict(self.fixture.inputs)
        inputs['package_lock_sha256'] = 'e' * 64
        errors = self.validate(inputs=inputs)
        self.assertTrue(any('package inventory disagrees' in error for error in errors))
        manifest = self.mutate(lambda value: value['storage'].__setitem__('applied', True))
        self.assertTrue(any('storage policy' in error for error in self.validate(manifest)))
        manifest = self.mutate(lambda value: value['storage'].__setitem__('audit_sha256', 'f' * 64))
        self.assertTrue(any('storage audit' in error for error in self.validate(manifest)))

    def test_rootfs_archive_safety(self):
        unsafe = io.BytesIO()
        with tarfile.open(fileobj=unsafe, mode='w:gz') as tar:
            info = tarfile.TarInfo('../escape')
            info.size = 1
            tar.addfile(info, io.BytesIO(b'x'))
        (self.fixture.directory / 'artifacts/unsafe.tar.gz').write_bytes(unsafe.getvalue())
        manifest = self.mutate(lambda value: value['artifacts'][7].update({
            'file': 'artifacts/unsafe.tar.gz', 'size': len(unsafe.getvalue()), 'sha256': sha256(unsafe.getvalue())
        }))
        self.assertTrue(any('unsafe path' in error for error in self.validate(manifest)))
        fifo = io.BytesIO()
        with tarfile.open(fileobj=fifo, mode='w:gz') as tar:
            info = tarfile.TarInfo('./fifo')
            info.type = tarfile.FIFOTYPE
            tar.addfile(info)
        (self.fixture.directory / 'artifacts/fifo.tar.gz').write_bytes(fifo.getvalue())
        manifest = self.mutate(lambda value: value['artifacts'][7].update({
            'file': 'artifacts/fifo.tar.gz', 'size': len(fifo.getvalue()), 'sha256': sha256(fifo.getvalue())
        }))
        self.assertTrue(any('disallowed member type' in error for error in self.validate(manifest)))

    def test_approval_must_bind_exact_hashes(self):
        manifest = self.mutate(lambda value: value.__setitem__('flash_approved', True))
        self.assertTrue(any('not in the approved list' in error for error in self.validate(manifest)))
        digest = self.write_manifest(manifest)
        lock = json.loads(canonical(self.fixture.lock))
        lock['approved_physical_manifest_sha256'] = [digest]
        self.assertTrue(any('approval record' in error for error in self.validate(manifest, lock=lock)))
        approval = {
            'manifest_sha256': digest,
            'artifact_hashes': {record['role']: record['sha256'] for record in manifest['artifacts']},
            'approved_on': '2026-10-01T00:00:00Z',
        }
        (self.fixture.directory / 'APPROVAL.json').write_bytes(canonical(approval))
        self.assertEqual(self.validate(manifest, lock=lock), [])
        approval['artifact_hashes']['dtb'] = 'f' * 64
        (self.fixture.directory / 'APPROVAL.json').write_bytes(canonical(approval))
        self.assertTrue(any('exact artifact hashes' in error for error in self.validate(manifest, lock=lock)))
        (self.fixture.directory / 'APPROVAL.json').unlink()

    def test_unapproved_manifest_cannot_carry_an_approval_block(self):
        manifest = self.mutate(lambda value: value.__setitem__('approval', {'manifest_sha256': '0' * 64}))
        self.assertTrue(any('must not carry an approval block' in error for error in self.validate(manifest)))


if __name__ == '__main__':
    unittest.main()
