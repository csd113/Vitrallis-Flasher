#!/usr/bin/env python3
"""Deterministically repack a prepared Linux rootfs; never extract an archive.

Two entry points share one normalization contract:

* `repack(source, destination, epoch)` rewrites an already extracted tree.
* `repack_archive(source, destination, epoch)` streams a gzip tar archive
  member by member. It exists for the Batch 1 pipeline, where the pinned
  upstream rootfs archive carries numeric ownership, char devices and
  hardlinks that a non-root extraction cannot faithfully reproduce. The
  archive form preserves them from the source headers instead of the host
  filesystem.

Both normalize mtime, owner/group names and gzip headers, preserve numeric
ownership/modes, and fail closed on unsafe paths and special files.
"""
import argparse
import gzip
import hashlib
import json
import os
import pathlib
import stat
import struct
import tarfile
import tempfile

MAX_MEMBERS = 200000
MAX_BYTES = 8 * 1024 * 1024 * 1024
TYPE_CODES = {
    tarfile.DIRTYPE: 1,
    tarfile.REGTYPE: 2,
    tarfile.SYMTYPE: 3,
    tarfile.LNKTYPE: 4,
    tarfile.CHRTYPE: 5,
    tarfile.BLKTYPE: 6,
}


class RepackError(ValueError):
    """An archive cannot be repacked under the normalization contract."""


def _safe_name(name):
    if not isinstance(name, str) or not name or "\x00" in name or name.startswith("/"):
        raise RepackError("unsafe archive member path: " + repr(name))
    if name in (".", "./"):
        return name
    stripped = name[2:] if name.startswith("./") else name
    if any(part in ("", ".", "..") for part in stripped.split("/")):
        raise RepackError("archive member path escapes the tree: " + repr(name))
    return name


def _safe_symlink_target(linkname):
    if not isinstance(linkname, str) or not linkname or "\x00" in linkname:
        raise RepackError("unsafe symlink target: " + repr(linkname))
    return linkname


def _safe_hardlink_target(linkname):
    if not isinstance(linkname, str) or not linkname or "\x00" in linkname or linkname.startswith("/"):
        raise RepackError("unsafe hardlink target: " + repr(linkname))
    stripped = linkname[2:] if linkname.startswith("./") else linkname
    if not stripped or any(part in ("", ".", "..") for part in stripped.split("/")):
        raise RepackError("hardlink target escapes the tree: " + repr(linkname))
    return linkname


def _field(value):
    return struct.pack(">I", len(value)) + value


def _canonical(member, content_sha):
    code = TYPE_CODES.get(member.type)
    if code is None:
        raise RepackError("unsupported archive member type")
    size = member.size if member.isfile() else 0
    return b"".join(
        [
            _field(member.name.encode("utf-8")),
            _field(bytes([code])),
            _field(struct.pack(">I", member.mode & 0o7777)),
            _field(struct.pack(">I", member.uid)),
            _field(struct.pack(">I", member.gid)),
            _field(member.linkname.encode("utf-8") if member.linkname else b""),
            _field(struct.pack(">I", member.devmajor)),
            _field(struct.pack(">I", member.devminor)),
            _field(struct.pack(">Q", size)),
            _field(content_sha if content_sha is not None else b"\x00" * 32),
        ]
    )


def _convert_member(member, epoch):
    _safe_name(member.name)
    if member.issym():
        _safe_symlink_target(member.linkname)
    elif member.islnk():
        _safe_hardlink_target(member.linkname)
    if member.issparse() or set(member.pax_headers) - {"path", "linkpath"}:
        raise RepackError("archive member carries unsupported metadata: " + member.name)
    code = TYPE_CODES.get(member.type)
    if code is None:
        raise RepackError("special archive member requires explicit policy: " + member.name)
    if member.isdev() and (member.devmajor < 0 or member.devminor < 0):
        raise RepackError("device member without a major/minor: " + member.name)
    info = tarfile.TarInfo(member.name)
    info.type = member.type
    info.mode = member.mode & 0o7777
    info.uid = member.uid
    info.gid = member.gid
    info.mtime = epoch
    info.uname = ""
    info.gname = ""
    if member.issym() or member.islnk():
        info.linkname = member.linkname
    if member.isdev():
        info.devmajor = member.devmajor
        info.devminor = member.devminor
    if member.isfile():
        info.size = member.size
    return info


def repack_archive(source, destination, epoch):
    """Deterministically rewrite a gzip tar archive without extracting it.

    Returns a summary dict with the output SHA-256, member count, total file
    bytes and the canonical manifest digest that must match the source.
    """
    source = pathlib.Path(source)
    destination = pathlib.Path(destination)
    if source.is_symlink() or not source.is_file():
        raise RepackError("regular source archive required")
    if destination.exists() or destination.is_symlink():
        raise RepackError("new output file required")
    if not 0 < epoch < 2**32:
        raise RepackError("invalid SOURCE_DATE_EPOCH")
    parent = destination.parent.resolve(strict=True)
    source = source.resolve(strict=True)
    if parent == source or source in parent.parents:
        raise RepackError("output must be outside the source archive")
    source_manifest = hashlib.sha256()
    output_manifest = hashlib.sha256()
    members = 0
    total = 0
    seen = set()
    with tempfile.NamedTemporaryFile(dir=parent, delete=False) as temporary:
        tmp = pathlib.Path(temporary.name)
        try:
            with gzip.GzipFile(filename="", mode="wb", fileobj=temporary, mtime=0) as compressed:
                with tarfile.open(source, "r|gz") as archive, tarfile.open(
                    fileobj=compressed, mode="w|", format=tarfile.PAX_FORMAT
                ) as output:
                    for member in archive:
                        members += 1
                        if members > MAX_MEMBERS:
                            raise RepackError("excessive archive members")
                        if member.name in seen:
                            raise RepackError("duplicate archive member: " + member.name)
                        seen.add(member.name)
                        if member.isfile():
                            total += member.size
                            if total > MAX_BYTES:
                                raise RepackError("archive exceeds the expansion bound")
                        content_sha = None
                        info = _convert_member(member, epoch)
                        if member.isfile():
                            origin = archive.extractfile(member)
                            if origin is None:
                                raise RepackError("unreadable archive member: " + member.name)
                            digest = hashlib.sha256()
                            written = 0

                            class _Reader:
                                def read(self, size=-1):
                                    nonlocal written
                                    chunk = origin.read(size if size and size > 0 else 1024 * 1024)
                                    written += len(chunk)
                                    digest.update(chunk)
                                    return chunk

                            output.addfile(info, _Reader())
                            if written != member.size:
                                raise RepackError("member size mismatch: " + member.name)
                            content_sha = digest.digest()
                        else:
                            output.addfile(info)
                        output_manifest.update(_canonical(info, content_sha))
                        source_manifest.update(_canonical(member, content_sha))
            if source_manifest.digest() != output_manifest.digest():
                raise RepackError("repacked archive is not equivalent to the source")
            temporary.flush()
            os.fsync(temporary.fileno())
            temporary.close()
            os.link(tmp, destination)
        finally:
            temporary.close()
            tmp.unlink(missing_ok=True)
    with destination.open("rb") as content:
        output_sha = hashlib.file_digest(content, "sha256").hexdigest()
    return {
        "sha256": output_sha,
        "members": members,
        "bytes": total,
        "manifest_sha256": source_manifest.hexdigest(),
    }


def repack(source, destination, epoch):
    if source.is_symlink() or not source.is_dir() or destination.exists() or destination.is_symlink():
        raise ValueError("regular source directory and new output file required")
    source = source.resolve(strict=True)
    parent = destination.parent.resolve(strict=True)
    if parent == source or source in parent.parents:
        raise ValueError("output must be outside the source tree")
    if not 0 < epoch < 2**32:
        raise ValueError("invalid SOURCE_DATE_EPOCH")
    entries = []
    for root, directories, files in os.walk(source, followlinks=False):
        for name in sorted(directories + files):
            path = pathlib.Path(root) / name
            mode = path.lstat().st_mode
            if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode) or stat.S_ISLNK(mode)):
                raise ValueError("special filesystem entries require explicit image policy")
            entries.append(path)
            if len(entries) > 150000:
                raise ValueError("excessive rootfs entries")
    entries.sort(key=lambda p: p.relative_to(source).as_posix())
    # Caller owns the prepared source tree; it must not change during packaging.
    with tempfile.NamedTemporaryFile(dir=parent, delete=False) as temporary:
        tmp = pathlib.Path(temporary.name)
        try:
            with gzip.GzipFile(filename="", mode="wb", fileobj=temporary, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w|", format=tarfile.PAX_FORMAT) as archive:
                    for path in entries:
                        info = archive.gettarinfo(str(path), "./" + path.relative_to(source).as_posix())
                        info.mtime = epoch
                        info.uname = info.gname = ""
                        info.pax_headers = {}
                        if info.isfile():
                            with path.open("rb") as content:
                                archive.addfile(info, content)
                        else:
                            archive.addfile(info)
            temporary.flush()
            os.fsync(temporary.fileno())
            # Atomic no-clobber publication on the Linux image-builder host.
            temporary.close()
            os.link(tmp, destination)
        finally:
            temporary.close()
            tmp.unlink(missing_ok=True)
    with destination.open("rb") as content:
        return hashlib.file_digest(content, "sha256").hexdigest()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("destination", type=pathlib.Path)
    parser.add_argument("--epoch", type=int, required=True)
    args = parser.parse_args()
    print(repack(args.source, args.destination, args.epoch))
