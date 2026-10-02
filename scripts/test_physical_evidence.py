"""Keep the measured SLC eraseblock distinction in deterministic fixtures."""

import hashlib
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "docs/evidence/batch3"


class RootfsBadBlockEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.correlation = json.loads(
            (EVIDENCE / "rootfs-bad-block-offset-correlation.json").read_text()
        )
        self.original = json.loads(
            (EVIDENCE / "mtd-ioctl-inventory.json").read_text()
        )["records"][4]
        recovery = json.loads(
            (EVIDENCE / "recovery-protected-chain-after-backup-erase-10.json").read_text()
        )
        log = recovery["records"][2]["response"]["Inventory"]["boot_log"]
        self.logged = {
            int(offset, 16)
            for offset in re.findall(r"nand_read_bbt: bad block at (0x[0-9a-f]+)", log)
        }

    def test_correlation_binds_exact_captures(self):
        for source in self.correlation["inputs"]:
            data = (ROOT / source["path"]).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), source["sha256"])

    def test_logical_blocks_map_to_all_logged_physical_blocks(self):
        info = self.original["mtd_info_user"]
        self.assertEqual(info["flags"], 0x4400)
        self.assertEqual(info["erasesize"], 2097152)
        offsets = [item["partition_offset"] for item in self.original["bad_blocks"]]
        translated = {16777216 + (offset // 2097152) * 4194304 for offset in offsets}
        self.assertEqual(len(offsets), 65)
        self.assertEqual(len(self.logged), 61)
        self.assertTrue(self.logged.issubset(translated))
        self.assertEqual(
            sorted(translated - self.logged),
            [8573157376, 8577351680, 8581545984, 8585740288],
        )
        rows = self.correlation["rows"]
        self.assertEqual([row["logical_partition_offset"] for row in rows], offsets)
        self.assertEqual({row["physical_offset"] for row in rows}, translated)
        for row in rows:
            self.assertEqual(row["kernel_logs_bad_block"], row["physical_offset"] in self.logged)

    def test_flat_partition_byte_translation_disagrees_with_hardware_log(self):
        flat = {
            16777216 + item["partition_offset"]
            for item in self.original["bad_blocks"]
        }
        self.assertFalse(self.logged.issubset(flat))
        self.assertNotEqual(flat, {row["physical_offset"] for row in self.correlation["rows"]})


if __name__ == "__main__":
    unittest.main()
