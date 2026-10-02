#!/usr/bin/env python3
"""Strict semantic validation of PocketCHIP physical flash manifests.

A physical manifest is the approval-gating document for one build variant. It
names the exact artifact files and hashes for all eight release roles and the
non-role build inputs, and records the compatibility, storage, package and
toolchain facts established by `images/assemble.py`.

Validation never relies on file names as identity. Every rule below is checked
against file bytes, parsed structures and the recorded locks:

* required roles present and unique;
* expected artifact type/format (eGON SPL, FEL bootloader, padded U-Boot,
  ARM zImage, FDT, overlay FDT, uImage script, uImage initrd, gzip tar rootfs);
* exact hashes against both the file bytes and the locked provenance;
* exact/maximum size constraints per role and per NAND geometry;
* role artifacts that must be byte-distinct;
* DTB symbol / overlay external-fixup compatibility;
* boot-script compatibility with the kernel/DTB/rootfs release;
* kernel/rootfs compatibility (kernel bytes and dpkg record inside the archive);
* correct SPL variant and structure for the declared OOB geometry;
* U-Boot within its 4 MiB slot and no NAND-layout overlap;
* rootfs starts at 0x1000000;
* no mutable or unpinned input anywhere in the document;
* approval bound to the exact manifest and artifact hashes.

`validate_manifest` returns a list of human-readable errors; an empty list
means the document satisfies every rule. No rule here writes to a device.
"""
import gzip
import hashlib
import json
import pathlib
import re
import tarfile

import fdt
import repack
import spl
from uimage import UIMAGE_TYPE_SCRIPT, UImageError, parse as parse_uimage, script_text

SCHEMA_VERSION = 1
ROLES = (
    "uboot",
    "uboot-nand",
    "kernel",
    "dtb",
    "recovery",
    "spl-hynix",
    "spl-toshiba",
    "rootfs",
)
ROLE_LIMITS = {
    "rootfs": 2 * 1024 * 1024 * 1024,
    "recovery": 40 * 1024 * 1024,
    "kernel": 16 * 1024 * 1024,
    "dtb": 1024 * 1024,
    "spl-hynix": 8 * 1024 * 1024,
    "spl-toshiba": 8 * 1024 * 1024,
    "uboot": 4 * 1024 * 1024,
    "uboot-nand": 4 * 1024 * 1024,
}
ROLE_FORMATS = {
    "uboot": "fel-bootloader",
    "uboot-nand": "padded-uboot",
    "kernel": "arm-zimage",
    "dtb": "fdt",
    "recovery": "uimage",
    "spl-hynix": "nand-spl",
    "spl-toshiba": "nand-spl",
    "rootfs": "gztar",
}
BOOT_OFFSETS = {
    "spl": 0x0,
    "spl-backup": 0x400000,
    "uboot-nand": 0x800000,
    "uboot-backup": 0xC00000,
}
BOOT_SLOT = 0x400000
ROOTFS_OFFSET = 0x1000000
PAGE_SIZE = 16384
ERASEBLOCK_SIZE = 4194304
OOB_BY_VARIANT = {"hynix": 1664, "toshiba": 1280}
SPL_ROLE_BY_VARIANT = {"hynix": "spl-hynix", "toshiba": "spl-toshiba"}
SHA256 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
IDENTIFIER = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,95}$")
SNAPSHOT = re.compile(r"^\d{8}T\d{6}Z$")
UTC_TIMESTAMP = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
CONTAINER_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
UIMAGE_MAGIC = 0x27051956
UIMAGE_TYPE_SCRIPT = 6
FDT_MAGIC = 0xD00DFEED
ZIMAGE_MAGIC = 0x016F2818
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_MEMBER_COUNT = 200000
ALLOWED_MEMBER_TYPES = {
    tarfile.DIRTYPE,
    tarfile.REGTYPE,
    tarfile.SYMTYPE,
    tarfile.LNKTYPE,
    tarfile.CHRTYPE,
    tarfile.BLKTYPE,
}


class ManifestError(ValueError):
    """The physical manifest or its artifact set is not valid."""


def sha256_file(path):
    with pathlib.Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def load_json(path, limit=MAX_MANIFEST_BYTES):
    raw = pathlib.Path(path).read_bytes()
    if len(raw) > limit:
        raise ManifestError("document exceeds the size bound: " + str(path))
    return json.loads(raw)


def _https_pinned(value):
    if not isinstance(value, str) or not value.startswith("https://") or len(value) > 2048:
        return False
    lowered = value.lower()
    return not any(segment in lowered for segment in ("/latest/", "/latest", "/master/", "/main/"))


def _check_distinct(errors, where, artifacts):
    seen = {}
    for record in artifacts:
        digest = record.get("sha256")
        role = record.get("role")
        if digest in seen:
            errors.append(f"{where}: roles {seen[digest]} and {role} share artifact bytes")
        else:
            seen[digest] = role


def _validate_artifact_common(errors, where, record, root):
    role = record.get("role")
    if role not in ROLES:
        errors.append(f"{where}: unknown role {role!r}")
        return None
    if record.get("format") != ROLE_FORMATS[role]:
        errors.append(f"{where}: role {role} declares the wrong format")
    digest = record.get("sha256")
    if not isinstance(digest, str) or not SHA256.match(digest):
        errors.append(f"{where}: role {role} needs a lowercase SHA-256")
        return None
    size = record.get("size")
    if type(size) is not int or size <= 0:
        errors.append(f"{where}: role {role} needs a positive size")
        return None
    if size > ROLE_LIMITS[role]:
        errors.append(f"{where}: role {role} exceeds its size limit")
    path = _resolve(root, record.get("file"))
    if path is None:
        errors.append(f"{where}: role {role} has an unsafe or missing file")
        return None
    if not path.is_file() or path.is_symlink():
        errors.append(f"{where}: role {role} file is missing or not regular")
        return None
    actual_size = path.stat().st_size
    actual_digest = sha256_file(path)
    if actual_size != size or actual_digest != digest:
        errors.append(f"{where}: role {role} file does not match its recorded size/hash")
    provenance = record.get("provenance")
    if not isinstance(provenance, dict):
        errors.append(f"{where}: role {role} needs provenance")
        return path
    if provenance.get("locked_sha256") != digest:
        errors.append(f"{where}: role {role} provenance is not bound to the artifact hash")
    if not isinstance(provenance.get("source"), str) or not provenance["source"]:
        errors.append(f"{where}: role {role} provenance needs a source")
    if not COMMIT.match(str(provenance.get("commit", ""))):
        errors.append(f"{where}: role {role} provenance needs an immutable commit")
    return path


def _resolve(root, relative):
    if not isinstance(relative, str) or not relative or relative.startswith("/") or "\\" in relative:
        return None
    parts = relative.split("/")
    if any(part in ("", ".", "..") for part in parts):
        return None
    candidate = pathlib.Path(root) / relative
    try:
        resolved = candidate.resolve(strict=False)
    except OSError:
        return None
    if pathlib.Path(root).resolve() not in resolved.parents:
        return None
    return candidate


def _validate_layout(errors, manifest):
    layout = manifest.get("layout")
    if not isinstance(layout, dict):
        errors.append("layout must be an object")
        return None
    if layout.get("page_size") != PAGE_SIZE:
        errors.append("layout.page_size must be the 16384-byte CHIP page")
    if layout.get("eraseblock_size") != ERASEBLOCK_SIZE:
        errors.append("layout.eraseblock_size must be the 4 MiB CHIP erase block")
    variant = manifest.get("nand_variant")
    if variant not in OOB_BY_VARIANT:
        errors.append("nand_variant must be hynix or toshiba")
        return None
    if layout.get("oob_size") != OOB_BY_VARIANT[variant]:
        errors.append("layout.oob_size disagrees with the NAND variant")
    offsets = layout.get("offsets")
    slots = layout.get("slots")
    if not isinstance(offsets, dict) or not isinstance(slots, dict):
        errors.append("layout needs offsets and slots objects")
        return None
    for name, expected in BOOT_OFFSETS.items():
        if offsets.get(name) != expected:
            errors.append(f"layout.offsets.{name} must be {expected:#x}")
        if slots.get(name) != BOOT_SLOT:
            errors.append(f"layout.slots.{name} must be {BOOT_SLOT:#x}")
    if offsets.get("rootfs") != ROOTFS_OFFSET:
        errors.append("rootfs must start at 0x1000000")
    ordered = sorted(
        ((offset, name) for name, offset in offsets.items() if name != "rootfs"),
        key=lambda item: item[0],
    )
    end = 0
    for offset, name in ordered:
        if offset < end:
            errors.append(f"layout slots overlap at {name}")
        end = offset + slots.get(name, 0)
    if offsets.get("rootfs", 0) < end:
        errors.append("rootfs overlaps the bootloader slots")
    return variant


def _validate_rootfs(errors, path, manifest, kernel_path, dtb_path, overlay_path):
    """Stream the rootfs archive: safety, kernel/DTB/overlay identity, dpkg record."""
    kernel_sha = None
    compatibility = manifest.get("compatibility", {})
    for record in manifest.get("artifacts", []):
        if isinstance(record, dict) and record.get("role") == "kernel":
            kernel_sha = record.get("sha256")
    try:
        with tarfile.open(path, "r|gz") as archive:
            members = 0
            found_kernel = found_dtb = found_overlay = False
            status = None
            for member in archive:
                members += 1
                if members > MAX_MEMBER_COUNT:
                    errors.append("rootfs archive has excessive members")
                    return
                try:
                    repack._safe_name(member.name)
                except repack.RepackError:
                    errors.append("rootfs archive contains an unsafe path")
                    return
                if member.type not in ALLOWED_MEMBER_TYPES:
                    errors.append("rootfs archive contains a disallowed member type")
                    return
                if not member.isfile():
                    continue
                name = member.name[2:] if member.name.startswith("./") else member.name
                wanted = None
                if dtb_path and name == dtb_path:
                    wanted = "dtb"
                elif overlay_path and name == overlay_path:
                    wanted = "overlay"
                elif kernel_path and name == kernel_path:
                    wanted = "kernel"
                elif name == "var/lib/dpkg/status":
                    wanted = "status"
                if wanted is None:
                    continue
                stream = archive.extractfile(member)
                if stream is None:
                    errors.append("rootfs member is unreadable")
                    return
                digest = hashlib.sha256()
                payload = bytearray()
                while True:
                    chunk = stream.read(1024 * 1024)
                    if not chunk:
                        break
                    digest.update(chunk)
                    if wanted in ("dtb", "overlay") or len(payload) <= 4 * 1024 * 1024:
                        payload += chunk
                if wanted == "kernel":
                    found_kernel = True
                    if digest.hexdigest() != kernel_sha:
                        errors.append("rootfs kernel bytes disagree with the kernel artifact")
                elif wanted == "dtb":
                    found_dtb = True
                    if digest.hexdigest() != compatibility.get("dtb_sha256"):
                        errors.append("rootfs DTB bytes disagree with the dtb artifact")
                elif wanted == "overlay":
                    found_overlay = True
                    if digest.hexdigest() != compatibility.get("overlay_sha256"):
                        errors.append("rootfs overlay bytes disagree with the overlay input")
                else:
                    status = bytes(payload)
            if members < 30000:
                errors.append("rootfs archive has implausibly few members")
            for found, label in (
                (found_kernel, "kernel"),
                (found_dtb, "DTB"),
                (found_overlay, "overlay"),
            ):
                if not found:
                    errors.append(f"rootfs archive does not contain the expected {label} path")
            if status is None:
                errors.append("rootfs archive has no dpkg status database")
                return
            _check_dpkg(errors, status, manifest["compatibility"])
    except (tarfile.TarError, gzip.BadGzipFile, EOFError, OSError):
        errors.append("rootfs is not a readable gzip tar archive")
        return


def _check_dpkg(errors, status, compatibility):
    package = compatibility.get("kernel_package")
    version = compatibility.get("kernel_version")
    record = None
    for block in status.decode("utf-8", "replace").split("\n\n"):
        fields = {}
        for line in block.splitlines():
            if ": " in line:
                key, _, value = line.partition(": ")
                fields[key] = value
        if fields.get("Package") == package:
            record = fields
            break
    if record is None:
        errors.append("rootfs dpkg status does not record the kernel package")
    elif record.get("Version") != version or record.get("Status") != "install ok installed":
        errors.append("rootfs dpkg kernel package version/status disagrees with the manifest")


def _check_approval(errors, manifest, digest, root, approved):
    approval_path = pathlib.Path(root) / "APPROVAL.json"
    approval = None
    if approval_path.is_file() and not approval_path.is_symlink():
        try:
            approval = json.loads(approval_path.read_text())
        except (OSError, ValueError):
            errors.append("APPROVAL.json is unreadable")
    if manifest.get("flash_approved"):
        if digest not in approved:
            errors.append("manifest claims approval but its hash is not in the approved list")
        if not isinstance(approval, dict):
            errors.append("approved manifest requires an approval record")
            return
        if approval.get("manifest_sha256") != digest:
            errors.append("approval record is not bound to the manifest hash")
        expected = {record["role"]: record["sha256"] for record in manifest.get("artifacts", [])}
        if approval.get("artifact_hashes") != expected:
            errors.append("approval record is not bound to the exact artifact hashes")
        if not UTC_TIMESTAMP.match(str(approval.get("approved_on", ""))):
            errors.append("approval record needs a UTC approval timestamp")
    else:
        if manifest.get("approval") is not None:
            errors.append("unapproved manifest must not carry an approval block")
        if digest in approved:
            errors.append("manifest is listed as approved but does not claim approval")


def validate_manifest(manifest_path, set_root, inputs, lock):
    """Validate one physical manifest against its artifact set and locks.

    `inputs` is `images/inputs.lock.json` and `lock` is `upstream-lock.json`.
    Returns a list of error strings; empty means every rule passed.
    """
    errors = []
    manifest_path = pathlib.Path(manifest_path)
    raw = manifest_path.read_bytes()
    if len(raw) > MAX_MANIFEST_BYTES:
        raise ManifestError("manifest exceeds the size bound")
    digest = hashlib.sha256(raw).hexdigest()
    try:
        manifest = json.loads(raw)
    except ValueError:
        return ["manifest is not valid JSON"]
    if not isinstance(manifest, dict):
        return ["manifest must be a JSON object"]
    if manifest.get("schema_version") != SCHEMA_VERSION:
        errors.append("unsupported physical manifest schema")
    release = manifest.get("release")
    if not isinstance(release, str) or not IDENTIFIER.match(release):
        errors.append("release must be a bounded identifier")
    if (
        manifest.get("board") != "pocketchip"
        or manifest.get("soc") != "allwinner-r8"
        or manifest.get("os") != "debian-13-trixie"
        or manifest.get("architecture") != "armhf"
    ):
        errors.append("manifest targets an unsupported board/SoC/OS")
    if manifest.get("installer_protocol") != 1 or manifest.get("minimum_flasher") != "0.1.0":
        errors.append("manifest must bind installer protocol 1 to flasher 0.1.0")
    if manifest.get("profile") != "stock" or manifest.get("vitrallis") != "not-installed":
        errors.append("Batch 1 manifests are stock-profile only")
    variant = _validate_layout(errors, manifest)
    compatibility = manifest.get("compatibility")
    if not isinstance(compatibility, dict):
        errors.append("manifest needs compatibility evidence")
        compatibility = {}
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, list):
        errors.append("artifacts must be a list")
        return errors
    roles = [record.get("role") for record in artifacts if isinstance(record, dict)]
    if sorted(roles) != sorted(ROLES):
        errors.append("manifest must list exactly the eight release roles once each")
    if len(set(roles)) != len(roles):
        errors.append("manifest lists a role more than once")
    _check_distinct(errors, "artifacts", artifacts)
    resolved = {}
    for index, record in enumerate(artifacts):
        if not isinstance(record, dict):
            errors.append(f"artifacts[{index}] must be an object")
            continue
        path = _validate_artifact_common(errors, f"artifacts[{index}]", record, set_root)
        if path is not None:
            resolved[record["role"]] = (record, path)
    if not isinstance(inputs, dict) or inputs.get("schema_version") != 3:
        errors.append("the image input lock must be schema 3")
    approved = lock.get("approved_physical_manifest_sha256", []) if isinstance(lock, dict) else []
    if not isinstance(approved, list) or any(not SHA256.match(str(item)) for item in approved):
        errors.append("approved_physical_manifest_sha256 must be a list of SHA-256 values")
        approved = []
    _check_approval(errors, manifest, digest, set_root, approved)

    # Locks must agree with every role's exact hash and size.
    role_locks = {entry["role"]: entry for entry in lock.get("physical_roles", []) if isinstance(entry, dict)}
    assets = {entry["name"]: entry for entry in lock.get("assets", []) if isinstance(entry, dict)}
    derived = {entry["name"]: entry for entry in lock.get("derived_artifacts", []) if isinstance(entry, dict)}
    for role, (record, _) in resolved.items():
        locked = role_locks.get(role, {}).get("artifact")
        source = derived.get(locked) or assets.get(locked)
        if source is None:
            errors.append(f"role {role} has no locked artifact record")
            continue
        if source.get("sha256") != record.get("sha256") or source.get("size") != record.get("size"):
            errors.append(f"role {role} disagrees with the provenance lock")
    if isinstance(inputs, dict) and inputs.get("rootfs"):
        rootfs_input = inputs["rootfs"]
        rootfs_record = resolved.get("rootfs", ({}, None))[0]
        if rootfs_input.get("sha256") != rootfs_record.get("sha256") or rootfs_input.get("size") != rootfs_record.get("size"):
            errors.append("rootfs role disagrees with the image input lock")

    # Inputs: identity, immutability and exact hash agreement.
    inputs_field = manifest.get("inputs")
    if not isinstance(inputs_field, list) or not inputs_field:
        errors.append("manifest needs a non-empty inputs list")
        inputs_field = []
    input_paths = {}
    for index, record in enumerate(inputs_field):
        if not isinstance(record, dict):
            errors.append(f"inputs[{index}] must be an object")
            continue
        name = record.get("name")
        if not isinstance(name, str) or not IDENTIFIER.match(name) or name in input_paths:
            errors.append(f"inputs[{index}] needs a unique bounded name")
            continue
        path = _resolve(set_root, record.get("file"))
        if path is None or not path.is_file() or path.is_symlink():
            errors.append(f"inputs[{index}] file is unsafe or missing")
            continue
        input_paths[name] = path
        size = record.get("size")
        digest = record.get("sha256")
        if type(size) is not int or size <= 0 or not isinstance(digest, str) or not SHA256.match(digest):
            errors.append(f"inputs[{index}] needs an exact size and SHA-256")
            continue
        if path.stat().st_size != size or sha256_file(path) != digest:
            errors.append(f"inputs[{index}] file does not match its recorded hash")
        if record.get("locked_sha256") != digest:
            errors.append(f"inputs[{index}] is not bound to the locked hash")
        if record.get("kind") not in ("asset", "derived", "tool"):
            errors.append(f"inputs[{index}] needs a kind")
        if not isinstance(record.get("source"), str) or not record["source"]:
            errors.append(f"inputs[{index}] needs a source")
        if not COMMIT.match(str(record.get("commit", ""))):
            errors.append(f"inputs[{index}] needs an immutable commit")
        for key in ("url", "repository"):
            if key in record and not _https_pinned(record[key]):
                errors.append(f"inputs[{index}].{key} is mutable or not HTTPS")

    # Format-specific validation.
    sunxi_spl = _read_input(input_paths, "sunxi-spl.bin", errors)
    uboot_dtb = _read_input(input_paths, "u-boot-dtb.bin", errors)
    boot_scr = _read_input(input_paths, "boot.scr", errors)
    overlay = _read_input(input_paths, "x-chip-pocketchip.dtbo", errors)
    dtb_record, dtb_path = resolved.get("dtb", ({}, None))
    if dtb_path is not None:
        try:
            dtb = fdt.Fdt(dtb_path.read_bytes())
        except (OSError, fdt.FdtError) as error:
            errors.append("dtb is not a valid flattened device tree: " + str(error))
            dtb = None
    else:
        dtb = None
    kernel_record, kernel_path = resolved.get("kernel", ({}, None))
    if kernel_path is not None:
        head = kernel_path.read_bytes()[:0x30]
        if len(head) < 0x28 or int.from_bytes(head[0x24:0x28], "little") != ZIMAGE_MAGIC:
            errors.append("kernel is not an ARM zImage")
    recovery_record, recovery_path = resolved.get("recovery", ({}, None))
    if recovery_path is not None:
        try:
            _, payload = parse_uimage(recovery_path.read_bytes()[: 64 + ROLE_LIMITS["recovery"]])
            if not payload:
                errors.append("recovery uImage has an empty payload")
        except UImageError as error:
            errors.append("recovery is not a valid uImage: " + str(error))
    uboot_record, uboot_path = resolved.get("uboot", ({}, None))
    if uboot_path is not None:
        _check_fel_bootloader(errors, uboot_path, uboot_dtb)
    uboot_nand_record, uboot_nand_path = resolved.get("uboot-nand", ({}, None))
    if uboot_nand_path is not None:
        _check_padded_uboot(errors, uboot_nand_path, uboot_dtb)
    variant = manifest.get("nand_variant")
    if variant in SPL_ROLE_BY_VARIANT:
        for role, oob in OOB_BY_VARIANT.items():
            _, path = resolved.get(f"spl-{role}", ({}, None))
            if path is None or sunxi_spl is None:
                continue
            try:
                spl.validate_image(path.read_bytes(), oob, sunxi_spl)
            except spl.SplError as error:
                errors.append(f"spl-{role} fails its NAND structure check: {error}")
        expected_size = spl.image_size(OOB_BY_VARIANT[variant])
        record, _ = resolved.get(SPL_ROLE_BY_VARIANT[variant], ({}, None))
        if record.get("size") != expected_size:
            errors.append("the variant SPL does not match the declared OOB geometry")
    if boot_scr is not None and kernel_record:
        _check_boot_script(errors, boot_scr, manifest, kernel_record, dtb_record)
    if dtb is not None and overlay is not None and kernel_record:
        _check_dtb_overlay(errors, manifest, dtb, overlay)
    rootfs_record, rootfs_path = resolved.get("rootfs", ({}, None))
    if rootfs_path is not None and kernel_record:
        release = compatibility.get("kernel_release")
        if isinstance(release, str):
            _validate_rootfs(
                errors,
                rootfs_path,
                manifest,
                f"boot/vmlinuz-{release}",
                f"boot/dtbs/{release}/sun5i-r8-chip.dtb",
                "usr/lib/firmware/nextthingco/chip/early/x-chip-pocketchip.dtbo",
            )

    # Toolchain, storage and inventory records must be complete and immutable.
    toolchain = manifest.get("toolchain")
    if not isinstance(toolchain, dict):
        errors.append("manifest needs toolchain provenance")
    else:
        if not CONTAINER_DIGEST.match(str(toolchain.get("container_digest", ""))):
            errors.append("toolchain.container_digest must be an image digest")
        if not SNAPSHOT.match(str(toolchain.get("debian_snapshot", ""))):
            errors.append("toolchain.debian_snapshot must be a UTC timestamp")
        tool = toolchain.get("spl_tool")
        if not isinstance(tool, dict) or not COMMIT.match(str(tool.get("commit", ""))):
            errors.append("toolchain.spl_tool needs an immutable commit")
        elif not isinstance(tool.get("files"), dict) or not tool["files"]:
            errors.append("toolchain.spl_tool needs file hashes")
        else:
            for name, digest in tool["files"].items():
                if not isinstance(name, str) or not SHA256.match(str(digest)):
                    errors.append("toolchain.spl_tool file hashes are malformed")
    storage = manifest.get("storage")
    if not isinstance(storage, dict):
        errors.append("manifest needs a storage policy record")
    else:
        if storage.get("nand_backed_swap") != "prohibited" or storage.get("applied") is not False:
            errors.append("storage policy must prohibit NAND swap and remain unapplied")
        if not SHA256.match(str(storage.get("policy_sha256", ""))):
            errors.append("storage.policy_sha256 is missing")
        audit = _resolve(set_root, storage.get("audit_file"))
        if audit is None or not audit.is_file() or sha256_file(audit) != storage.get("audit_sha256"):
            errors.append("storage audit file is missing or does not match its hash")
    inventory = manifest.get("package_inventory")
    if not isinstance(inventory, dict):
        errors.append("manifest needs a package inventory record")
    else:
        if type(inventory.get("count")) is not int or inventory["count"] < 300:
            errors.append("package inventory count is implausible")
        if not SHA256.match(str(inventory.get("sha256", ""))):
            errors.append("package inventory needs a SHA-256")
        elif isinstance(inputs, dict) and inventory.get("sha256") != inputs.get("package_lock_sha256"):
            errors.append("package inventory disagrees with the image input lock")
        inventory_file = _resolve(set_root, inventory.get("file"))
        if inventory_file is None or not inventory_file.is_file() or sha256_file(inventory_file) != inventory.get("sha256"):
            errors.append("package inventory file is missing or does not match its hash")
    compatibility = manifest.get("compatibility")
    if not isinstance(compatibility, dict):
        errors.append("manifest needs compatibility evidence")
    return errors


def _read_input(paths, name, errors):
    path = paths.get(name)
    if path is None:
        errors.append(f"manifest is missing the required input {name}")
        return None
    try:
        return path.read_bytes()
    except OSError:
        errors.append(f"input {name} is unreadable")
        return None


def _check_fel_bootloader(errors, path, uboot_dtb):
    data = path.read_bytes()
    if len(data) < 0x8000 + 64:
        errors.append("uboot is too small to embed an SPL and U-Boot payload")
        return
    if data[4:12] != b"eGON.BT0":
        errors.append("uboot lacks the eGON SPL header")
    if int.from_bytes(data[16:20], "little") != 16384:
        errors.append("uboot eGON header length is not 16 KiB")
    try:
        _, payload = parse_uimage(data[0x8000:])
    except UImageError as error:
        errors.append("uboot does not embed a valid U-Boot uImage: " + str(error))
        return
    if uboot_dtb is not None and hashlib.sha256(payload).digest() != hashlib.sha256(uboot_dtb).digest():
        errors.append("uboot embedded U-Boot payload disagrees with u-boot-dtb.bin")


def _check_padded_uboot(errors, path, uboot_dtb):
    data = path.read_bytes()
    if len(data) != BOOT_SLOT:
        errors.append("uboot-nand must be exactly one 4 MiB erase block")
        return
    if uboot_dtb is not None:
        if data[: len(uboot_dtb)] != uboot_dtb:
            errors.append("uboot-nand does not begin with the exact u-boot-dtb.bin bytes")
        if any(data[len(uboot_dtb):]):
            errors.append("uboot-nand padding is not all zero")
    else:
        if any(data[ROLE_LIMITS["uboot-nand"] // 2:]):
            errors.append("uboot-nand tail padding is not zero")


def _check_boot_script(errors, data, manifest, kernel_record, dtb_record):
    try:
        header, payload = parse_uimage(data)
    except UImageError as error:
        errors.append("boot script is not a valid uImage: " + str(error))
        return
    if header["type"] != UIMAGE_TYPE_SCRIPT:
        errors.append("boot script uImage has the wrong image type")
    if header["load"] != 0 or header["entry"] != 0:
        errors.append("boot script uImage must not declare a load address")
    compatibility = manifest.get("compatibility", {})
    release = compatibility.get("kernel_release")
    if not isinstance(release, str) or not release:
        errors.append("compatibility.kernel_release is missing")
        return
    try:
        text = script_text(payload)
    except UnicodeDecodeError:
        errors.append("boot script payload is not UTF-8")
        return
    expected = {
        f"vmlinuz-{release}": True,
        f"dtbs/{release}/sun5i-r8-chip.dtb": True,
        "bootz 0x42000000": True,
        "0x43000000": True,
    }
    for needle in expected:
        if needle not in text:
            errors.append(f"boot script does not reference {needle}")
    for found in re.findall(r"vmlinuz-([^\s'\"]+)", text):
        if found != release:
            errors.append("boot script references a different kernel release")
    if kernel_record.get("sha256") and manifest.get("compatibility", {}).get("boot_script_sha256") != sha256_text(data):
        errors.append("boot script hash disagrees with the compatibility record")


def sha256_text(data):
    return hashlib.sha256(data).hexdigest()


def _check_dtb_overlay(errors, manifest, dtb, overlay):
    compatibility = manifest.get("compatibility", {})
    required = compatibility.get("overlay_required_labels")
    if not isinstance(required, list) or not required or any(not isinstance(item, str) for item in required):
        errors.append("compatibility.overlay_required_labels must be a non-empty string list")
        return
    labels = dtb.symbols()
    missing = [label for label in required if label not in labels]
    if missing:
        errors.append("DTB is missing overlay labels: " + ", ".join(sorted(missing)))
    try:
        overlay_tree = fdt.Fdt(overlay)
    except fdt.FdtError as error:
        errors.append("overlay is not a valid flattened device tree: " + str(error))
        return
    fixups = overlay_tree.external_fixups()
    if not fixups:
        errors.append("overlay declares no external fixups")
    unresolved = sorted(label for label in fixups if label not in labels)
    if unresolved:
        errors.append("overlay requires DTB labels that are absent: " + ", ".join(unresolved))


def validate_set(root, inputs_path, lock_path):
    """Validate every manifest in an artifact set plus its checksum inventory."""
    root = pathlib.Path(root)
    inputs = load_json(inputs_path)
    lock = load_json(lock_path, limit=8 * 1024 * 1024)
    errors = []
    manifests = sorted(root.glob("manifest-*.json"))
    if len(manifests) != 2:
        errors.append("artifact set must contain exactly two variant manifests")
    sums = root / "SHA256SUMS"
    if not sums.is_file():
        errors.append("artifact set is missing SHA256SUMS")
    else:
        listed = {}
        for line in sums.read_text().splitlines():
            digest, _, name = line.partition("  ")
            listed[name] = digest
        for path in sorted(root.rglob("*")):
            if path.is_file() and path.name != "SHA256SUMS":
                relative = path.relative_to(root).as_posix()
                if listed.get(relative) != sha256_file(path):
                    errors.append("SHA256SUMS disagrees for " + relative)
        if set(listed) - {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}:
            errors.append("SHA256SUMS lists files that are not present")
    for manifest_path in manifests:
        errors.extend(f"{manifest_path.name}: {error}" for error in validate_manifest(manifest_path, root, inputs, lock))
    return errors


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=pathlib.Path)
    parser.add_argument("--inputs", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1] / "images/inputs.lock.json")
    parser.add_argument("--lock", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1] / "upstream-lock.json")
    arguments = parser.parse_args()
    problems = validate_set(arguments.root, arguments.inputs, arguments.lock)
    for problem in problems:
        print(problem, file=__import__("sys").stderr)
    raise SystemExit(1 if problems else 0)
