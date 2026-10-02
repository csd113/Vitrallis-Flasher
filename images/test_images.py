import json
import pathlib
import tempfile
import unittest
from build import load_inputs, plan, missing_inputs, ROOT
from repack import repack


class ImageScaffoldTests(unittest.TestCase):
    def test_selected_route_inputs_are_pinned(self):
        lock = load_inputs(ROOT / 'images/inputs.lock.json')
        stock = plan(lock)
        self.assertEqual(stock['status'], 'inputs-pinned')
        self.assertEqual(stock['missing'], [])
        self.assertEqual(stock['physical_approval'], 'blocked')
        self.assertEqual(stock['rootfs']['asset'], lock['rootfs']['asset'])
        self.assertEqual(stock['route'], 'consume-pinned-prebuilt-rootfs')

    def test_stock_does_not_depend_on_or_install_vitrallis(self):
        lock = load_inputs(ROOT / 'images/inputs.lock.json')
        stock = plan(lock)
        self.assertEqual(stock['profile'], 'stock')
        self.assertEqual(stock['default_desktop'], 'pockethome')
        self.assertFalse(stock['install_vitrallis'])
        self.assertTrue(stock['preserve_pockethome'])
        self.assertNotIn('vitrallis_bundle_sha256', stock['missing'])
        self.assertFalse(any(item in stock['blockers'] for item in lock['vitrallis_blockers']))
        shell = plan(lock, 'vitrallis-default')
        self.assertTrue(shell['install_vitrallis'])
        self.assertTrue(shell['preserve_pockethome'])
        self.assertEqual(shell['default_desktop'], 'vitrallis')
        self.assertIn('vitrallis_bundle_sha256', shell['missing'])
        self.assertEqual(shell['status'], 'blocked')
        with self.assertRaises(ValueError):
            missing_inputs(lock, 'unknown')

    def test_false_readiness_is_rejected(self):
        lock = json.loads((ROOT / 'images/inputs.lock.json').read_text())
        lock['vitrallis_compatibility'] = 'ready'
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'lock.json'
            path.write_text(json.dumps(lock))
            with self.assertRaises(ValueError):
                load_inputs(path)

    def test_dependency_pins_must_be_immutable(self):
        lock = json.loads((ROOT / 'images/inputs.lock.json').read_text())
        self.assertEqual(set(lock['dependencies']), {'bootloader', 'kernel', 'overlays', 'package_repository', 'recovery_installer'})
        for name in lock['dependencies']:
            self.assertRegex(lock['dependencies'][name]['commit'], r'^[0-9a-f]{40}$')
        lock['dependencies']['kernel']['commit'] = 'main'
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'lock.json'
            path.write_text(json.dumps(lock))
            with self.assertRaises(ValueError):
                load_inputs(path)

    def test_moving_container_and_snapshot_values_are_rejected(self):
        lock = json.loads((ROOT / 'images/inputs.lock.json').read_text())
        for field, value in (('container_digest', 'latest'), ('debian_snapshot', 'trixie')):
            lock[field] = value
            with tempfile.TemporaryDirectory() as directory:
                path = pathlib.Path(directory) / 'lock.json'
                path.write_text(json.dumps(lock))
                with self.assertRaises(ValueError):
                    load_inputs(path)
            lock[field] = None
        lock['container_digest'] = 'sha256:' + '0' * 64
        lock['debian_snapshot'] = '20260930T000000Z'
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'lock.json'
            path.write_text(json.dumps(lock))
            self.assertEqual(load_inputs(path)['debian_snapshot'], '20260930T000000Z')

    def test_selected_rootfs_and_derived_pins_are_required(self):
        lock = json.loads((ROOT / 'images/inputs.lock.json').read_text())
        lock['rootfs']['sha256'] = 'not-a-hash'
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'lock.json'
            path.write_text(json.dumps(lock))
            with self.assertRaises(ValueError):
                load_inputs(path)
        del lock['derived']['spl-hynix']
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'lock.json'
            path.write_text(json.dumps(lock))
            with self.assertRaises(ValueError):
                load_inputs(path)
        lock['derived']['spl-hynix'] = {'size': 1, 'sha256': 'x'}
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'lock.json'
            path.write_text(json.dumps(lock))
            with self.assertRaises(ValueError):
                load_inputs(path)

    def test_repack_is_deterministic_and_preserves_contents(self):
        import tarfile
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            source = base / 'source'
            source.mkdir()
            (source / 'hello').write_bytes(b'rootfs fixture')
            first, second = base / 'first.gz', base / 'second.gz'
            self.assertEqual(repack(source, first, 1700000000), repack(source, second, 1700000000))
            with tarfile.open(first) as archive:
                self.assertEqual(archive.extractfile('./hello').read(), b'rootfs fixture')
            with self.assertRaises(ValueError):
                repack(source, first, 1700000000)

    def test_output_inside_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory)
            with self.assertRaises(ValueError):
                repack(source, source / 'output.gz', 1700000000)


if __name__ == '__main__':
    unittest.main()
