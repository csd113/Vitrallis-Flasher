#!/usr/bin/env python3
"""Build a private session-bound RAM-only recovery image; never extract archives."""
import argparse
import gzip
import hashlib
import io
import json
import lzma
import os
import pathlib
import stat
import struct
import tarfile

import uimage

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASE_SHA256 = '5b8b392c095fd37472f9a08f1ed8f6cdbb6d6a04bb36a0696dbcd3033c0f3b25'
ROOTFS_SHA256 = '1e516cade3085633f61697d69a5d95cb84a501d8b606247987db5837a53e19ef'
MAX_ARCHIVE = 160 * 1024 * 1024
MAX_FILE = 32 * 1024 * 1024
MAX_ENTRIES = 20000
MODULE = 'usr/lib/modules/6.12.107+deb13-chip/kernel/drivers/nvmem/nvmem_sunxi_sid.ko'
WATCHDOG_MODULE = 'usr/lib/modules/6.12.107+deb13-chip/kernel/drivers/watchdog/sunxi_wdt.ko'
SELECTED = {
    'usr/bin/kmod',
    'usr/lib/arm-linux-gnueabihf/libkmod.so.2',
    'usr/lib/arm-linux-gnueabihf/libkmod.so.2.5.1',
    'usr/lib/arm-linux-gnueabihf/libcrypto.so.3',
    'usr/lib/arm-linux-gnueabihf/liblzma.so.5',
    'usr/lib/arm-linux-gnueabihf/liblzma.so.5.8.1',
    'usr/lib/arm-linux-gnueabihf/libzstd.so.1',
    'usr/lib/arm-linux-gnueabihf/libzstd.so.1.5.7',
    MODULE + '.xz',
    WATCHDOG_MODULE + '.xz',
}


def safe_path(name):
    if name == '.':
        return name
    if not name or name.startswith('/') or '\0' in name or any(p in ('', '.', '..') for p in name.split('/')):
        raise ValueError('unsafe cpio path')
    return name


def safe_link(name, target):
    if not target or '\0' in target:
        raise ValueError('unsafe link')
    parts = [] if target.startswith('/') else name.split('/')[:-1]
    for part in target.split('/'):
        if part in ('', '.'):
            continue
        if part == '..':
            if not parts:
                raise ValueError('link escapes root')
            parts.pop()
        else:
            parts.append(part)
    if not parts:
        raise ValueError('link targets root')


def cpio_entries(data):
    """Bounded newc parser, preserving original records without host extraction."""
    if len(data) > MAX_ARCHIVE:
        raise ValueError('cpio expansion bound')
    offset = 0
    seen = set()
    records = []
    while len(records) < MAX_ENTRIES:
        begin = offset
        if offset + 110 > len(data) or data[offset:offset + 6] != b'070701':
            raise ValueError('truncated or unsupported cpio header')
        try:
            fields = [int(data[offset + 6 + i * 8:offset + 14 + i * 8], 16) for i in range(13)]
        except ValueError as error:
            raise ValueError('malformed cpio integer') from error
        mode, size, namesize = fields[1], fields[6], fields[11]
        if not 1 <= namesize <= 4096 or size > MAX_FILE or fields[12] != 0:
            raise ValueError('cpio entry bound or checksum')
        name_start = offset + 110
        name_end = name_start + namesize
        if name_end > len(data) or data[name_end - 1] != 0:
            raise ValueError('truncated cpio name')
        name = data[name_start:name_end - 1].decode('utf-8', 'strict')
        offset = (name_end + 3) & ~3
        end = offset + size
        if end > len(data):
            raise ValueError('truncated cpio content')
        content = data[offset:end]
        offset = (end + 3) & ~3
        if offset > len(data):
            raise ValueError('truncated cpio padding')
        if name == 'TRAILER!!!':
            if size or any(data[offset:]):
                raise ValueError('cpio trailing payload')
            return records
        safe_path(name)
        if name in seen:
            raise ValueError('duplicate cpio name')
        seen.add(name)
        if stat.S_ISLNK(mode):
            safe_link(name, content.decode('utf-8', 'strict'))
        records.append((name, mode, content, data[begin:offset]))
    raise ValueError('cpio entry count bound')


def record(name, content, mode, inode):
    safe_path(name)
    name_bytes = name.encode() + b'\0'
    fields = [inode, mode, 0, 0, 1, 0, len(content), 0, 0, 0, 0, len(name_bytes), 0]
    header = b'070701' + ''.join(f'{v:08x}' for v in fields).encode()
    entry = header + name_bytes
    entry += bytes(-len(entry) % 4)
    entry += content
    entry += bytes(-len(entry) % 4)
    return entry


def regular(path, limit):
    path = pathlib.Path(path)
    for component in (path, *path.parents):
        if component.is_symlink():
            raise ValueError('symlink input path')
    if not path.is_file() or path.stat().st_size > limit:
        raise ValueError('input bound or file type')
    return path.read_bytes()


def digest_file(path):
    with pathlib.Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def rootfs_files(path):
    path = pathlib.Path(path)
    if any(component.is_symlink() for component in (path, *path.parents)) or not path.is_file() or path.stat().st_size != 516563033:
        raise ValueError('unsafe or wrongly sized rootfs input')
    if digest_file(path) != ROOTFS_SHA256:
        raise ValueError('rootfs does not match locked source')
    selected = {}
    count, total = 0, 0
    with tarfile.open(path, 'r|gz') as archive:
        for member in archive:
            count += 1
            total += member.size
            if count > 200000 or total > 8 * 1024 ** 3:
                raise ValueError('rootfs bounds')
            name = member.name.removeprefix('./')
            if name not in SELECTED:
                continue
            if name in selected or member.size > MAX_FILE:
                raise ValueError('duplicate or oversized selected file')
            if member.issym():
                safe_link(name, member.linkname)
                selected[name] = (stat.S_IFLNK | 0o777, member.linkname.encode())
            elif member.isfile():
                source = archive.extractfile(member)
                if source is None:
                    raise ValueError('missing selected file')
                content = source.read(MAX_FILE + 1)
                if len(content) != member.size:
                    raise ValueError('selected file truncated')
                selected[name] = (stat.S_IFREG | (member.mode & 0o777), content)
            else:
                raise ValueError('selected file is not regular or symlink')
    if set(selected) != SELECTED:
        raise ValueError('locked rootfs missing recovery runtime')
    for name in (MODULE, WATCHDOG_MODULE):
        compressed = selected.pop(name + '.xz')[1]
        with lzma.LZMAFile(io.BytesIO(compressed)) as stream:
            module = stream.read(MAX_FILE + 1)
        if len(module) > MAX_FILE or not module.startswith(b'\x7fELF'):
            raise ValueError('module decompression bound or format')
        selected[name] = (stat.S_IFREG | 0o644, module)
    return selected


def build(base, rootfs, daemon, output):
    """Validate all inputs before publishing into a new private directory."""
    base_bytes = regular(base, 40 * 1024 * 1024)
    if hashlib.sha256(base_bytes).hexdigest() != BASE_SHA256:
        raise ValueError('unreviewed recovery base')
    header, payload = uimage.parse(base_bytes)
    if header['type'] != 3 or header['compression'] != 1 or len(base_bytes) != len(payload) + 64:
        raise ValueError('unsupported recovery container')
    with gzip.GzipFile(fileobj=io.BytesIO(payload)) as stream:
        archive = stream.read(MAX_ARCHIVE + 1)
    original = cpio_entries(archive)
    binary = regular(daemon, MAX_FILE)
    if len(binary) < 52 or binary[:6] != b'\x7fELF\x01\x01' or struct.unpack_from('<H', binary, 18)[0] != 40:
        raise ValueError('daemon is not ARM Linux ELF')
    replacements = rootfs_files(rootfs)
    replacements['usr/sbin/insmod'] = replacements['usr/bin/kmod']
    replacements['init'] = (stat.S_IFREG | 0o755, regular(ROOT / 'recovery/init', 8192))
    replacements['usr/sbin/flasher-recovery'] = (stat.S_IFREG | 0o755, binary)
    output = pathlib.Path(output).absolute()
    if output.exists() or output.is_symlink() or any(p.is_symlink() for p in output.parents):
        raise ValueError('output must be a new directory without symlink parents')
    # No shared installer credentials or SSH daemon survives this template.
    parts = [raw for name, _, _, raw in original if name not in replacements and not name.startswith(('root/.ssh', 'etc/dropbear')) and name != 'usr/sbin/dropbear']
    existing = {name for name, _, _, _ in original}
    for name in sorted(replacements):
        parent = pathlib.PurePosixPath(name).parent
        parents = []
        while str(parent) != '.':
            parents.append(str(parent))
            parent = parent.parent
        for directory in reversed(parents):
            if directory not in existing:
                parts.append(record(directory, b'', stat.S_IFDIR | 0o755, 200000 + len(parts)))
                existing.add(directory)
        mode, content = replacements[name]
        parts.append(record(name, content, mode, 200000 + len(parts)))
    parts.append(record('TRAILER!!!', b'', 0, 0))
    cpio = b''.join(parts)
    cpio += bytes(-len(cpio) % 512)
    cpio_entries(cpio)  # final archive validates before output mutation
    image = uimage.build({'type': 3, 'compression': 1, 'name': 'Vitrallis recovery v1'}, gzip.compress(cpio, compresslevel=9, mtime=0))
    if len(image) > 40 * 1024 * 1024:
        raise ValueError('recovery RAM image bound')
    metadata = {'protocol': 2, 'sid': None, 'session_id': None, 'daemon_sha256': hashlib.sha256(binary).hexdigest(), 'image_sha256': hashlib.sha256(image).hexdigest(), 'image_bytes': len(image), 'base_sha256': BASE_SHA256, 'rootfs_sha256': ROOTFS_SHA256, 'operations': ['ping', 'inventory', 'boot-readback', 'return-to-fel'], 'nand_writes': False}
    output.mkdir(mode=0o700)
    files = [('initrd.uimage', image), ('metadata.json', (json.dumps(metadata, indent=2) + '\n').encode())]
    for name, data in files:
        fd = os.open(output / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    print(json.dumps(metadata, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True, type=pathlib.Path)
    parser.add_argument('--rootfs', required=True, type=pathlib.Path)
    parser.add_argument('--daemon', required=True, type=pathlib.Path)
    parser.add_argument('--output', required=True, type=pathlib.Path)
    args = parser.parse_args()
    build(args.base, args.rootfs, args.daemon, args.output)


if __name__ == '__main__':
    main()
