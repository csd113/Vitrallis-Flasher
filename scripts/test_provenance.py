import copy
import json
import pathlib
import tempfile
import unittest
from provenance import (
    ROOT,
    check_compatibility,
    check_evidence,
    check_inputs,
    check_kernel_config,
    check_lock,
    cross_checks,
    load_document,
    parse_kernel_config,
)

COMMIT = '0' * 40
OTHER = '1' * 40
DIGEST = 'a' * 64


def repository():
    return {
        'repository': 'https://github.com/example/project',
        'commit': COMMIT,
        'head': '2' * 40,
        'reviewed_files': {'file': DIGEST},
        'license': 'GPL-2.0-or-later',
        'redistributed': False,
        'tag': {'name': 'release-1', 'commit': COMMIT, 'type': 'lightweight'},
    }


def asset():
    return {
        'name': 'artifact',
        'url': 'https://github.com/example/project/releases/download/release-1/artifact',
        'size': 10,
        'sha256': DIGEST,
        'source_repository': 'https://github.com/example/project',
        'source_commit': COMMIT,
        'license': 'GPL-2.0-or-later',
        'reproducibility': 'PINNED PREBUILT BINARY',
        'status': 'VERIFIED BUT PREBUILT',
        'download_verified': True,
        'flash_approved': False,
        'provenance': 'release build log reviewed',
    }


def roles():
    result = []
    for role in ('uboot', 'kernel', 'dtb', 'recovery', 'spl-hynix', 'spl-toshiba', 'uboot-nand', 'rootfs'):
        result.append({
            'role': role,
            'artifact': 'artifact',
            'status': 'VERIFIED BUT PREBUILT',
            'purpose': 'role fixture',
        })
    return result


def lock():
    return {
        'schema_version': 2,
        'reviewed_on': '2026-09-25',
        'repositories': [repository()],
        'assets': [asset()],
        'physical_roles': roles(),
        'approved_physical_manifest_sha256': [],
    }


class LockContractTests(unittest.TestCase):
    def test_complete_repository_lock_passes(self):
        self.assertEqual(check_lock(lock()), [])

    def test_approved_artifact_needs_complete_provenance(self):
        for field in ('sha256', 'size', 'source_commit', 'license'):
            broken = lock()
            broken['assets'][0]['flash_approved'] = True
            del broken['assets'][0][field]
            self.assertTrue(check_lock(broken), field)
        still_mutable = lock()
        still_mutable['assets'][0]['flash_approved'] = True
        still_mutable['assets'][0]['reproducibility'] = 'PROVENANCE INCOMPLETE'
        self.assertTrue(check_lock(still_mutable))

    def test_approval_requires_an_approved_manifest_hash(self):
        broken = lock()
        broken['assets'][0]['flash_approved'] = True
        self.assertTrue(check_lock(broken))

    def test_mutable_source_revision_is_rejected(self):
        broken = lock()
        broken['repositories'][0]['commit'] = 'master'
        self.assertTrue(check_lock(broken))
        broken = lock()
        broken['assets'][0]['source_commit'] = 'main'
        self.assertTrue(check_lock(broken))
        broken = lock()
        broken['assets'][0]['source_commit'] = '3' * 40
        self.assertTrue(check_lock(broken))
        recorded_head = lock()
        recorded_head['assets'][0]['source_commit'] = '2' * 40
        self.assertEqual(check_lock(recorded_head), [])

    def test_tag_must_map_to_the_recorded_commit(self):
        broken = lock()
        broken['repositories'][0]['tag']['commit'] = OTHER
        self.assertTrue(check_lock(broken))
        broken = lock()
        broken['repositories'][0]['tag'] = {'name': 'latest', 'commit': COMMIT, 'type': 'lightweight'}
        self.assertTrue(check_lock(broken))

    def test_mutable_asset_url_is_rejected(self):
        for url in (
            'https://github.com/example/project/releases/latest/download/artifact',
            'https://github.com/example/project/releases/download/main/artifact',
        ):
            broken = lock()
            broken['assets'][0]['url'] = url
            self.assertTrue(check_lock(broken), url)

    def test_every_release_role_is_required(self):
        broken = lock()
        broken['physical_roles'] = broken['physical_roles'][:-1]
        self.assertTrue(check_lock(broken))
        broken = lock()
        broken['physical_roles'][1]['role'] = 'uboot'
        self.assertTrue(check_lock(broken))

    def test_unresolved_role_needs_an_explanation(self):
        broken = lock()
        broken['physical_roles'][2]['artifact'] = None
        broken['physical_roles'][2]['status'] = 'PARTIAL'
        self.assertTrue(check_lock(broken))
        broken['physical_roles'][2]['notes'] = 'requires hardware validation'
        self.assertEqual(check_lock(broken), [])

    def test_unresolved_role_cannot_be_verified(self):
        broken = lock()
        broken['physical_roles'][2]['artifact'] = None
        broken['physical_roles'][2]['status'] = 'VERIFIED'
        broken['physical_roles'][2]['notes'] = 'no input'
        self.assertTrue(check_lock(broken))

    def test_derived_artifact_must_reference_a_locked_asset(self):
        broken = lock()
        broken['derived_artifacts'] = [{
            'name': 'extracted',
            'from_asset': 'missing',
            'path': 'boot/extracted',
            'size': 10,
            'sha256': DIGEST,
            'status': 'PARTIAL',
            'notes': 'extracted from the archive',
        }]
        self.assertTrue(check_lock(broken))
        broken['derived_artifacts'][0]['from_asset'] = 'artifact'
        broken['physical_roles'][2]['artifact'] = 'extracted'
        broken['physical_roles'][2]['status'] = 'PARTIAL'
        broken['physical_roles'][2]['notes'] = 'derived from the locked archive'
        self.assertEqual(check_lock(broken), [])


class ImageInputLockTests(unittest.TestCase):
    def test_current_inputs_lock_passes(self):
        self.assertEqual(check_inputs(load_document(ROOT / 'images/inputs.lock.json')), [])

    def test_dependencies_need_immutable_revisions(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'lock.json'
            value = json.loads((ROOT / 'images/inputs.lock.json').read_text())
            value['dependencies']['kernel']['commit'] = 'main'
            path.write_text(json.dumps(value))
            self.assertTrue(check_inputs(load_document(path)))

    def test_selected_route_cross_checks_pass_offline(self):
        lock = load_document(ROOT / 'upstream-lock.json')
        inputs = load_document(ROOT / 'images/inputs.lock.json')
        _, requirements = check_evidence()
        self.assertEqual(cross_checks(lock, inputs, requirements), [])

    def test_selected_rootfs_derived_pins_inventory_and_tool_are_cross_checked(self):
        lock = load_document(ROOT / 'upstream-lock.json')
        inputs = json.loads((ROOT / 'images/inputs.lock.json').read_text())
        _, requirements = check_evidence()
        broken = json.loads(json.dumps(inputs))
        broken['rootfs']['sha256'] = '0' * 64
        self.assertTrue(cross_checks(lock, broken, requirements))
        broken = json.loads(json.dumps(inputs))
        broken['derived']['spl-hynix']['sha256'] = '0' * 64
        self.assertTrue(cross_checks(lock, broken, requirements))
        broken = json.loads(json.dumps(inputs))
        broken['package_lock_sha256'] = '0' * 64
        self.assertTrue(cross_checks(lock, broken, requirements))
        broken = json.loads(json.dumps(inputs))
        broken['spl_tool']['files']['nand-image-builder.c'] = '0' * 64
        self.assertTrue(cross_checks(lock, broken, requirements))
        broken = json.loads(json.dumps(inputs))
        broken['rootfs']['fallback']['sha256'] = '0' * 64
        self.assertTrue(cross_checks(lock, broken, requirements))

    def test_derived_tool_record_must_be_reviewed(self):
        lock = load_document(ROOT / 'upstream-lock.json')
        for artifact in lock['derived_artifacts']:
            if artifact['name'] == 'spl-hynix':
                artifact['tool']['sha256'] = '0' * 64
        self.assertTrue(check_lock(lock))


class CompatibilityTests(unittest.TestCase):
    def compatibility_fixture(self):
        return {
            'schema_version': 1,
            'kernel_release': '6.12.94+deb13-chip',
            'dtb': {'artifact': 'extracted', 'sha256': DIGEST},
            'overlay': {'artifact': 'artifact', 'sha256': DIGEST},
            'boot_script': {'artifact': 'artifact', 'sha256': DIGEST},
            'overlay_required_labels': ['pwm', 'i2c1'],
            'dtb_symbols': ['pwm', 'i2c1'],
            'overlay_selection': {'eeprom_magic': 'CHIP', 'pid_offset': 9, 'pocketchip_pid': [0, 1]},
        }

    def check(self, compatibility):
        lock_value = lock()
        lock_value['derived_artifacts'] = [{
            'name': 'extracted',
            'from_asset': 'artifact',
            'path': 'boot/extracted',
            'size': 10,
            'sha256': DIGEST,
            'status': 'VERIFIED',
            'notes': 'fixture',
        }]
        requirements = {
            'kernel_package': {'name': 'linux-image-6.12.94+deb13-chip'},
            'config': 'images/evidence/kernel-config-6.12.94+deb13-chip',
        }
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'compatibility.json'
            path.write_text(json.dumps(compatibility))
            return check_compatibility(lock_value, requirements, path)

    def test_complete_compatibility_evidence_passes(self):
        self.assertEqual(self.check(self.compatibility_fixture()), [])

    def test_missing_dtb_label_fails(self):
        broken = self.compatibility_fixture()
        broken['overlay_required_labels'].append('missing-label')
        self.assertTrue(self.check(broken))

    def test_wrong_artifact_hash_fails(self):
        broken = self.compatibility_fixture()
        broken['dtb']['sha256'] = 'b' * 64
        self.assertTrue(self.check(broken))

    def test_wrong_kernel_release_fails(self):
        broken = self.compatibility_fixture()
        broken['kernel_release'] = '6.12.107+deb13-chip'
        self.assertTrue(self.check(broken))


class EvidenceTests(unittest.TestCase):
    def test_checked_in_kernel_evidence_matches_the_lock(self):
        errors, requirements = check_evidence()
        self.assertEqual(errors, [])
        self.assertEqual(check_kernel_config(ROOT / requirements['config'], requirements), [])

    def test_changed_evidence_hash_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = pathlib.Path(directory)
            config = directory / 'config'
            config.write_text('CONFIG_MTD=y\n')
            import hashlib
            digest = hashlib.sha256(config.read_bytes()).hexdigest()
            requirements = {
                'schema_version': 1,
                'config': str(config),
                'config_sha256': digest,
                'required': {'CONFIG_MTD': 'y'},
                'kernel_package': {
                    'name': 'linux-image-fixture',
                    'version': '1',
                    'source_repository': 'https://github.com/example/project',
                    'source_commit': COMMIT,
                },
                'config_source': {
                    'asset': 'artifact',
                    'url': 'https://example.com/package.deb',
                    'path': 'boot/config',
                    'sha256': DIGEST,
                },
            }
            path = directory / 'requirements.json'
            path.write_text(json.dumps(requirements))
            self.assertEqual(check_evidence(path)[0], [])
            config.write_text('CONFIG_MTD=n\n')
            self.assertEqual(len(check_evidence(path)[0]), 2)

    def test_evidence_cross_references_disagreeing_lock(self):
        lock_value = lock()
        inputs = load_document(ROOT / 'images/inputs.lock.json')
        requirements = {
            'config_source': {
                'asset': 'artifact',
                'url': 'https://example.com/package.deb',
                'path': 'boot/config',
                'sha256': DIGEST,
            },
        }
        # The fixture asset's URL is not the one referenced by config_source.
        self.assertTrue(cross_checks(lock_value, inputs, requirements))

    def test_missing_and_demoted_symbols_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            config = pathlib.Path(directory) / 'config'
            config.write_text('CONFIG_MTD=y\nCONFIG_MTD_UBI=m\n')
            requirements = {'required': {'CONFIG_MTD': 'y', 'CONFIG_MTD_UBI': 'y', 'CONFIG_UBIFS_FS': 'y'}}
            failures = check_kernel_config(config, requirements)
            self.assertEqual(len(failures), 2)
            self.assertEqual(parse_kernel_config(config)['CONFIG_MTD_UBI'], 'm')

    def test_not_set_symbols_are_absent(self):
        with tempfile.TemporaryDirectory() as directory:
            config = pathlib.Path(directory) / 'config'
            config.write_text('# CONFIG_MTD is not set\n')
            self.assertEqual(parse_kernel_config(config)['CONFIG_MTD'], 'n')


if __name__ == '__main__':
    unittest.main()
