#!/usr/bin/env python3
"""Deterministic provenance checks for release inputs and checked-in evidence.

`check` validates `upstream-lock.json`, `images/inputs.lock.json` and the
checked-in kernel-config evidence with no network access. `report` prints the
artifact matrix. `verify-upstream` is an explicit network command that
re-downloads every locked asset and re-resolves recorded tags; it is never part
of routine validation, so CI does not depend on upstream availability.
`check-kernel-config` asserts a resolved kernel configuration against the
recorded requirements.

An unresolved value stays explicit. Validation never turns a null or missing
pin into a trusted input.
"""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
LOCK_PATH = ROOT / 'upstream-lock.json'
INPUTS_PATH = ROOT / 'images/inputs.lock.json'
REQUIREMENTS_PATH = ROOT / 'images/kernel-requirements.json'
MAX_DOCUMENT_BYTES = 1024 * 1024

ROLES = ('uboot', 'kernel', 'dtb', 'recovery', 'spl-hynix', 'spl-toshiba', 'uboot-nand', 'rootfs')
STATUSES = ('VERIFIED', 'VERIFIED BUT PREBUILT', 'PARTIAL', 'BLOCKED', 'UNKNOWN')
REPRODUCIBILITY = (
    'REPRODUCIBLE FROM PINNED SOURCE',
    'PINNED PREBUILT BINARY',
    'SOURCE KNOWN / BUILD NOT YET REPRODUCED',
    'PROVENANCE INCOMPLETE',
    'UNRESOLVED',
)
IMMUTABLE_REF = re.compile(r'^[0-9a-f]{40}$')
SHA256 = re.compile(r'^[0-9a-f]{64}$')
MUTABLE_SEGMENTS = ('/latest/', '/releases/latest')
TAG_TYPES = ('lightweight', 'annotated')


class ProvenanceError(ValueError):
    """A lock or evidence document does not satisfy the provenance contract."""


def load_document(path, limit=MAX_DOCUMENT_BYTES):
    raw = pathlib.Path(path).read_bytes()
    if len(raw) > limit:
        raise ProvenanceError('document exceeds the size bound: ' + str(path))
    return json.loads(raw)


def sha256_file(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def https_url(value):
    if not isinstance(value, str) or not value.startswith('https://') or len(value) > 2048:
        return False
    if any(segment in value for segment in MUTABLE_SEGMENTS):
        return False
    if value.endswith(('/latest', '/master', '/main')):
        return False
    release = re.search(r'/releases/(?:download|tag)/([^/?#]+)', value)
    if release and release.group(1) in ('latest', 'master', 'main', 'HEAD'):
        return False
    return True


def problem(errors, message):
    errors.append(message)


def check_repositories(lock, errors):
    repositories = lock.get('repositories')
    if not isinstance(repositories, list) or not repositories:
        problem(errors, 'repositories must be a non-empty list')
        return {}
    known = {}
    for index, repository in enumerate(repositories):
        where = f'repositories[{index}]'
        if not isinstance(repository, dict):
            problem(errors, where + ' must be an object')
            continue
        url = repository.get('repository')
        if not https_url(url):
            problem(errors, where + ' needs an immutable HTTPS repository URL')
            continue
        if url in known:
            problem(errors, where + ' duplicates ' + url)
        commit = repository.get('commit')
        if not isinstance(commit, str) or not IMMUTABLE_REF.match(commit):
            problem(errors, where + ' needs a full 40-digit lowercase commit')
        files = repository.get('reviewed_files')
        if not isinstance(files, dict) or not files:
            problem(errors, where + ' needs at least one reviewed file hash')
        else:
            for name, digest in files.items():
                if not isinstance(name, str) or not name or '..' in name:
                    problem(errors, where + ' has an unsafe reviewed file name')
                if not isinstance(digest, str) or not SHA256.match(digest):
                    problem(errors, where + f' reviewed file {name!r} needs a lowercase SHA-256')
        if not isinstance(repository.get('license'), str) or not repository['license']:
            problem(errors, where + ' needs a license expression or unresolved marker')
        if not isinstance(repository.get('redistributed'), bool):
            problem(errors, where + ' needs an explicit redistributed boolean')
        tag = repository.get('tag')
        if tag is not None:
            if not isinstance(tag, dict) or set(tag) != {'name', 'commit', 'type'}:
                problem(errors, where + ' tag needs exactly name, commit and type')
            elif tag['name'] in ('HEAD', 'master', 'main', 'latest') or not isinstance(tag['name'], str) or not tag['name']:
                problem(errors, where + ' records a mutable tag name')
            elif not IMMUTABLE_REF.match(str(tag['commit'])) or tag['commit'] != commit:
                problem(errors, where + ' tag does not map to the recorded commit')
            elif tag['type'] not in TAG_TYPES:
                problem(errors, where + ' tag type must be lightweight or annotated')
        history = repository.get('tag_history')
        if history is not None:
            names = set()
            if not isinstance(history, list) or not history:
                problem(errors, where + ' tag_history must be a non-empty list')
            else:
                for entry in history:
                    if not isinstance(entry, dict) or set(entry) != {'name', 'commit', 'type'}:
                        problem(errors, where + ' tag_history entries need exactly name, commit and type')
                        continue
                    if not isinstance(entry['name'], str) or not entry['name'] or entry['name'] in ('HEAD', 'master', 'main', 'latest') or entry['name'] in names:
                        problem(errors, where + ' tag_history has an invalid or duplicate tag name')
                    names.add(entry['name'])
                    if not IMMUTABLE_REF.match(str(entry['commit'])) or entry['type'] not in TAG_TYPES:
                        problem(errors, where + ' tag_history needs immutable commits and valid types')
                if tag is not None and isinstance(tag, dict) and tag.get('name') in names:
                    problem(errors, where + ' tag_history repeats the accepted tag')
        known[url] = repository
    return known


def known_revisions(repository):
    revisions = {repository.get('commit')}
    if repository.get('head'):
        revisions.add(repository['head'])
    tag = repository.get('tag')
    if isinstance(tag, dict):
        revisions.add(tag.get('commit'))
    for entry in repository.get('tag_history', []):
        if isinstance(entry, dict):
            revisions.add(entry.get('commit'))
    return revisions


def check_assets(lock, repositories, errors):
    assets = lock.get('assets')
    if not isinstance(assets, list) or not assets:
        problem(errors, 'assets must be a non-empty list')
        return {}
    approved = lock.get('approved_physical_manifest_sha256')
    if not isinstance(approved, list) or any(not SHA256.match(str(item)) for item in approved):
        problem(errors, 'approved_physical_manifest_sha256 must be a list of SHA-256 values')
        approved = []
    known = {}
    for index, asset in enumerate(assets):
        where = f'assets[{index}]'
        if not isinstance(asset, dict):
            problem(errors, where + ' must be an object')
            continue
        name = asset.get('name')
        if not isinstance(name, str) or not name or name in known:
            problem(errors, where + ' needs a unique name')
            continue
        known[name] = asset
        if not https_url(asset.get('url')):
            problem(errors, where + ' needs a pinned HTTPS URL without mutable segments')
        size = asset.get('size')
        if type(size) is not int or size <= 0:
            problem(errors, where + ' needs a positive integer size')
        digest = asset.get('sha256')
        if not isinstance(digest, str) or not SHA256.match(digest):
            problem(errors, where + ' needs a lowercase 64-digit SHA-256')
        source = asset.get('source_repository')
        source_commit = asset.get('source_commit')
        if source not in repositories:
            problem(errors, where + ' source_repository is not a reviewed repository')
        if not isinstance(source_commit, str) or not IMMUTABLE_REF.match(source_commit):
            problem(errors, where + ' needs an immutable source_commit')
        elif source in repositories and source_commit not in known_revisions(repositories[source]):
            problem(errors, where + ' source_commit is not a recorded revision of its repository')
        built_from = asset.get('built_from')
        if built_from is not None:
            if not isinstance(built_from, dict) or not https_url(built_from.get('repository')):
                problem(errors, where + ' built_from needs an HTTPS repository')
            elif built_from['repository'] not in repositories:
                problem(errors, where + ' built_from repository is not reviewed')
            elif not IMMUTABLE_REF.match(str(built_from.get('commit'))):
                problem(errors, where + ' built_from needs an immutable commit')
            elif built_from['commit'] not in known_revisions(repositories[built_from['repository']]):
                problem(errors, where + ' built_from commit is not a recorded revision of its repository')
        if not isinstance(asset.get('license'), str) or not asset['license']:
            problem(errors, where + ' needs a license expression or unresolved marker')
        if asset.get('reproducibility') not in REPRODUCIBILITY:
            problem(errors, where + ' needs a reproducibility classification')
        if asset.get('status') not in STATUSES:
            problem(errors, where + ' needs a status classification')
        if not isinstance(asset.get('download_verified'), bool):
            problem(errors, where + ' needs an explicit download_verified boolean')
        if not isinstance(asset.get('flash_approved'), bool):
            problem(errors, where + ' needs an explicit flash_approved boolean')
            continue
        if asset['flash_approved']:
            if not approved:
                problem(errors, where + ' cannot be flash-approved without an approved manifest hash')
            if asset.get('status') not in ('VERIFIED', 'VERIFIED BUT PREBUILT'):
                problem(errors, where + ' cannot be flash-approved from an unverified status')
            if asset.get('reproducibility') not in ('REPRODUCIBLE FROM PINNED SOURCE', 'PINNED PREBUILT BINARY'):
                problem(errors, where + ' cannot be flash-approved with incomplete reproducibility')
            if not asset.get('provenance'):
                problem(errors, where + ' cannot be flash-approved without provenance notes')
    return known


def check_derived(lock, assets, errors):
    derived = lock.get('derived_artifacts')
    if derived is None:
        return {}
    if not isinstance(derived, list):
        problem(errors, 'derived_artifacts must be a list')
        return {}
    known = {}
    for index, artifact in enumerate(derived):
        where = f'derived_artifacts[{index}]'
        if not isinstance(artifact, dict):
            problem(errors, where + ' must be an object')
            continue
        name = artifact.get('name')
        if not isinstance(name, str) or not name or name in known or name in assets:
            problem(errors, where + ' needs a unique name')
            continue
        known[name] = artifact
        if artifact.get('from_asset') not in assets:
            problem(errors, where + ' must name a locked source asset')
        path = artifact.get('path')
        transform = artifact.get('transform')
        if path is None and transform is None:
            problem(errors, where + ' needs an archive path or a deterministic transform')
        if path is not None and (not isinstance(path, str) or not path or path.startswith('/') or '..' in path.split('/')):
            problem(errors, where + ' needs a safe archive-relative path')
        if transform is not None and (not isinstance(transform, str) or not transform):
            problem(errors, where + ' transform must be a non-empty description')
        size = artifact.get('size')
        if type(size) is not int or size <= 0:
            problem(errors, where + ' needs a positive integer size')
        digest = artifact.get('sha256')
        if not isinstance(digest, str) or not SHA256.match(digest):
            problem(errors, where + ' needs a lowercase 64-digit SHA-256')
        if artifact.get('status') not in STATUSES:
            problem(errors, where + ' needs a status classification')
        if not isinstance(artifact.get('notes'), str) or not artifact['notes']:
            problem(errors, where + ' must explain how it is derived')
    return known


def check_physical_roles(lock, assets, derived, errors):
    roles = lock.get('physical_roles')
    if not isinstance(roles, list) or len(roles) != len(ROLES):
        problem(errors, 'physical_roles must list exactly the eight release roles')
        return
    seen = {}
    for index, entry in enumerate(roles):
        where = f'physical_roles[{index}]'
        if not isinstance(entry, dict):
            problem(errors, where + ' must be an object')
            continue
        role = entry.get('role')
        if role not in ROLES:
            problem(errors, where + ' names an unknown role')
            continue
        if role in seen:
            problem(errors, where + ' duplicates role ' + role)
        seen[role] = entry
        if entry.get('status') not in STATUSES:
            problem(errors, where + ' needs a status classification')
        if not isinstance(entry.get('purpose'), str) or not entry['purpose']:
            problem(errors, where + ' needs a purpose')
        artifact = entry.get('artifact')
        if artifact is None:
            if entry.get('status') not in ('PARTIAL', 'BLOCKED', 'UNKNOWN'):
                problem(errors, where + ' may only omit an artifact as PARTIAL, BLOCKED or UNKNOWN')
            if not isinstance(entry.get('notes'), str) or not entry['notes']:
                problem(errors, where + ' must explain the unresolved input')
        elif artifact in derived:
            if entry.get('status') == 'VERIFIED' and derived[artifact].get('status') not in ('VERIFIED', 'VERIFIED BUT PREBUILT'):
                problem(errors, where + ' is VERIFIED while its derived artifact is not')
        elif artifact in assets:
            if entry.get('status') == 'VERIFIED' and assets[artifact].get('status') not in ('VERIFIED', 'VERIFIED BUT PREBUILT'):
                problem(errors, where + ' is VERIFIED while its asset is not')
            if not assets[artifact].get('download_verified'):
                problem(errors, where + ' references an asset that was never downloaded and verified')
        else:
            problem(errors, where + ' references an unknown artifact')
    for role in ROLES:
        if role not in seen:
            problem(errors, 'physical_roles is missing role ' + role)


def check_lock(lock):
    errors = []
    if not isinstance(lock, dict):
        return ['lock must be a JSON object']
    if lock.get('schema_version') != 2:
        problem(errors, 'schema_version must be 2')
    if not isinstance(lock.get('reviewed_on'), str) or not re.match(r'^\d{4}-\d{2}-\d{2}$', lock.get('reviewed_on', '')):
        problem(errors, 'reviewed_on must be an ISO date')
    repositories = check_repositories(lock, errors)
    assets = check_assets(lock, repositories, errors)
    derived = check_derived(lock, assets, errors)
    check_physical_roles(lock, assets, derived, errors)
    return errors


def check_inputs(lock):
    errors = []
    if not isinstance(lock, dict):
        return ['image input lock must be a JSON object']
    if lock.get('schema_version') != 2:
        problem(errors, 'image input lock schema_version must be 2')
    if lock.get('architecture') != 'armhf' or lock.get('distribution') != 'trixie':
        problem(errors, 'image input lock must target armhf trixie')
    dependencies = lock.get('dependencies')
    if not isinstance(dependencies, dict) or not dependencies:
        problem(errors, 'image input lock needs resolved dependency provenance')
    else:
        for name, record in dependencies.items():
            where = 'dependencies.' + str(name)
            if not isinstance(record, dict) or not https_url(record.get('repository')):
                problem(errors, where + ' needs an HTTPS repository')
                continue
            commit = record.get('commit')
            if commit is not None and not IMMUTABLE_REF.match(str(commit)):
                problem(errors, where + ' commit must be a full 40-digit lowercase commit')
            tag = record.get('tag')
            if tag is None and commit is None:
                problem(errors, where + ' needs a commit or an explicit unresolved marker')
    for field in ('container_digest', 'debian_snapshot', 'chip_snapshot_sha256', 'package_lock_sha256', 'vitrallis_bundle_sha256'):
        value = lock.get(field)
        if value is not None and not (isinstance(value, str) and value):
            problem(errors, 'image input lock field ' + field + ' must be null or a non-empty string')
    return errors


def check_evidence(requirements_path=REQUIREMENTS_PATH):
    errors = []
    requirements = load_document(requirements_path)
    if requirements.get('schema_version') != 1:
        problem(errors, 'kernel requirements schema_version must be 1')
    config_path = ROOT / requirements.get('config', '')
    if not config_path.is_file():
        problem(errors, 'kernel requirements config file is missing')
        return errors, requirements
    digest = requirements.get('config_sha256')
    actual = sha256_file(config_path)
    if actual != digest:
        problem(errors, f'kernel config evidence hash mismatch: recorded {digest}, actual {actual}')
    required = requirements.get('required')
    if not isinstance(required, dict) or not required:
        problem(errors, 'kernel requirements need a required symbol map')
    else:
        values = parse_kernel_config(config_path)
        for symbol, expected in required.items():
            if values.get(symbol) != expected:
                problem(errors, f'kernel config {symbol} is {values.get(symbol)!r}, expected {expected!r}')
    package = requirements.get('kernel_package')
    if not isinstance(package, dict):
        problem(errors, 'kernel requirements need kernel_package provenance')
    else:
        for field in ('name', 'version', 'source_repository', 'source_commit'):
            if not package.get(field):
                problem(errors, 'kernel package provenance is missing ' + field)
        if package.get('source_commit') is not None and not IMMUTABLE_REF.match(str(package['source_commit'])):
            problem(errors, 'kernel package provenance needs an immutable source commit')
        if package.get('source_repository') is not None and not https_url(package['source_repository']):
            problem(errors, 'kernel package provenance needs an HTTPS repository')
    source = requirements.get('config_source')
    if not isinstance(source, dict):
        problem(errors, 'kernel requirements need config_source provenance')
    else:
        for field in ('asset', 'url', 'path', 'sha256'):
            if not source.get(field):
                problem(errors, 'kernel config source is missing ' + field)
        if source.get('sha256') is not None and not SHA256.match(str(source['sha256'])):
            problem(errors, 'kernel config source needs a lowercase SHA-256')
        if source.get('url') is not None and not https_url(source['url']):
            problem(errors, 'kernel config source needs a pinned HTTPS URL')
    return errors, requirements


def parse_kernel_config(path):
    values = {}
    for line in pathlib.Path(path).read_text().splitlines():
        if line.startswith('CONFIG_'):
            name, _, value = line.partition('=')
            values[name] = value
        elif line.startswith('# CONFIG_') and line.endswith(' is not set'):
            values[line[len('# '):-len(' is not set')]] = 'n'
    return values


def check_kernel_config(config_path, requirements):
    values = parse_kernel_config(config_path)
    failures = []
    for symbol, expected in requirements['required'].items():
        if values.get(symbol) != expected:
            failures.append(f'{symbol}={values.get(symbol, "absent")} expected {expected}')
    return failures


def check_compatibility(lock, requirements, path=None):
    errors = []
    path = pathlib.Path(path) if path is not None else ROOT / 'images/compatibility.json'
    if not path.is_file():
        problem(errors, 'compatibility evidence file is missing')
        return errors
    compatibility = load_document(path)
    if compatibility.get('schema_version') != 1:
        problem(errors, 'compatibility evidence schema_version must be 1')
    derived = {artifact.get('name'): artifact for artifact in lock.get('derived_artifacts', []) if isinstance(artifact, dict)}
    assets = {asset.get('name'): asset for asset in lock.get('assets', []) if isinstance(asset, dict)}
    for key in ('dtb', 'overlay', 'boot_script'):
        record = compatibility.get(key)
        if not isinstance(record, dict) or not record.get('artifact') or not SHA256.match(str(record.get('sha256'))):
            problem(errors, 'compatibility evidence ' + key + ' needs an artifact and SHA-256')
            continue
        artifact = derived.get(record['artifact']) or assets.get(record['artifact'])
        if artifact is None:
            problem(errors, 'compatibility evidence references an unlocked artifact: ' + str(record['artifact']))
        elif artifact.get('sha256') != record['sha256']:
            problem(errors, 'compatibility evidence disagrees with the lock for ' + str(record['artifact']))
    labels = compatibility.get('overlay_required_labels')
    symbols = compatibility.get('dtb_symbols')
    if not isinstance(labels, list) or not labels or not isinstance(symbols, list) or not symbols:
        problem(errors, 'compatibility evidence needs overlay_required_labels and dtb_symbols')
    else:
        missing = [label for label in labels if label not in symbols]
        if missing:
            problem(errors, 'compatibility evidence is missing DTB labels: ' + ', '.join(missing))
    selection = compatibility.get('overlay_selection')
    if (
        not isinstance(selection, dict)
        or selection.get('eeprom_magic') != 'CHIP'
        or type(selection.get('pid_offset')) is not int
        or not isinstance(selection.get('pocketchip_pid'), list)
        or len(selection['pocketchip_pid']) != 2
    ):
        problem(errors, 'compatibility evidence needs a valid overlay DIP selection block')
    release = compatibility.get('kernel_release')
    package = requirements.get('kernel_package', {}) if isinstance(requirements, dict) else {}
    config = requirements.get('config', '') if isinstance(requirements, dict) else ''
    if not release or release not in package.get('name', '') or release not in config:
        problem(errors, 'compatibility kernel_release disagrees with the checked-in kernel evidence')
    return errors


def cross_checks(lock, inputs, requirements):
    errors = []
    assets = {asset.get('name'): asset for asset in lock.get('assets', []) if isinstance(asset, dict)}
    source = requirements.get('config_source') if isinstance(requirements, dict) else None
    if isinstance(source, dict):
        asset = assets.get(source.get('asset'))
        if asset is None:
            problem(errors, 'kernel config_source asset is not locked in upstream-lock.json')
        elif asset.get('sha256') != source.get('sha256') or asset.get('url') != source.get('url'):
            problem(errors, 'kernel config_source disagrees with the locked asset')
    repositories = {repository.get('repository'): repository for repository in lock.get('repositories', []) if isinstance(repository, dict)}
    chip = repositories.get(inputs.get('source_repository'))
    if chip is None:
        problem(errors, 'image source repository is not locked in upstream-lock.json')
    elif chip.get('commit') != inputs.get('source_commit'):
        problem(errors, 'image source commit disagrees with upstream-lock.json')
    for name, record in inputs.get('dependencies', {}).items():
        repository = repositories.get(record.get('repository'))
        if repository is None:
            problem(errors, 'image dependency repository is not locked: ' + str(name))
        elif record.get('commit') not in known_revisions(repository):
            problem(errors, 'image dependency commit is not a recorded revision: ' + str(name))
    storage = ROOT / 'docs/storage-inspection.json'
    if storage.is_file():
        recorded = json.loads(storage.read_text()).get('sha256')
        rootfs = assets.get('pocketchip-rootfs.tar.gz')
        if rootfs is not None and recorded != rootfs.get('sha256'):
            problem(errors, 'storage inspection rootfs hash disagrees with the lock')
    return errors


def report(lock):
    lines = [
        '# Artifact provenance matrix',
        '',
        'Generated by `python3 scripts/provenance.py report`. The lock file is the source of truth;',
        '`python3 scripts/provenance.py check` validates it offline.',
        '',
        '| Role | Artifact | Source | Commit | SHA-256 | Status | Reproducibility | License |',
        '| --- | --- | --- | --- | --- | --- | --- | --- |',
    ]
    assets = {asset['name']: asset for asset in lock['assets']}
    derived = {artifact['name']: artifact for artifact in lock.get('derived_artifacts', [])}
    for entry in lock['physical_roles']:
        name = entry.get('artifact')
        if name in derived:
            artifact = derived[name]
            source = assets.get(artifact['from_asset'], {})
            row = {
                'source': source.get('source_repository', '').replace('https://github.com/', '') + ' (derived)',
                'commit': (source.get('source_commit') or '')[:12],
                'sha': artifact.get('sha256', '')[:16],
                'repro': 'DERIVED FROM PINNED ARCHIVE',
                'license': source.get('license', ''),
            }
        else:
            artifact = assets.get(name, {})
            row = {
                'source': artifact.get('source_repository', '').replace('https://github.com/', ''),
                'commit': (artifact.get('source_commit') or '')[:12],
                'sha': (artifact.get('sha256') or '')[:16],
                'repro': artifact.get('reproducibility', ''),
                'license': artifact.get('license', ''),
            }
        lines.append('| {role} | {artifact} | {source} | {commit} | {sha} | {status} | {repro} | {license} |'.format(
            role=entry['role'],
            artifact=name or 'UNRESOLVED',
            status=entry.get('status', ''),
            **row,
        ))
    return '\n'.join(lines) + '\n'


def verify_upstream(lock):
    failures = []
    for repository in lock['repositories']:
        tag = repository.get('tag')
        if tag is None:
            continue
        output = subprocess.run(
            ['git', 'ls-remote', repository['repository'], 'refs/tags/' + tag['name'] + '*'],
            check=True, capture_output=True, text=True, timeout=120,
        ).stdout.splitlines()
        resolved = None
        for line in output:
            ref, _, commit = line.partition('\t')
            if ref.endswith('^{}'):
                resolved = commit
            elif resolved is None:
                resolved = commit
        if resolved != tag['commit']:
            failures.append(f'{repository["repository"]} tag {tag["name"]} resolves to {resolved}, recorded {tag["commit"]}')
    for asset in lock['assets']:
        digest = hashlib.sha256()
        size = 0
        request = urllib.request.Request(asset['url'], headers={'Accept-Encoding': 'identity'})
        with urllib.request.urlopen(request, timeout=600) as response:
            if response.status != 200:
                failures.append(f'{asset["name"]}: HTTP {response.status}')
                continue
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                size += len(chunk)
                if size > asset['size']:
                    break
                digest.update(chunk)
        if size != asset['size']:
            failures.append(f'{asset["name"]}: size {size}, recorded {asset["size"]}')
        elif digest.hexdigest() != asset['sha256']:
            failures.append(f'{asset["name"]}: SHA-256 mismatch')
        else:
            print(f'verified {asset["name"]} ({size} bytes)')
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['check', 'report', 'verify-upstream', 'check-kernel-config'])
    parser.add_argument('--lock', type=pathlib.Path, default=LOCK_PATH)
    parser.add_argument('--inputs', type=pathlib.Path, default=INPUTS_PATH)
    parser.add_argument('--requirements', type=pathlib.Path, default=REQUIREMENTS_PATH)
    parser.add_argument('--config', type=pathlib.Path)
    args = parser.parse_args()
    if args.command == 'verify-upstream':
        lock = load_document(args.lock)
        failures = verify_upstream(lock)
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1 if failures else 0
    if args.command == 'check-kernel-config':
        requirements = load_document(args.requirements)
        config = args.config or ROOT / requirements['config']
        failures = check_kernel_config(config, requirements)
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1 if failures else 0
    lock = load_document(args.lock)
    if args.command == 'report':
        print(report(lock))
        return 0
    inputs = load_document(args.inputs)
    errors = check_lock(lock)
    errors.extend(check_inputs(inputs))
    evidence_errors, requirements = check_evidence(args.requirements)
    errors.extend(evidence_errors)
    errors.extend(cross_checks(lock, inputs, requirements))
    errors.extend(check_compatibility(lock, requirements))
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        return 1
    print(
        'provenance lock valid: '
        f'{len(lock["repositories"])} repositories, {len(lock["assets"])} assets, '
        f'{len(lock["physical_roles"])} release roles, offline evidence verified'
    )
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
