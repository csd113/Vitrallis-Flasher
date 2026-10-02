#!/usr/bin/env python3
"""Host-side Batch 1 Debian 13 PocketCHIP artifact builder.

The selected route (see `docs/image-build.md`) consumes the pinned prebuilt
rootfs and assembles the complete host-side artifact set from locked inputs:

* verifies every asset by size and SHA-256;
* stream-scans the pinned rootfs without extracting it (kernel, DTB, overlay,
  boot script, kernel config, dpkg database, storage state, devices, links);
* rebuilds the pinned `sunxi-nand-image-builder` in the digest-pinned host
  container (no network) and generates the deterministic Hynix and Toshiba SPL
  images twice each;
* zero-pads the locked `u-boot-dtb.bin` to its 4 MiB erase-block slot;
* deterministically repacks the pinned rootfs archive and requires an
  equivalent member manifest;
* audits the actual storage state and compatibility relations;
* emits a strict physical manifest per NAND variant;
* can build the complete set twice from clean state and compare every byte.

It never touches NAND, a device, a mutable upstream repository, or the live
CHIP apt repository. `fetch-assets` is the only network command and downloads
exactly the locked release assets.
"""
import argparse
import configparser
import gzip
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
IMAGES = ROOT / "images"
sys.path.insert(0, str(IMAGES))
sys.path.insert(0, str(ROOT / "scripts"))

import build
import fdt
import physical
import repack
import spl
import storage
from uimage import UIMAGE_TYPE_SCRIPT, UImageError, parse as parse_uimage, script_text

import provenance

DEFAULT_INPUTS = IMAGES / "inputs.lock.json"
DEFAULT_UPSTREAM = ROOT / "upstream-lock.json"
DEFAULT_TOOL_SRC = ROOT / "work/g/repos/sunxi-tools"
DEFAULT_TOOL_IMAGE = "vitrallis-flasher-builder:bookworm-20260930"
DEFAULT_PLATFORM = "linux/amd64"
MAX_WANTED_BYTES = 32 * 1024 * 1024
ASSET_RESOLUTION = {"user-agent": "vitrallis-flasher-image-builder/0.1"}


class BuildError(ValueError):
    """A build input or produced artifact violates the reviewed contract."""


def canonical_json(value) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=True) + "\n").encode()


def write_json(path, value):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    data = canonical_json(value)
    path.write_bytes(data)
    return hashlib.sha256(data).hexdigest()


def sha256_file(path):
    with pathlib.Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def sha256_bytes(data: bytes):
    return hashlib.sha256(data).hexdigest()


def load_locks(inputs_path=DEFAULT_INPUTS, upstream_path=DEFAULT_UPSTREAM):
    inputs = build.load_inputs(inputs_path)
    lock = json.loads(pathlib.Path(upstream_path).read_bytes())
    errors = provenance.check_lock(lock)
    errors.extend(provenance.check_inputs(inputs))
    if errors:
        raise BuildError("locks are invalid: " + "; ".join(errors))
    return inputs, lock


def role_sources(lock, inputs):
    """Resolve each release role to its locked asset or derived artifact."""
    assets = {asset["name"]: asset for asset in lock["assets"]}
    derived = {artifact["name"]: artifact for artifact in lock.get("derived_artifacts", [])}
    result = {}
    for entry in lock["physical_roles"]:
        name = entry.get("artifact")
        if name in assets:
            result[entry["role"]] = {"kind": "asset", "name": name, "record": assets[name]}
        elif name in derived:
            result[entry["role"]] = {"kind": "derived", "name": name, "record": derived[name]}
        else:
            raise BuildError("role has no locked artifact: " + entry["role"])
    return result


def needed_assets(lock, inputs, sources):
    """Every asset file the build reads, keyed by the upstream asset name."""
    assets = {asset["name"]: asset for asset in lock["assets"]}
    needed = {}
    for role in ("uboot", "recovery", "rootfs"):
        source = sources[role]
        if source["kind"] != "asset":
            raise BuildError("role must resolve to a locked asset: " + role)
        needed[source["name"]] = assets[source["name"]]
    rootfs_name = inputs["rootfs"]["asset"]
    if rootfs_name not in assets:
        raise BuildError("selected rootfs is not in the provenance lock")
    root = assets[rootfs_name]
    if (
        root.get("size") != inputs["rootfs"]["size"]
        or root.get("sha256") != inputs["rootfs"]["sha256"]
        or root.get("url") != inputs["rootfs"]["url"]
    ):
        raise BuildError("selected rootfs disagrees between the locks")
    needed[rootfs_name] = root
    for role in ("uboot-nand", "spl-hynix", "spl-toshiba", "kernel", "dtb"):
        source = sources[role]
        if source["kind"] == "derived":
            from_asset = source["record"].get("from_asset")
            if from_asset not in assets:
                raise BuildError("derived artifact has an unknown source asset: " + source["name"])
            needed[from_asset] = assets[from_asset]
    return needed


def verify_assets(needed, assets_dir):
    assets_dir = pathlib.Path(assets_dir)
    resolved = {}
    for name, record in sorted(needed.items()):
        path = assets_dir / name
        if path.is_symlink() or not path.is_file():
            raise BuildError(f"missing locked asset {name}; run fetch-assets")
        size = path.stat().st_size
        if size != record["size"]:
            raise BuildError(f"asset {name} has size {size}, expected {record['size']}")
        digest = sha256_file(path)
        if digest != record["sha256"]:
            raise BuildError(f"asset {name} does not match its locked SHA-256")
        resolved[name] = path
    return resolved


def fetch_assets(needed, assets_dir):
    assets_dir = pathlib.Path(assets_dir)
    assets_dir.mkdir(parents=True, exist_ok=True)
    fetched = []
    for name, record in sorted(needed.items()):
        destination = assets_dir / name
        if destination.is_file() and not destination.is_symlink():
            if destination.stat().st_size == record["size"] and sha256_file(destination) == record["sha256"]:
                print(f"present  {name}")
                fetched.append(destination)
                continue
        if not record["url"].startswith("https://"):
            raise BuildError("refusing a non-HTTPS asset URL: " + name)
        print(f"fetching {name} ({record['size']} bytes)")
        request = urllib.request.Request(record["url"], headers=ASSET_RESOLUTION)
        temporary = destination.with_suffix(destination.suffix + ".part")
        digest = hashlib.sha256()
        size = 0
        with urllib.request.urlopen(request, timeout=600) as response, temporary.open("wb") as output:
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                size += len(chunk)
                if size > record["size"]:
                    raise BuildError("asset stream exceeds its locked size: " + name)
                digest.update(chunk)
                output.write(chunk)
        if size != record["size"] or digest.hexdigest() != record["sha256"]:
            temporary.unlink(missing_ok=True)
            raise BuildError("downloaded asset failed verification: " + name)
        os.replace(temporary, destination)
        fetched.append(destination)
    return fetched


def wanted_rootfs_files(release):
    return {
        f"boot/vmlinuz-{release}",
        f"boot/dtbs/{release}/sun5i-r8-chip.dtb",
        "boot/boot.scr",
        f"boot/config-{release}",
        "usr/lib/firmware/nextthingco/chip/early/x-chip-pocketchip.dtbo",
        "var/lib/dpkg/status",
        "etc/fstab",
        "etc/systemd/journald.conf",
        "usr/lib/systemd/system/tmp.mount",
    }


def scan_rootfs(archive, release, min_members=30000):
    """Stream-scan the pinned rootfs archive; never extract it to disk."""
    wanted = wanted_rootfs_files(release)
    result = {
        "files": {},
        "dpkg_info": {},
        "members": 0,
        "types": {},
        "hardlinks": 0,
        "devices": [],
        "system_links": [],
        "module_dir": False,
        "bytes": 0,
        "journal_dropins": 0,
        "zram_configs": 0,
        "unsafe": [],
    }
    with tarfile.open(archive, "r|gz") as tar:
        for member in tar:
            result["members"] += 1
            if result["members"] > 200000:
                raise BuildError("rootfs archive has excessive members")
            try:
                repack._safe_name(member.name)
            except repack.RepackError as error:
                raise BuildError(str(error)) from None
            if member.issparse() or set(member.pax_headers) - {"path", "linkpath"}:
                raise BuildError("rootfs archive carries unsupported metadata: " + member.name)
            if member.isdir():
                kind = "dir"
            elif member.isfile():
                kind = "file"
            elif member.issym():
                kind = "symlink"
            elif member.islnk():
                kind = "hardlink"
                result["hardlinks"] += 1
            elif member.ischr():
                kind = "char"
            elif member.isblk():
                kind = "block"
            else:
                raise BuildError("rootfs archive has a disallowed member type: " + member.name)
            result["types"][kind] = result["types"].get(kind, 0) + 1
            normalized = member.name[2:] if member.name.startswith("./") else member.name
            if member.isdev():
                result["devices"].append(
                    {"path": normalized, "type": kind, "major": member.devmajor, "minor": member.devminor}
                )
            if member.isdir() and normalized == f"usr/lib/modules/{release}":
                result["module_dir"] = True
            if normalized.startswith("etc/systemd/system/") and member.issym():
                result["system_links"].append([normalized, member.linkname])
            if normalized.startswith("etc/systemd/journald.conf.d/"):
                result["journal_dropins"] += 1
            if normalized == "etc/systemd/zram-generator.conf" or normalized.startswith("etc/systemd/zram-generator.conf.d/"):
                result["zram_configs"] += 1
            if not member.isfile():
                continue
            result["bytes"] += member.size
            if normalized not in wanted and not normalized.startswith("var/lib/dpkg/info/"):
                continue
            stream = tar.extractfile(member)
            if stream is None:
                raise BuildError("unreadable rootfs member: " + normalized)
            if normalized in wanted:
                if member.size > MAX_WANTED_BYTES:
                    raise BuildError("wanted rootfs file exceeds the size bound: " + normalized)
                result["files"][normalized] = stream.read()
            else:
                basename = normalized.rsplit("/", 1)[-1]
                if basename.endswith((".list", ".md5sums")):
                    digest = hashlib.sha256()
                    while True:
                        chunk = stream.read(1024 * 1024)
                        if not chunk:
                            break
                        digest.update(chunk)
                    result["dpkg_info"][basename] = digest.hexdigest()
                else:
                    while stream.read(1024 * 1024):
                        pass
    missing = wanted - set(result["files"])
    if missing:
        raise BuildError("rootfs is missing required files: " + ", ".join(sorted(missing)))
    if result["members"] < min_members:
        raise BuildError("rootfs archive has implausibly few members")
    if not result["module_dir"]:
        raise BuildError(f"rootfs has no kernel module tree for {release}")
    return result


def parse_dpkg_status(text):
    packages = []
    for block in text.split("\n\n"):
        fields = {}
        current = None
        for line in block.splitlines():
            if line[:1] in (" ", "\t") and current:
                fields[current] += "\n" + line
            elif ": " in line:
                current, _, value = line.partition(": ")
                fields[current] = value
        if "Package" in fields:
            packages.append(fields)
    return packages


def package_inventory(scan, inputs):
    """Content-hashed dpkg inventory of the pinned rootfs."""
    release = inputs["rootfs"]["kernel"]["release"]
    status = scan["files"]["var/lib/dpkg/status"]
    records = []
    for fields in parse_dpkg_status(status.decode("utf-8", "replace")):
        name = fields.get("Package")
        architecture = fields.get("Architecture", "")
        candidates = [name + ":" + architecture, name] if architecture and architecture != "all" else [name]
        listing = md5sums = None
        for candidate in candidates:
            listing = listing or scan["dpkg_info"].get(candidate + ".list")
            md5sums = md5sums or scan["dpkg_info"].get(candidate + ".md5sums")
        records.append(
            {
                "name": name,
                "version": fields.get("Version", ""),
                "architecture": architecture,
                "status": fields.get("Status", ""),
                "list_sha256": listing,
                "md5sums_sha256": md5sums,
            }
        )
    records.sort(key=lambda record: (record["name"], record["architecture"]))
    inventory = {
        "schema_version": 1,
        "rootfs": {"asset": inputs["rootfs"]["asset"], "sha256": inputs["rootfs"]["sha256"]},
        "dpkg_status_sha256": sha256_bytes(status),
        "package_count": len(records),
        "packages": records,
    }
    data = canonical_json(inventory)
    return inventory, data


def _noncomment_lines(text):
    return [line for line in text.splitlines() if line.strip() and not line.lstrip().startswith("#")]


def _parse_ini(text):
    parser = configparser.ConfigParser(interpolation=None)
    parser.read_string(text)
    return parser


def storage_audit(scan, config_values):
    """Audit the actual pinned rootfs against the reviewed storage policy."""
    violations = []
    fstab = scan["files"]["etc/fstab"].decode("utf-8", "replace")
    for line in _noncomment_lines(fstab):
        fields = line.split()
        if "swap" in fields:
            violations.append("fstab references swap: " + line)
    for path, target in scan["system_links"]:
        basename = path.rsplit("/", 1)[-1]
        if basename in ("swap.target", "swapfile.swap") or "swap" in basename or "zram" in basename:
            violations.append("enabled swap/zram unit link: " + path)
        if target and (target.endswith("swap.target") or "zram" in target):
            violations.append("unit link targets swap/zram: " + path)
    journal = _parse_ini(scan["files"]["etc/systemd/journald.conf"].decode("utf-8", "replace"))
    active = [
        f"{section}.{option}"
        for section in journal.sections()
        for option in journal.options(section)
    ]
    if active:
        violations.append("upstream journald.conf carries active directives: " + ", ".join(active))
    tmp_options = None
    for line in scan["files"]["usr/lib/systemd/system/tmp.mount"].decode("utf-8", "replace").splitlines():
        if line.startswith("Options="):
            tmp_options = line.partition("=")[2]
    if not tmp_options:
        violations.append("tmp.mount has no Options= line")
    policy = storage.storage_policy()
    audit = {
        "policy_status": policy["status"],
        "nand_backed_swap": "prohibited",
        "fstab_swap": False,
        "enabled_swap_units": [],
        "journal_active_directives": len(active),
        "journal_dropins": scan["journal_dropins"],
        "zram_generator_configs": scan["zram_configs"],
        "tmp_options": tmp_options,
        "kernel": {name: config_values.get(name) for name in ("CONFIG_SWAP", "CONFIG_ZSWAP", "CONFIG_ZRAM", "CONFIG_ZRAM_WRITEBACK", "CONFIG_UBIFS_FS")},
        "candidate_configurations": policy["configurations"],
        "policy_sha256": sha256_bytes(canonical_json(policy)),
        "applied": False,
    }
    if violations:
        raise BuildError("storage audit failed: " + "; ".join(violations))
    return audit


def compatibility_report(scan, inputs, lock, compatibility, sources):
    errors = []
    release = inputs["rootfs"]["kernel"]["release"]
    package = inputs["rootfs"]["kernel"]["package"]
    version = inputs["rootfs"]["kernel"]["version"]
    files = scan["files"]
    dtb = files[f"boot/dtbs/{release}/sun5i-r8-chip.dtb"]
    overlay = files["usr/lib/firmware/nextthingco/chip/early/x-chip-pocketchip.dtbo"]
    boot_scr = files["boot/boot.scr"]
    if compatibility.get("kernel_release") != release:
        errors.append("compatibility kernel release disagrees with the selected rootfs")
    for key, data, expected in (
        ("dtb", dtb, compatibility.get("dtb", {})),
        ("overlay", overlay, compatibility.get("overlay", {})),
        ("boot_script", boot_scr, compatibility.get("boot_script", {})),
    ):
        if expected.get("sha256") != sha256_bytes(data):
            errors.append(f"compatibility evidence disagrees with the extracted {key}")
    derived = {artifact["name"]: artifact for artifact in lock.get("derived_artifacts", [])}
    for name, data in (
        (compatibility.get("dtb", {}).get("artifact"), dtb),
        (compatibility.get("overlay", {}).get("artifact"), overlay),
        (compatibility.get("boot_script", {}).get("artifact"), boot_scr),
    ):
        record = derived.get(name)
        if record is None:
            errors.append(f"compatibility artifact is not locked: {name}")
        elif record.get("sha256") != sha256_bytes(data):
            errors.append(f"locked derived artifact disagrees with the extracted file: {name}")
    dtb_tree = fdt.Fdt(dtb)
    overlay_tree = fdt.Fdt(overlay)
    symbols = dtb_tree.symbols()
    required = compatibility.get("overlay_required_labels", [])
    missing = sorted(label for label in required if label not in symbols)
    if missing:
        errors.append("DTB is missing overlay labels: " + ", ".join(missing))
    fixups = sorted(overlay_tree.external_fixups())
    unresolved = sorted(label for label in fixups if label not in symbols)
    if unresolved:
        errors.append("overlay requires absent DTB labels: " + ", ".join(unresolved))
    selection = compatibility.get("overlay_selection", {})
    if selection.get("eeprom_magic") != "CHIP" or selection.get("pid_offset") != 9 or selection.get("pocketchip_pid") != [0, 1]:
        errors.append("overlay DIP selection evidence is malformed")
    try:
        header, payload = parse_uimage(boot_scr)
    except UImageError as error:
        errors.append("boot script is not a valid uImage: " + str(error))
        header, payload = None, b""
    if header is not None:
        if header["type"] != UIMAGE_TYPE_SCRIPT:
            errors.append("boot script uImage is not a script image")
        if header["load"] != 0 or header["entry"] != 0:
            errors.append("boot script declares unexpected load/entry addresses")
        try:
            text = script_text(payload)
        except UnicodeDecodeError:
            errors.append("boot script payload is not UTF-8")
            text = ""
        for needle in (
            f"vmlinuz-{release}",
            f"dtbs/{release}/sun5i-r8-chip.dtb",
            "bootz 0x42000000",
            "0x43000000",
            "0x45000009",
            "0x4500000a",
        ):
            if needle not in text:
                errors.append("boot script does not reference " + needle)
    kernel_config = files[f"boot/config-{release}"]
    for name in ("linux-image-" + release, "linux-image-chip"):
        found = None
        for fields in parse_dpkg_status(scan["files"]["var/lib/dpkg/status"].decode("utf-8", "replace")):
            if fields.get("Package") == name:
                found = fields.get("Version")
        if found != version:
            errors.append(f"dpkg package {name} is {found!r}, expected {version!r}")
    if errors:
        raise BuildError("compatibility validation failed: " + "; ".join(errors))
    return {
        "kernel_release": release,
        "kernel_package": package,
        "kernel_version": version,
        "dtb_sha256": sha256_bytes(dtb),
        "overlay_sha256": sha256_bytes(overlay),
        "boot_script_sha256": sha256_bytes(boot_scr),
        "overlay_required_labels": list(required),
        "dtb_symbols": sorted(symbols),
        "overlay_fixups": fixups,
        "boot_script_kernel": f"vmlinuz-{release}",
        "kernel_config_sha256": sha256_bytes(kernel_config),
    }


def validate_spl_tool(inputs, tool_src):
    tool = inputs["spl_tool"]
    tool_src = pathlib.Path(tool_src)
    for relative, digest in tool["files"].items():
        path = tool_src / relative
        if path.is_symlink() or not path.is_file():
            raise BuildError("SPL tool source is missing: " + relative)
        if sha256_file(path) != digest:
            raise BuildError("SPL tool source does not match its pinned hash: " + relative)
    return tool


def build_spls(inputs, tool_src, sunxi_spl, work, tool_image, platform):
    """Run the pinned tool twice per variant inside the digest-pinned container."""
    tool = validate_spl_tool(inputs, tool_src)
    work = pathlib.Path(work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    flags = " ".join(tool["compile_flags"])
    report = {}
    for variant, oob in sorted(spl.OOB_SIZES.items()):
        source = spl.build_source(sunxi_spl)
        entropy = spl.entropy_stream(variant)
        source_path = work / f"source-{variant}.bin"
        entropy_path = work / f"entropy-{variant}.bin"
        source_path.write_bytes(source)
        entropy_path.write_bytes(entropy)
        script = (
            "set -eu\n"
            "mkdir -p /work/tool\n"
            "cp /src/nand-image-builder.c /src/common.h /src/include/portable_endian.h /work/tool/\n"
            "cat > /work/tool/version.h <<'VERSION_HEADER'\n"
            + tool["version_header"]
            + "VERSION_HEADER\n"
            + f"gcc {flags} -o /work/tool/sunxi-nand-image-builder /work/tool/nand-image-builder.c\n"
            + f"/work/tool/sunxi-nand-image-builder -c 64/1024 -p 16384 -o {oob} -u 1024 -e 4194304 -b -s /work/source-{variant}.bin /work/run1-{variant}.nand\n"
            + f"/work/tool/sunxi-nand-image-builder -c 64/1024 -p 16384 -o {oob} -u 1024 -e 4194304 -b -s /work/source-{variant}.bin /work/run2-{variant}.nand\n"
        )
        command = [
            "docker",
            "run",
            "--rm",
            "--network",
            "none",
            "--platform",
            platform,
            "-v",
            f"{pathlib.Path(tool_src).resolve()}:/src:ro",
            "-v",
            f"{work}:/work:rw",
            "-v",
            f"{entropy_path}:/dev/urandom:ro",
            tool_image,
            "sh",
            "-eu",
            "-c",
            script,
        ]
        completed = subprocess.run(command, check=False, capture_output=True, text=True, timeout=900)
        if completed.returncode != 0:
            raise BuildError("SPL tool container failed: " + completed.stderr.strip()[-500:])
        first = (work / f"run1-{variant}.nand").read_bytes()
        second = (work / f"run2-{variant}.nand").read_bytes()
        if first != second:
            raise BuildError(f"{variant} SPL generation is not deterministic")
        spl.validate_image(first, oob, sunxi_spl)
        pin = inputs["derived"][f"spl-{variant}"]
        digest = sha256_bytes(first)
        if len(first) != pin["size"] or digest != pin["sha256"]:
            raise BuildError(f"{variant} SPL disagrees with the locked derived pin")
        report[variant] = {
            "size": len(first),
            "sha256": digest,
            "locked_sha256": pin["sha256"],
            "runs_identical": True,
            "source_sha256": sha256_bytes(source),
            "entropy_sha256": sha256_bytes(entropy),
            "tool_sha256": sha256_file(work / "tool/sunxi-nand-image-builder"),
        }
    return report


def pad_uboot(uboot_dtb, inputs):
    if len(uboot_dtb) > 4194304:
        raise BuildError("u-boot-dtb.bin does not fit its erase-block slot")
    padded = uboot_dtb + b"\x00" * (4194304 - len(uboot_dtb))
    pin = inputs["derived"]["u-boot-dtb-padded.bin"]
    if len(padded) != pin["size"] or sha256_bytes(padded) != pin["sha256"]:
        raise BuildError("padded U-Boot disagrees with the locked derived pin")
    return padded


def materialize(source, destination):
    """Hardlink when possible so both variant sets share the same bytes."""
    destination = pathlib.Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        raise BuildError("artifact already exists: " + str(destination))
    try:
        os.link(source, destination)
    except OSError:
        shutil.copyfile(source, destination)
    return destination


def role_artifact_names(inputs, sources):
    """Canonical file names inside the artifact set."""
    rootfs_asset = inputs["rootfs"]["asset"]
    return {
        "uboot": "u-boot-sunxi-with-spl.bin",
        "uboot-nand": "u-boot-dtb-padded.bin",
        "kernel": f"vmlinuz-{inputs['rootfs']['kernel']['release']}",
        "dtb": "sun5i-r8-chip.dtb",
        "recovery": "initrd.uimage",
        "spl-hynix": "spl-hynix.nand",
        "spl-toshiba": "spl-toshiba.nand",
        "rootfs": rootfs_asset,
    }


def provenance_record(source, lock):
    record = source["record"]
    if source["kind"] == "asset":
        return {
            "kind": "asset",
            "locked_sha256": record["sha256"],
            "source": record["source_repository"],
            "commit": record["source_commit"],
            "url": record["url"],
        }
    assets = {asset["name"]: asset for asset in lock["assets"]}
    from_asset = record.get("from_asset")
    origin = assets.get(from_asset, {})
    return {
        "kind": "derived",
        "locked_sha256": record["sha256"],
        "source": origin.get("source_repository", ""),
        "commit": origin.get("source_commit", ""),
        "from_asset": from_asset,
        "transform": record.get("transform") or record.get("path") or "deterministic derivation",
    }


def derived_commit(lock, inputs, name):
    assets = {asset["name"]: asset for asset in lock["assets"]}
    derived = {artifact["name"]: artifact for artifact in lock.get("derived_artifacts", [])}
    record = derived.get(name) or assets.get(name)
    if record is None:
        raise BuildError("unknown locked artifact: " + name)
    from_asset = record.get("from_asset")
    if from_asset in assets:
        return assets[from_asset]["source_repository"], assets[from_asset]["source_commit"]
    return record.get("source_repository", ""), record.get("source_commit", "")


def make_manifest(variant, release_id, inputs, lock, sources, set_dir, artifact_meta, compat, audit_meta, inventory_meta, input_meta, toolchain):
    artifacts = []
    for role in physical.ROLES:
        meta = artifact_meta[role]
        source = sources[role]
        provenance = provenance_record(source, lock)
        artifacts.append(
            {
                "role": role,
                "file": "artifacts/" + meta["file"],
                "size": meta["size"],
                "sha256": meta["sha256"],
                "format": physical.ROLE_FORMATS[role],
                "provenance": provenance,
            }
        )
    manifest = {
        "schema_version": 1,
        "release": release_id,
        "board": "pocketchip",
        "soc": "allwinner-r8",
        "os": "debian-13-trixie",
        "architecture": "armhf",
        "installer_protocol": 1,
        "minimum_flasher": "0.1.0",
        "profile": "stock",
        "vitrallis": "not-installed",
        "nand_variant": variant,
        "layout": {
            "page_size": physical.PAGE_SIZE,
            "eraseblock_size": physical.ERASEBLOCK_SIZE,
            "oob_size": physical.OOB_BY_VARIANT[variant],
            "offsets": dict(physical.BOOT_OFFSETS, rootfs=physical.ROOTFS_OFFSET),
            "slots": {name: physical.BOOT_SLOT for name in physical.BOOT_OFFSETS},
        },
        "artifacts": artifacts,
        "inputs": input_meta,
        "compatibility": {
            "kernel_release": compat["kernel_release"],
            "kernel_package": compat["kernel_package"],
            "kernel_version": compat["kernel_version"],
            "dtb_file": "artifacts/" + artifact_meta["dtb"]["file"],
            "dtb_sha256": compat["dtb_sha256"],
            "overlay_file": "inputs/x-chip-pocketchip.dtbo",
            "overlay_sha256": compat["overlay_sha256"],
            "boot_script_file": "inputs/boot.scr",
            "boot_script_sha256": compat["boot_script_sha256"],
            "overlay_required_labels": compat["overlay_required_labels"],
        },
        "storage": {
            "policy": "candidate-not-applied",
            "policy_sha256": audit_meta["policy_sha256"],
            "nand_backed_swap": "prohibited",
            "applied": False,
            "audit_file": "evidence/storage-audit.json",
            "audit_sha256": audit_meta["audit_sha256"],
        },
        "package_inventory": {
            "file": "evidence/package-inventory.json",
            "sha256": inventory_meta["sha256"],
            "count": inventory_meta["count"],
        },
        "toolchain": toolchain,
        "flash_approved": False,
        "approval": None,
    }
    return manifest


def locked_digest(lock, name):
    derived = {artifact["name"]: artifact for artifact in lock.get("derived_artifacts", [])}
    if name in derived:
        return derived[name]["sha256"]
    assets = {asset["name"]: asset for asset in lock["assets"]}
    if name in assets:
        return assets[name]["sha256"]
    raise BuildError("unknown locked artifact: " + name)


def _input_meta(name, file, size, digest, kind, source, commit, locked, url=None, repository=None):
    if digest != locked:
        raise BuildError(f"input {name} disagrees with its locked hash")
    record = {
        "name": name,
        "file": file,
        "size": size,
        "sha256": digest,
        "locked_sha256": locked,
        "kind": kind,
        "source": source,
        "commit": commit,
    }
    if url:
        record["url"] = url
    if repository:
        record["repository"] = repository
    return record


def write_checksums(set_dir):
    set_dir = pathlib.Path(set_dir)
    lines = []
    for path in sorted(set_dir.rglob("*")):
        if path.is_file() and path.name != "SHA256SUMS":
            lines.append(sha256_file(path) + "  " + path.relative_to(set_dir).as_posix())
    (set_dir / "SHA256SUMS").write_text("\n".join(lines) + "\n")
    return len(lines)


def build_set(
    inputs,
    lock,
    assets_dir,
    set_dir,
    work_dir,
    tool_src=DEFAULT_TOOL_SRC,
    tool_image=DEFAULT_TOOL_IMAGE,
    platform=DEFAULT_PLATFORM,
    variants=("hynix", "toshiba"),
    do_repack=True,
    inputs_path=DEFAULT_INPUTS,
    upstream_path=DEFAULT_UPSTREAM,
):
    set_dir = pathlib.Path(set_dir)
    work_dir = pathlib.Path(work_dir)
    if set_dir.exists() or set_dir.is_symlink():
        raise BuildError("refusing to build into an existing directory")
    if work_dir.exists():
        raise BuildError("refusing to use an existing work directory")
    sources = role_sources(lock, inputs)
    if sources["rootfs"]["name"] != inputs["rootfs"]["asset"]:
        raise BuildError("physical rootfs role disagrees with the selected rootfs")
    needed = needed_assets(lock, inputs, sources)
    assets = verify_assets(needed, assets_dir)
    release = inputs["rootfs"]["kernel"]["release"]
    release_id = f"debian13-{inputs['rootfs']['tag']}-stock"
    scan = scan_rootfs(assets[inputs["rootfs"]["asset"]], release)

    evidence_dir = IMAGES / "evidence"
    config_evidence = evidence_dir / f"kernel-config-{release}"
    config_bytes = scan["files"][f"boot/config-{release}"]
    if not config_evidence.is_file() or config_evidence.read_bytes() != config_bytes:
        raise BuildError("extracted kernel config does not match the checked-in evidence")
    requirements = json.loads((IMAGES / "kernel-requirements.json").read_bytes())
    if requirements.get("config_sha256") != sha256_bytes(config_bytes):
        raise BuildError("kernel requirements evidence hash disagrees with the extraction")
    if requirements.get("kernel_package", {}).get("name") != inputs["rootfs"]["kernel"]["package"]:
        raise BuildError("kernel requirements name disagrees with the selected rootfs")
    config_values = provenance.parse_kernel_config(config_evidence)

    inventory, inventory_bytes = package_inventory(scan, inputs)
    inventory_digest = sha256_bytes(inventory_bytes)
    checked_in = IMAGES / f"package-inventory-{release}.json"
    if checked_in.is_file() and checked_in.read_bytes() != inventory_bytes:
        raise BuildError("checked-in package inventory disagrees with the rootfs")
    if inventory_digest != inputs["package_lock_sha256"]:
        raise BuildError("package inventory disagrees with the input lock")
    compatibility = json.loads((IMAGES / "compatibility.json").read_bytes())
    compat = compatibility_report(scan, inputs, lock, compatibility, sources)
    audit = storage_audit(scan, config_values)
    spl_report = build_spls(inputs, tool_src, assets["sunxi-spl.bin"].read_bytes(), work_dir / "spl", tool_image, platform)
    padded = pad_uboot(assets["u-boot-dtb.bin"].read_bytes(), inputs)

    # Materialize the set.
    (set_dir / "artifacts").mkdir(parents=True)
    (set_dir / "inputs").mkdir()
    (set_dir / "evidence").mkdir()
    names = role_artifact_names(inputs, sources)
    uboot_name = names["uboot"]
    roles_paths = {
        "uboot": materialize(assets["u-boot-sunxi-with-spl.bin"], set_dir / "artifacts" / uboot_name),
        "recovery": materialize(assets["initrd.uimage"], set_dir / "artifacts" / names["recovery"]),
        "rootfs": materialize(assets[inputs["rootfs"]["asset"]], set_dir / "artifacts" / names["rootfs"]),
    }
    roles_paths["uboot-nand"] = set_dir / "artifacts" / names["uboot-nand"]
    roles_paths["uboot-nand"].write_bytes(padded)
    roles_paths["kernel"] = set_dir / "artifacts" / names["kernel"]
    roles_paths["kernel"].write_bytes(scan["files"][f"boot/vmlinuz-{release}"])
    roles_paths["dtb"] = set_dir / "artifacts" / names["dtb"]
    roles_paths["dtb"].write_bytes(scan["files"][f"boot/dtbs/{release}/sun5i-r8-chip.dtb"])
    for variant in spl.OOB_SIZES:
        roles_paths[f"spl-{variant}"] = set_dir / "artifacts" / names[f"spl-{variant}"]
        roles_paths[f"spl-{variant}"].write_bytes((work_dir / "spl" / f"run1-{variant}.nand").read_bytes())
    input_paths = {
        "sunxi-spl.bin": materialize(assets["sunxi-spl.bin"], set_dir / "inputs/sunxi-spl.bin"),
        "u-boot-dtb.bin": materialize(assets["u-boot-dtb.bin"], set_dir / "inputs/u-boot-dtb.bin"),
    }
    input_paths["boot.scr"] = set_dir / "inputs/boot.scr"
    input_paths["boot.scr"].write_bytes(scan["files"]["boot/boot.scr"])
    input_paths["x-chip-pocketchip.dtbo"] = set_dir / "inputs/x-chip-pocketchip.dtbo"
    input_paths["x-chip-pocketchip.dtbo"].write_bytes(
        scan["files"]["usr/lib/firmware/nextthingco/chip/early/x-chip-pocketchip.dtbo"]
    )
    evidence_paths = {
        "kernel-config": set_dir / "evidence" / f"kernel-config-{release}",
        "package-inventory.json": set_dir / "evidence/package-inventory.json",
        "storage-audit.json": set_dir / "evidence/storage-audit.json",
    }
    evidence_paths["kernel-config"].write_bytes(config_bytes)
    evidence_paths["package-inventory.json"].write_bytes(inventory_bytes)
    audit_bytes = canonical_json(audit)
    evidence_paths["storage-audit.json"].write_bytes(audit_bytes)
    audit_sha256 = sha256_bytes(audit_bytes)

    repack_summary = None
    if do_repack:
        (set_dir / "derived").mkdir()
        repack_summary = repack.repack_archive(
            assets[inputs["rootfs"]["asset"]],
            set_dir / "derived/rootfs-repacked.tar.gz",
            inputs["source_date_epoch"],
        )

    artifact_meta = {}
    for role, path in roles_paths.items():
        artifact_meta[role] = {"file": path.name, "size": path.stat().st_size, "sha256": sha256_file(path)}
        source = sources[role]
        if source["record"].get("sha256") != artifact_meta[role]["sha256"]:
            raise BuildError(f"role {role} does not match its locked provenance hash")
        if source["record"].get("size") != artifact_meta[role]["size"]:
            raise BuildError(f"role {role} does not match its locked provenance size")
    for role in ("spl-hynix", "spl-toshiba"):
        variant = role.split("-", 1)[1]
        if artifact_meta[role]["sha256"] != spl_report[variant]["sha256"]:
            raise BuildError("generated SPL summary disagrees with the artifact")

    rootfs_commit = next(asset["source_commit"] for asset in lock["assets"] if asset["name"] == inputs["rootfs"]["asset"])
    rootfs_repo = next(asset["source_repository"] for asset in lock["assets"] if asset["name"] == inputs["rootfs"]["asset"])
    input_meta = [
        _input_meta(
            "sunxi-spl.bin",
            "inputs/sunxi-spl.bin",
            input_paths["sunxi-spl.bin"].stat().st_size,
            sha256_file(input_paths["sunxi-spl.bin"]),
            "asset",
            needed["sunxi-spl.bin"]["source_repository"],
            needed["sunxi-spl.bin"]["source_commit"],
            needed["sunxi-spl.bin"]["sha256"],
            url=needed["sunxi-spl.bin"]["url"],
        ),
        _input_meta(
            "u-boot-dtb.bin",
            "inputs/u-boot-dtb.bin",
            input_paths["u-boot-dtb.bin"].stat().st_size,
            sha256_file(input_paths["u-boot-dtb.bin"]),
            "asset",
            needed["u-boot-dtb.bin"]["source_repository"],
            needed["u-boot-dtb.bin"]["source_commit"],
            needed["u-boot-dtb.bin"]["sha256"],
            url=needed["u-boot-dtb.bin"]["url"],
        ),
        _input_meta(
            "boot.scr",
            "inputs/boot.scr",
            input_paths["boot.scr"].stat().st_size,
            sha256_file(input_paths["boot.scr"]),
            "derived",
            rootfs_repo,
            rootfs_commit,
            locked_digest(lock, "boot.scr"),
        ),
        _input_meta(
            "x-chip-pocketchip.dtbo",
            "inputs/x-chip-pocketchip.dtbo",
            input_paths["x-chip-pocketchip.dtbo"].stat().st_size,
            sha256_file(input_paths["x-chip-pocketchip.dtbo"]),
            "derived",
            rootfs_repo,
            rootfs_commit,
            locked_digest(lock, "x-chip-pocketchip.dtbo"),
        ),
    ]
    inventory_meta = {"sha256": inventory_digest, "count": inventory["package_count"]}
    toolchain = {
        "container_digest": inputs["container_digest"],
        "debian_snapshot": inputs["debian_snapshot"],
        "spl_tool": {
            "repository": inputs["spl_tool"]["repository"],
            "commit": inputs["spl_tool"]["commit"],
            "files": {pathlib.Path(name).name: digest for name, digest in inputs["spl_tool"]["files"].items()},
            "compile_flags": inputs["spl_tool"]["compile_flags"],
        },
    }
    manifest_meta = {}
    for variant in variants:
        manifest = make_manifest(
            variant, release_id, inputs, lock, sources, set_dir, artifact_meta, compat, {"policy_sha256": audit["policy_sha256"], "audit_sha256": audit_sha256}, inventory_meta, input_meta, toolchain
        )
        manifest_path = set_dir / f"manifest-{variant}.json"
        digest = write_json(manifest_path, manifest)
        errors = physical.validate_manifest(manifest_path, set_dir, inputs, lock)
        if errors:
            raise BuildError("generated manifest failed validation: " + "; ".join(errors))
        manifest_meta[variant] = {"file": manifest_path.name, "sha256": digest}
    report = {
        "schema_version": 1,
        "release": release_id,
        "inputs_lock_sha256": sha256_file(inputs_path),
        "upstream_lock_sha256": sha256_file(upstream_path),
        "source_date_epoch": inputs["source_date_epoch"],
        "rootfs": {
            "asset": inputs["rootfs"]["asset"],
            "sha256": inputs["rootfs"]["sha256"],
            "members": scan["members"],
            "bytes": scan["bytes"],
        },
        "artifacts": [
            {"role": role, "size": artifact_meta[role]["size"], "sha256": artifact_meta[role]["sha256"], "locked_sha256": sources[role]["record"]["sha256"]}
            for role in physical.ROLES
        ],
        "spl": spl_report,
        "uboot_padding": {"input_sha256": needed["u-boot-dtb.bin"]["sha256"], "output_sha256": artifact_meta["uboot-nand"]["sha256"], "size": artifact_meta["uboot-nand"]["size"]},
        "repack": repack_summary,
        "package_inventory": inventory_meta,
        "storage_audit_sha256": audit_sha256,
        "compatibility": compat,
        "toolchain": toolchain,
        "manifests": manifest_meta,
    }
    report_path = set_dir / "evidence/build-report.json"
    write_json(report_path, report)
    write_checksums(set_dir)
    return report


def compare_sets(first, second):
    first, second = pathlib.Path(first), pathlib.Path(second)
    left = {path.relative_to(first).as_posix(): sha256_file(path) for path in first.rglob("*") if path.is_file()}
    right = {path.relative_to(second).as_posix(): sha256_file(path) for path in second.rglob("*") if path.is_file()}
    different = sorted(name for name in set(left) & set(right) if left[name] != right[name])
    only_left = sorted(set(left) - set(right))
    only_right = sorted(set(right) - set(left))
    return {
        "identical": not different and not only_left and not only_right,
        "files": len(left),
        "different": different,
        "only_first": only_left,
        "only_second": only_right,
    }


def reproduce(inputs, lock, assets_dir, output_dir, work_root, **options):
    output_dir = pathlib.Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    report = {}
    for index in (1, 2):
        report[index] = build_set(
            inputs,
            lock,
            assets_dir,
            output_dir / f"build-{index}",
            pathlib.Path(work_root) / f"build-{index}",
            **options,
        )
    comparison = compare_sets(output_dir / "build-1", output_dir / "build-2")
    result = {"comparison": comparison, "manifests": report[1]["manifests"]}
    print(json.dumps(result, indent=2))
    if not comparison["identical"]:
        raise BuildError("the two clean builds are not byte-identical")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=pathlib.Path, default=DEFAULT_INPUTS)
    parser.add_argument("--upstream", type=pathlib.Path, default=DEFAULT_UPSTREAM)
    sub = parser.add_subparsers(dest="command", required=True)

    fetch = sub.add_parser("fetch-assets", help="download the locked release assets")
    fetch.add_argument("--assets", type=pathlib.Path, required=True)

    build_parser = sub.add_parser("build", help="build one complete artifact set")
    build_parser.add_argument("--assets", type=pathlib.Path, required=True)
    build_parser.add_argument("--output", type=pathlib.Path, required=True)
    build_parser.add_argument("--work", type=pathlib.Path, required=True)
    build_parser.add_argument("--tool-src", type=pathlib.Path, default=DEFAULT_TOOL_SRC)
    build_parser.add_argument("--tool-image", default=DEFAULT_TOOL_IMAGE)
    build_parser.add_argument("--platform", default=DEFAULT_PLATFORM)
    build_parser.add_argument("--variants", default="hynix,toshiba")
    build_parser.add_argument("--skip-repack", action="store_true")

    reproduce_parser = sub.add_parser("reproduce", help="build twice from clean state and compare")
    reproduce_parser.add_argument("--assets", type=pathlib.Path, required=True)
    reproduce_parser.add_argument("--output", type=pathlib.Path, required=True)
    reproduce_parser.add_argument("--work", type=pathlib.Path, required=True)
    reproduce_parser.add_argument("--tool-src", type=pathlib.Path, default=DEFAULT_TOOL_SRC)
    reproduce_parser.add_argument("--tool-image", default=DEFAULT_TOOL_IMAGE)
    reproduce_parser.add_argument("--platform", default=DEFAULT_PLATFORM)
    reproduce_parser.add_argument("--skip-repack", action="store_true")

    verify = sub.add_parser("verify", help="validate an existing artifact set")
    verify.add_argument("--set", type=pathlib.Path, required=True)

    args = parser.parse_args()
    inputs, lock = load_locks(args.inputs, args.upstream)
    options = {"tool_src": DEFAULT_TOOL_SRC, "tool_image": DEFAULT_TOOL_IMAGE, "platform": DEFAULT_PLATFORM}
    if args.command == "fetch-assets":
        sources = role_sources(lock, inputs)
        needed = needed_assets(lock, inputs, sources)
        result = fetch_assets(needed, args.assets)
        print(f"verified {len(result)} locked assets")
        return 0
    if args.command == "verify":
        errors = physical.validate_set(args.set, args.inputs, args.upstream)
        for error in errors:
            print(error, file=sys.stderr)
        return 1 if errors else 0
    options = {
        "tool_src": getattr(args, "tool_src", DEFAULT_TOOL_SRC),
        "tool_image": getattr(args, "tool_image", DEFAULT_TOOL_IMAGE),
        "platform": getattr(args, "platform", DEFAULT_PLATFORM),
        "do_repack": not getattr(args, "skip_repack", False),
    }
    if args.command == "build":
        variants = tuple(part for part in args.variants.split(",") if part)
        for variant in variants:
            if variant not in physical.OOB_BY_VARIANT:
                raise BuildError("unknown variant: " + variant)
        report = build_set(inputs, lock, args.assets, args.output, args.work, variants=variants, inputs_path=args.inputs, upstream_path=args.upstream, **options)
        print(json.dumps(report, indent=2))
        return 0
    result = reproduce(
        inputs,
        lock,
        args.assets,
        args.output,
        args.work,
        variants=("hynix", "toshiba"),
        inputs_path=args.inputs,
        upstream_path=args.upstream,
        **options,
    )
    return 0 if result["comparison"]["identical"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
