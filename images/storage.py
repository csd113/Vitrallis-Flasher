"""Reviewed candidate storage policy, shared by both desktop image plans.

This module describes requirements and hashes configuration drafts. It never changes
mounts, enables swap, installs packages or executes a candidate configuration.
"""
import hashlib
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent
CANDIDATES = {
    'zram-generator.conf': '/etc/systemd/zram-generator.conf',
    'journald.conf': '/etc/systemd/journald.conf.d/60-chip-storage.conf',
    'tmp.conf': '/etc/systemd/system/tmp.mount.d/60-chip-storage.conf',
}


def storage_policy():
    configurations = []
    for name, destination in CANDIDATES.items():
        source = ROOT / 'storage-candidates' / name
        if source.is_symlink() or not source.is_file():
            raise ValueError('missing or unsafe candidate storage configuration')
        with source.open('rb') as stream:
            data = stream.read(16385)
        if not data or len(data) > 16384:
            raise ValueError('invalid candidate storage configuration size')
        configurations.append({
            'source': 'images/storage-candidates/' + name,
            'destination': destination,
            'sha256': hashlib.sha256(data).hexdigest(),
        })
    return {
        'status': 'candidate-not-applied',
        'nand_backed_swap': 'prohibited',
        'zram': {
            'status': 'requires-package-lock-and-device-validation',
            'logical_size_mib': 'min(usable_ram_mib / 4, 128)',
            'compression': 'lz4',
            'writeback_device': None,
            'disk_swap_fallback': False,
        },
        'journal': {'storage': 'volatile', 'runtime_max_use_mib': 8, 'lost_on_reboot': True},
        'tmp': {'filesystem': 'tmpfs', 'max_size_mib': 64},
        'preserve': [
            'PocketHome and optional Vitrallis desktop behavior',
            'UBI wear leveling, scrubbing and ubihealthd health state',
            'filesystem synchronization and crash recovery',
            'package/security update capability, dpkg state and backups',
            'clock recovery state and time synchronization',
            'persistent /var/tmp and user data',
        ],
        'configurations': configurations,
        'release_checks': [
            'Audit live-build swap settings, fstab, swap units, generators, init/cron hooks and boot arguments; allow no NAND-backed swap or zram writeback.',
            'Verify /proc/swaps and each zram backing_dev after boot, desktop startup, memory pressure and reboot; do not mask swap.target or treat swappiness=0 as swap disablement.',
            'Lock exactly one zram manager if adopted; measure CPU, compression, memory pressure and OOM behavior for both desktops; never fall back to NAND swap.',
            'Verify effective journald drop-ins/namespaces and other file loggers; volatile journald alone does not eliminate application log writes.',
            'Verify the effective /tmp mount cap and application/update compatibility; keep package staging and large downloads out of bounded tmpfs.',
            'Measure plocate/timer and application cache writes before opting out; preserve NAND health, clocks and update services.',
            'Record idle and workload write activity plus UBI erase-counter distribution on both NAND parts; do not claim a measured lifetime gain without hardware data.',
        ],
    }
