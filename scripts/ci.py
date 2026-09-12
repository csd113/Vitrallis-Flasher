#!/usr/bin/env python3
"""Reproducible validation/package entry points using structured argv only."""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import plistlib
import shutil
import subprocess
import sys
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGETS = {
    ('Windows', 'AMD64'): 'x86_64-pc-windows-msvc',
    ('Darwin', 'arm64'): 'aarch64-apple-darwin',
    ('Darwin', 'x86_64'): 'x86_64-apple-darwin',
    ('Linux', 'x86_64'): 'x86_64-unknown-linux-gnu',
    ('Linux', 'aarch64'): 'aarch64-unknown-linux-gnu',
}


def run(argv, timeout=1800):
    subprocess.run(argv, cwd=ROOT, check=True, timeout=timeout)


def validate():
    run(['cargo', 'fmt', '--all', '--check'])
    run(['cargo', 'clippy', '--workspace', '--all-targets', '--all-features', '--locked', '--', '-D', 'warnings', '-D', 'clippy::all', '-D', 'clippy::pedantic', '-D', 'clippy::nursery', '-D', 'clippy::cargo'])
    run(['cargo', 'test', '--workspace', '--all-features', '--locked'])
    run([sys.executable, '-m', 'unittest', 'discover', '-s', 'images', '-p', 'test_*.py'])
    run([sys.executable, '-m', 'unittest', 'discover', '-s', 'scripts', '-p', 'test_*.py'])
    run([sys.executable, 'images/build.py', 'plan'])
    run([sys.executable, 'images/build.py', 'plan', '--profile', 'vitrallis-default'])
    run(['cargo', 'build', '--workspace', '--all-features', '--release', '--locked'])
    run(['cargo', 'run', '--locked', '-p', 'flasher-cli', '--', 'validate', 'manifests/simulation.json'])
    run(['git', 'diff', '--check'])


def package(target=None, bin_dir=None):
    target = target or TARGETS.get((platform.system(), platform.machine()))
    if target not in TARGETS.values():
        raise ValueError('unsupported packaging host')
    metadata = subprocess.check_output(['cargo','metadata','--locked','--format-version','1'],cwd=ROOT,text=True)
    deps = json.loads(metadata)
    build_directory = bin_dir.resolve(strict=True) if bin_dir else pathlib.Path(deps['target_directory']) / 'release'
    output = ROOT / 'dist'
    output.mkdir(exist_ok=True)
    if output.is_symlink():
        raise ValueError('dist must not be a symlink')
    with tempfile.TemporaryDirectory(dir=output, prefix='.stage-') as staging:
        destination = pathlib.Path(staging) / ('vitrallis-flasher-' + target)
        destination.mkdir()
        extension = '.exe' if target.endswith('windows-msvc') else ''
        for name in ['flasher-cli', 'flasher-gui']:
            source = build_directory / (name + extension)
            if not source.is_file() or source.is_symlink():
                raise ValueError('build the release binaries first')
            verify_binary(source, target)
            shutil.copy2(source, destination / source.name)
        if target.endswith('apple-darwin'):
            contents = destination / 'Vitrallis Flasher.app/Contents'
            (contents / 'MacOS').mkdir(parents=True)
            shutil.copy2(destination / 'flasher-gui', contents / 'MacOS/flasher-gui')
            with (contents / 'Info.plist').open('wb') as plist:
                plistlib.dump({'CFBundleExecutable':'flasher-gui','CFBundleIdentifier':'com.vitrallis.flasher','CFBundleName':'Vitrallis Flasher','CFBundlePackageType':'APPL','CFBundleShortVersionString':'0.1.0','NSHighResolutionCapable':True}, plist)
        for filename in ['README.md', 'to-do.md', 'CONTRIBUTING.md', 'SECURITY.md', 'upstream-lock.json', 'Cargo.lock']:
            shutil.copy2(ROOT / filename, destination / filename)
        shutil.copytree(ROOT / 'docs', destination / 'docs')
        shutil.copytree(ROOT / 'manifests', destination / 'manifests')
        shutil.copytree(ROOT / 'fixtures', destination / 'fixtures')
        (destination / 'scripts').mkdir()
        shutil.copy2(ROOT / 'scripts/upgrade_debian13.py', destination / 'scripts/upgrade_debian13.py')
        (destination / 'images').mkdir()
        for filename in ['build.py', 'inputs.lock.json', 'storage.py']:
            shutil.copy2(ROOT / 'images' / filename, destination / 'images' / filename)
        shutil.copytree(ROOT / 'images/storage-candidates', destination / 'images/storage-candidates')
        (destination / 'dependency-licenses.json').write_text(json.dumps([{'name':p['name'],'version':p['version'],'license':p['license'],'repository':p['repository']} for p in deps['packages']], indent=2)+'\n')
        # Include available upstream license texts for registry crates used by these builds.
        notices = destination / 'third-party-licenses'
        notices.mkdir()
        for package_info in deps['packages']:
            if not package_info['source']:
                continue
            source = pathlib.Path(package_info['manifest_path']).parent
            texts = [p for p in source.iterdir() if p.is_file() and p.name.upper().startswith(('LICENSE','COPYING','NOTICE','COPYRIGHT'))]
            if texts:
                folder = notices / (package_info['name']+'-'+package_info['version'])
                folder.mkdir(exist_ok=True)
                for text in texts:
                    shutil.copy2(text, folder / text.name)
        inventory = []
        for path in sorted(destination.rglob('*')):
            if path.is_file():
                with path.open('rb') as stream:
                    inventory.append(hashlib.file_digest(stream,'sha256').hexdigest()+'  '+path.relative_to(destination).as_posix())
        (destination / 'SHA256SUMS').write_text('\n'.join(inventory)+'\n')
        archive = output / (destination.name + '.zip')
        publish_zip(destination, archive)
        print(archive)



def verify_binary(path, target):
    """Reject a mislabeled target before packaging, including inherited target-dir overrides."""
    import struct
    with path.open('rb') as binary:
        header = binary.read(64)
        valid = False
        if len(header) == 64:
            if target.endswith('linux-gnu'):
                machine = 183 if target.startswith('aarch64') else 62
                valid = header[:6] == b'\x7fELF\x02\x01' and struct.unpack_from('<H',header,18)[0] == machine
            elif target.endswith('apple-darwin'):
                machine = 0x0100000C if target.startswith('aarch64') else 0x01000007
                valid = header[:4] == b'\xcf\xfa\xed\xfe' and struct.unpack_from('<I',header,4)[0] == machine
            elif target == 'x86_64-pc-windows-msvc' and header[:2] == b'MZ':
                offset = struct.unpack_from('<I',header,60)[0]
                if 64 <= offset <= 1048576:
                    binary.seek(offset)
                    signature = binary.read(6)
                    valid = signature == b'PE\x00\x00\x64\x86'
        if not valid:
            raise ValueError('binary format/architecture differs from packaging target: '+str(path))


def publish_zip(destination, archive):
    with tempfile.NamedTemporaryFile(dir=archive.parent, prefix='.package-', delete=False) as temporary:
        temporary_path = pathlib.Path(temporary.name)
        try:
            with zipfile.ZipFile(temporary,'w',compression=zipfile.ZIP_DEFLATED,strict_timestamps=False) as bundle:
                for path in sorted(destination.rglob('*')):
                    if path.is_file():
                        bundle.write(path, destination.name+'/'+path.relative_to(destination).as_posix())
            temporary.flush()
            os.fsync(temporary.fileno())
            temporary.close()
            os.link(temporary_path, archive)  # atomic no-clobber publication
        finally:
            temporary.close()
            temporary_path.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command',choices=['setup','validate','package'])
    parser.add_argument('--target', choices=sorted(set(TARGETS.values())))
    parser.add_argument('--bin-dir', type=pathlib.Path)
    args = parser.parse_args()
    if args.command != 'package' and (args.target or args.bin_dir):
        parser.error('target/bin-dir apply only to package')
    os.chdir(ROOT)
    if args.command == 'setup':
        run(['rustup','toolchain','install','1.98.1','--profile','minimal','--component','rustfmt,clippy'])
        if platform.system() == 'Linux':
            run(['sudo','apt-get','update'])
            run(['sudo','apt-get','install','-y','pkg-config','libx11-dev','libxi-dev','libxcursor-dev','libxrandr-dev','libxinerama-dev','libgl1-mesa-dev','libegl1-mesa-dev','libwayland-dev','libxkbcommon-dev'])
    elif args.command == 'validate':
        validate()
    else:
        package(args.target, args.bin_dir)


if __name__ == '__main__':
    main()
