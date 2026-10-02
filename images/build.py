#!/usr/bin/env python3
"""Linux image-builder planner and fail-closed input lock reader.

Batch 1 selected the pinned prebuilt rootfs route (route 1 in
`docs/image-build.md`): the rootfs is consumed byte-for-byte from the locked
upstream release, and `images/assemble.py` performs the real host-side assembly
from the same lock. This module only plans and validates; it never executes a
build step, touches a device, or runs apt.

Does not flash, publish, execute image hooks, or modify an upstream checkout.
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
PIN = "f9191c2914c94a3bfe2030556cadf7aaa3d9b4ff"
REPOSITORY = "https://github.com/nextthingco/x-chip-os"
DEPENDENCIES = ("bootloader", "kernel", "overlays", "package_repository", "recovery_installer")
DERIVED_PINS = ("spl-hynix", "spl-toshiba", "u-boot-dtb-padded.bin")
SNAPSHOT = re.compile(r"^\d{8}T\d{6}Z$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
CONTAINER_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
INPUT_FIELDS = {
    "schema_version",
    "source_repository",
    "source_commit",
    "architecture",
    "distribution",
    "source_date_epoch",
    "rootfs",
    "spl_tool",
    "derived",
    "dependencies",
    "container_digest",
    "debian_snapshot",
    "chip_snapshot_sha256",
    "package_lock_sha256",
    "vitrallis_bundle_sha256",
    "vitrallis_compatibility",
    "blockers",
    "vitrallis_blockers",
}


class InputLockError(ValueError):
    """The image input lock does not satisfy the reviewed schema."""


def immutable_commit(value):
    return isinstance(value, str) and bool(COMMIT.match(value))


def _positive_int(value):
    return type(value) is int and value > 0


def _https(value):
    return isinstance(value, str) and value.startswith("https://") and len(value) <= 2048


def _sha256(value):
    return isinstance(value, str) and bool(SHA256.match(value))


def load_inputs(path):
    data = pathlib.Path(path).read_bytes()
    if len(data) > 65536:
        raise InputLockError("input lock exceeds 64 KiB")
    lock = json.loads(data)
    if not isinstance(lock, dict) or set(lock) != INPUT_FIELDS:
        raise InputLockError("unexpected image input fields")
    if lock["schema_version"] != 3:
        raise InputLockError("image input lock must be schema 3")
    if lock["source_repository"] != REPOSITORY or lock["source_commit"] != PIN:
        raise InputLockError("unreviewed source revision")
    if lock["architecture"] != "armhf" or lock["distribution"] != "trixie":
        raise InputLockError("unsupported image target")
    if type(lock["source_date_epoch"]) is not int or not 0 < lock["source_date_epoch"] < 2**32:
        raise InputLockError("invalid deterministic timestamp")
    _check_rootfs(lock["rootfs"])
    _check_spl_tool(lock["spl_tool"])
    _check_dependencies(lock["dependencies"])
    _check_derived(lock["derived"])
    if not CONTAINER_DIGEST.match(str(lock["container_digest"])):
        raise InputLockError("container digest must be a pinned sha256 digest")
    if not SNAPSHOT.match(str(lock["debian_snapshot"])):
        raise InputLockError("Debian snapshot must be a UTC timestamp")
    if lock["chip_snapshot_sha256"] is not None and not _sha256(lock["chip_snapshot_sha256"]):
        raise InputLockError("invalid CHIP snapshot checksum")
    if not _sha256(lock["package_lock_sha256"]):
        raise InputLockError("package inventory checksum must be pinned")
    if lock["vitrallis_bundle_sha256"] is not None and not _sha256(lock["vitrallis_bundle_sha256"]):
        raise InputLockError("invalid Vitrallis bundle checksum")
    if lock["vitrallis_compatibility"] != "blocked":
        raise InputLockError("this build cannot certify Vitrallis compatibility")
    for field in ("blockers", "vitrallis_blockers"):
        value = lock[field]
        if not isinstance(value, list) or not value or any(not isinstance(item, str) or not item or len(item) > 1024 for item in value):
            raise InputLockError("invalid release blockers")
    return lock


def _check_rootfs(record):
    expected = {"asset", "tag", "url", "size", "sha256", "kernel", "fallback"}
    if not isinstance(record, dict) or set(record) != expected:
        raise InputLockError("invalid selected-rootfs record")
    if not isinstance(record["asset"], str) or not record["asset"]:
        raise InputLockError("selected rootfs needs an asset name")
    if not isinstance(record["tag"], str) or not record["tag"]:
        raise InputLockError("selected rootfs needs an immutable release tag")
    if not _https(record["url"]) or not _positive_int(record["size"]) or not _sha256(record["sha256"]):
        raise InputLockError("selected rootfs needs a pinned HTTPS URL, size and SHA-256")
    kernel = record["kernel"]
    if not isinstance(kernel, dict) or set(kernel) != {"release", "package", "version"}:
        raise InputLockError("selected rootfs needs a kernel record")
    if not all(isinstance(kernel[field], str) and kernel[field] for field in kernel):
        raise InputLockError("kernel release, package and version must be explicit")
    fallback = record["fallback"]
    if not isinstance(fallback, dict) or set(fallback) != {"asset", "tag", "url", "size", "sha256"}:
        raise InputLockError("a pinned fallback rootfs must be documented")
    if not _https(fallback["url"]) or not _positive_int(fallback["size"]) or not _sha256(fallback["sha256"]):
        raise InputLockError("fallback rootfs needs a pinned HTTPS URL, size and SHA-256")


def _check_spl_tool(tool):
    expected = {"repository", "commit", "files", "version_header", "compile_flags"}
    if not isinstance(tool, dict) or set(tool) != expected:
        raise InputLockError("invalid SPL tool record")
    if not _https(tool["repository"]) or not immutable_commit(tool["commit"]):
        raise InputLockError("SPL tool needs an immutable repository and commit")
    files = tool["files"]
    if not isinstance(files, dict) or not files or any(not isinstance(name, str) or not name or not _sha256(digest) for name, digest in files.items()):
        raise InputLockError("SPL tool needs reviewed file hashes")
    if not isinstance(tool["version_header"], str) or "VERSION" not in tool["version_header"]:
        raise InputLockError("SPL tool needs a pinned generated version header")
    if not isinstance(tool["compile_flags"], list) or not tool["compile_flags"] or any(not isinstance(flag, str) or not flag.startswith("-") for flag in tool["compile_flags"]):
        raise InputLockError("SPL tool needs explicit compile flags")


def _check_dependencies(dependencies):
    if not isinstance(dependencies, dict) or set(dependencies) != set(DEPENDENCIES):
        raise InputLockError("unexpected dependency set")
    for name, record in dependencies.items():
        if not isinstance(record, dict) or set(record) != {"repository", "commit", "tag"}:
            raise InputLockError("invalid dependency record: " + name)
        if not _https(record["repository"]):
            raise InputLockError("dependency needs an immutable HTTPS repository: " + name)
        if not immutable_commit(record["commit"]):
            raise InputLockError("dependency needs an immutable commit: " + name)
        if record["tag"] is not None and (not isinstance(record["tag"], str) or not record["tag"] or len(record["tag"]) > 128):
            raise InputLockError("invalid dependency tag: " + name)


def _check_derived(derived):
    if not isinstance(derived, dict) or set(derived) != set(DERIVED_PINS):
        raise InputLockError("unexpected derived artifact pins")
    for name, record in derived.items():
        if not isinstance(record, dict) or set(record) != {"size", "sha256"}:
            raise InputLockError("invalid derived artifact pin: " + name)
        if not _positive_int(record["size"]) or not _sha256(record["sha256"]):
            raise InputLockError("derived artifact needs an exact size and SHA-256: " + name)


PROFILES = ("stock", "vitrallis-default")


def missing_inputs(lock, profile):
    if profile not in PROFILES:
        raise InputLockError("unknown installation profile")
    missing = []
    if lock["vitrallis_bundle_sha256"] is None and profile == "vitrallis-default":
        missing.append("vitrallis_bundle_sha256")
    return missing


def plan(lock, profile="stock"):
    """Commands are reviewable argv arrays, never interpreted or executed."""
    missing = missing_inputs(lock, profile)
    shell = profile == "vitrallis-default"
    rootfs = lock["rootfs"]
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
        "status": "blocked" if missing else "inputs-pinned",
        "storage_policy": storage_policy(),
        "profile": profile,
        "route": "consume-pinned-prebuilt-rootfs",
        "rootfs": {
            "asset": rootfs["asset"],
            "tag": rootfs["tag"],
            "sha256": rootfs["sha256"],
            "kernel": rootfs["kernel"],
            "fallback_asset": rootfs["fallback"]["asset"],
        },
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
        "container_digest": lock["container_digest"],
        "debian_snapshot": lock["debian_snapshot"],
        "package_lock_sha256": lock["package_lock_sha256"],
        "blockers": lock["blockers"] + (lock["vitrallis_blockers"] if shell else []),
        "physical_approval": "blocked",
        "steps": [
            {"action": "verify every locked asset by size and SHA-256 into a private asset directory", "argv": [sys.executable, "images/assemble.py", "fetch-assets", "--assets", "ASSETS"]},
            {"action": "stream-scan the pinned rootfs; extract and hash kernel, DTB, overlay, boot script, config and dpkg state"},
            {"action": "build the pinned sunxi-nand-image-builder from the locked source in the digest-pinned container with no network"},
            {"action": "generate deterministic Hynix and Toshiba SPLs twice each and require byte-identical, structurally validated output"},
            {"action": "zero-pad the locked u-boot-dtb.bin to its 4 MiB erase-block slot"},
            {"action": "deterministically repack the pinned rootfs archive without extraction and require equivalent member metadata/content"},
            {"action": "audit the actual rootfs storage state (no NAND swap, no zram manager, bounded volatile logs/tmp) and record it"},
            {"action": "check kernel/DTB/overlay/boot-script/rootfs compatibility and the recorded package inventory"},
            *desktop_steps,
            {"action": "emit a strict physical manifest per NAND variant; never auto-approve or publish"},
            {"action": "build the complete artifact set twice from clean state and compare every byte"},
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
    return 2 if args.command == "check" and result["missing"] else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, TypeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
