import configparser
import hashlib
import unittest
from build import load_inputs, plan, ROOT
from storage import storage_policy


class StoragePolicyTests(unittest.TestCase):
    def test_both_desktops_get_the_same_nand_protection(self):
        lock = load_inputs(ROOT / 'images/inputs.lock.json')
        stock = plan(lock, 'stock')['storage_policy']
        shell = plan(lock, 'vitrallis-default')['storage_policy']
        self.assertEqual(stock, shell)
        self.assertEqual(stock['nand_backed_swap'], 'prohibited')
        self.assertIsNone(stock['zram']['writeback_device'])
        self.assertFalse(stock['zram']['disk_swap_fallback'])
        self.assertEqual(stock['status'], 'candidate-not-applied')
        self.assertEqual(plan(lock)['status'], 'inputs-pinned')

    def test_candidate_configs_match_policy_and_have_no_execution_directives(self):
        policy = storage_policy()
        parsed = {}
        for record in policy['configurations']:
            path = ROOT / record['source']
            raw = path.read_bytes()
            self.assertEqual(hashlib.sha256(raw).hexdigest(), record['sha256'])
            parser = configparser.ConfigParser(interpolation=None)
            parser.read_string(raw.decode())
            parsed[path.name] = parser
        zram = parsed['zram-generator.conf']
        self.assertEqual(zram.sections(), ['zram0'])
        self.assertEqual(dict(zram['zram0']), {
            'zram-size': 'min(ram / 4, 128)',
            'compression-algorithm': 'lz4',
            'swap-priority': '100',
        })
        journal = parsed['journald.conf']['Journal']
        self.assertEqual(journal['Storage'], 'volatile')
        self.assertEqual(journal['RuntimeMaxUse'], '8M')
        self.assertEqual(journal['ForwardToSyslog'], 'no')
        options = parsed['tmp.conf']['Mount']['Options'].split(',')
        self.assertIn('size=64M', options)
        self.assertIn('mode=1777', options)
        self.assertIn('nosuid', options)
        self.assertIn('nodev', options)
        self.assertNotIn('noexec', options)
