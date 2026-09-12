#!/usr/bin/env python3
"""Deterministically repack a prepared Linux rootfs; never extract an archive."""
import argparse
import gzip
import hashlib
import os
import pathlib
import stat
import tarfile
import tempfile


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
