#!/usr/bin/env python3
"""Linux image-builder scaffold and deterministic archive/audit utilities.

Does not flash, publish, execute image hooks, or modify an upstream checkout.
Only `plan` and `check` are exposed until immutable build inputs are reviewed.
"""
import argparse
import hashlib
import json
import pathlib
import platform
import re
import sys

from storage import storage_policy

ROOT = pathlib.Path(__file__).resolve().parents[1]
PIN = "7584eab1aafb1667bd89ae210dcd641efc7cc5b5"
REPOSITORY = "https://github.com/nextthingco/x-chip-os"
DEPENDENCIES = ("bootloader", "kernel", "overlays", "package_repository", "recovery_installer")
SNAPSHOT = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")


def immutable_commit(value):
    return isinstance(value, str) and len(value) == 40 and all(c in "0123456789abcdef" for c in value)


def load_inputs(path):
    data = path.read_bytes()
    if len(data) > 65536:
        raise ValueError("input lock exceeds 64 KiB")
    lock = json.loads(data)
    expected = {"schema_version", "source_repository", "source_commit", "architecture", "distribution", "source_date_epoch", "dependencies", "container_digest", "debian_snapshot", "chip_snapshot_sha256", "package_lock_sha256", "vitrallis_bundle_sha256", "vitrallis_compatibility", "blockers", "vitrallis_blockers"}
    if not isinstance(lock, dict) or set(lock) != expected:
        raise ValueError("unexpected image input fields")
    if lock["schema_version"] != 2 or lock["source_repository"] != REPOSITORY or lock["source_commit"] != PIN:
        raise ValueError("unreviewed source revision")
    if lock["architecture"] != "armhf" or lock["distribution"] != "trixie":
        raise ValueError("unsupported image target")
    if type(lock["source_date_epoch"]) is not int or not 0 < lock["source_date_epoch"] < 2**32:
        raise ValueError("invalid deterministic timestamp")
    dependencies = lock["dependencies"]
    if not isinstance(dependencies, dict) or set(dependencies) != set(DEPENDENCIES):
        raise ValueError("unexpected dependency set")
    for name, record in dependencies.items():
        if not isinstance(record, dict) or set(record) != {"repository", "commit", "tag"}:
            raise ValueError("invalid dependency record: " + name)
        if not isinstance(record["repository"], str) or not record["repository"].startswith("https://"):
            raise ValueError("dependency needs an immutable HTTPS repository: " + name)
        if not immutable_commit(record["commit"]):
            raise ValueError("dependency needs an immutable commit: " + name)
        if record["tag"] is not None and (not isinstance(record["tag"], str) or not record["tag"] or len(record["tag"]) > 128):
            raise ValueError("invalid dependency tag: " + name)
    if lock["vitrallis_compatibility"] != "blocked":
        raise ValueError("this scaffold cannot certify Vitrallis compatibility")
    for field in ["chip_snapshot_sha256", "package_lock_sha256", "vitrallis_bundle_sha256"]:
        value = lock[field]
        if value is not None and (not isinstance(value, str) or len(value) != 64 or any(c not in "0123456789abcdef" for c in value)):
            raise ValueError("invalid checksum lock")
    if lock["container_digest"] is not None and (not isinstance(lock["container_digest"], str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", lock["container_digest"])):
        raise ValueError("container digest must be a sha256 digest or null")
    if lock["debian_snapshot"] is not None and (not isinstance(lock["debian_snapshot"], str) or not SNAPSHOT.match(lock["debian_snapshot"])):
        raise ValueError("Debian snapshot must be a UTC timestamp or null")
    for field in ("blockers", "vitrallis_blockers"):
        if not isinstance(lock[field], list) or not lock[field] or any(not isinstance(item, str) or not item or len(item) > 1024 for item in lock[field]):
            raise ValueError("invalid release blockers")
    return lock


PROFILES = ("stock", "vitrallis-default")


def missing_inputs(lock, profile):
    if profile not in PROFILES:
        raise ValueError("unknown installation profile")
    fields = ["container_digest", "debian_snapshot", "chip_snapshot_sha256", "package_lock_sha256"]
    if profile == "vitrallis-default":
        fields.append("vitrallis_bundle_sha256")
    return [field for field in fields if lock[field] is None]


def plan(lock, profile="stock"):
    """Commands are reviewable argv arrays, never interpreted or executed."""
    missing = missing_inputs(lock, profile)
    shell = profile == "vitrallis-default"
    desktop_steps = [
        {"action": "retain and audit PocketHome/Marshmallow, stock menu, Awesome startup and hardware configuration"},
    ]
    if shell:
        desktop_steps.extend([
            {"action": "verify a complete compatible Vitrallis bundle and matching installer/session/uninstaller helpers from one immutable release"},
            {"action": "install through the reviewed normal-user installer after runtime and session checks; retain PocketHome"},
            {"action": "back up the existing Awesome rc.lua; append only the exact optional startup block documented by Vitrallis, after existing startup; retain launch_home_screen()"},
            {"action": "validate automatic Vitrallis launch, failure fallback, Home bindings and removal of the exact startup block before approval"},
        ])
    else:
        desktop_steps.append({"action": "exclude Vitrallis binaries, installer, shortcuts and startup changes from the stock image"})
    return {
        "status": "blocked",
        "storage_policy": storage_policy(),
        "profile": profile,
        "default_desktop": "vitrallis" if shell else "pockethome",
        "install_vitrallis": shell,
        "preserve_pockethome": True,
        "preserves_existing_device_data": False,
        "upgrade_method": "FEL recovery reimage; no in-place Jessie-to-trixie migration",
        "missing": missing,
        "linux_only": True,
        "source": {"repository": REPOSITORY, "commit": PIN},
        "dependencies": lock["dependencies"],
        "source_date_epoch": lock["source_date_epoch"],
        "blockers": lock["blockers"] + (lock["vitrallis_blockers"] if shell else []),
        "steps": [
            {"action": "materialize reviewed source into a fresh Linux build workspace", "argv": ["git", "clone", "--no-checkout", REPOSITORY, "x-chip-os"]},
            {"action": "select immutable source", "argv": ["git", "-C", "x-chip-os", "checkout", "--detach", PIN]},
            {"action": "replace moving apt mirrors with signed, locked snapshots and inventory all packages"},
            {"action": "build pinned arm/v7 container using images/Dockerfile and generated apt.sources"},
            {"action": "run live-build in an isolated disposable Linux runner, using pocketchip/config", "argv": ["lb", "build"]},
            *desktop_steps,
            {"action": "review and apply the shared NAND-write policy in the disposable image build; inspect effective configuration and pass every storage release check"},
            {"action": "audit rootfs, session contract and separate kernel/DTB assets"},
            {"action": "produce deterministic rootfs with sorted paths, SOURCE_DATE_EPOCH and gzip mtime 0; build twice and compare SHA-256"},
            {"action": "produce unsigned candidate manifest and package inventory; never auto-approve or publish"},
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["plan", "check"])
    parser.add_argument("--profile", choices=PROFILES, default="stock")
    args = parser.parse_args()
    lock = load_inputs(ROOT / "images/inputs.lock.json")
    result = plan(lock, args.profile)
    result["lock_sha256"] = hashlib.sha256((ROOT / "images/inputs.lock.json").read_bytes()).hexdigest()
    if args.command == "check":
        result["host"] = platform.system()
    print(json.dumps(result, indent=2))
    return 2 if args.command == "check" else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, TypeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
