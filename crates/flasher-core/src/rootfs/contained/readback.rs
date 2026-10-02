//! Semantic verification through captured inodes; device nodes are never opened.
use super::{Root, check_character, proc_descriptors, validate_metadata};
use crate::{
    Cancellation, Error,
    rootfs::{Entry, Inspection, Kind},
};
use rustix::{
    fd::{AsRawFd, OwnedFd},
    fs::{self, FileType, Mode, OFlags},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
};

impl Root {
    /// Verify all installed contents, inode metadata and directory membership.
    ///
    /// Uses the retained root and captured no-follow leaf descriptors. Regular
    /// file reads reopen only verified regular inodes through trusted procfs;
    /// symlinks and character devices are never opened for content I/O.
    /// The filesystem must be quiescent for complete-tree verification.
    /// This is semantic readback, not physical NAND/UBI health authorization.
    /// # Errors
    /// Rejects missing/extra/changed entries, mount-device changes, mismatched
    /// links, content/metadata changes, cancellation or read failures.
    pub fn verify_rootfs(&self, expected: &Inspection, cancel: &Cancellation) -> Result<(), Error> {
        expected.installation_preflight(cancel)?;
        let inventory: BTreeMap<_, _> = expected
            .entries()
            .iter()
            .map(|e| (e.path.as_str(), e))
            .collect();
        let links = link_counts(expected, cancel)?;
        let device = fs::fstat(&self.directory)
            .map_err(std::io::Error::from)?
            .st_dev;
        let mut files = BTreeSet::new();
        let mut children = 0_usize;
        for entry in expected.entries() {
            cancel.check()?;
            let node = self.capture(&entry.path, cancel)?;
            let before = fs::fstat(&node).map_err(std::io::Error::from)?;
            if before.st_dev != device {
                return Err(Error::UnsafePath);
            }
            let kind = match entry.kind {
                Kind::Directory => FileType::Directory,
                Kind::File | Kind::Hardlink => FileType::RegularFile,
                Kind::Symlink => FileType::Symlink,
                Kind::Character => FileType::CharacterDevice,
            };
            validate_metadata(&before, entry, kind)?;
            match entry.kind {
                Kind::Directory => {
                    children = children
                        .checked_add(self.verify_children(entry, &node, &inventory, cancel)?)
                        .ok_or(Error::Length)?;
                }
                Kind::File => {
                    if !files.insert((before.st_dev, before.st_ino))
                        || u128::from(before.st_nlink)
                            != u128::from(*links.get(entry.path.as_str()).ok_or(Error::Length)?)
                    {
                        return Err(Error::UnsafePath);
                    }
                    verify_file(&node, entry, cancel)?;
                }
                Kind::Hardlink => {
                    let target = self.capture(&entry.link, cancel)?;
                    let target = fs::fstat(target).map_err(std::io::Error::from)?;
                    if (before.st_dev, before.st_ino) != (target.st_dev, target.st_ino) {
                        return Err(Error::UnsafePath);
                    }
                }
                Kind::Symlink => {
                    let mut bytes = [0; super::super::MAX_PATH + 1];
                    let count = fs::readlinkat_raw(&node, "", &mut bytes[..])
                        .map_err(std::io::Error::from)?;
                    if count > super::super::MAX_PATH || bytes[..count] != *entry.link.as_bytes() {
                        return Err(Error::UnsafePath);
                    }
                }
                Kind::Character => check_character(&before, entry)?,
            }
            cancel.check()?;
            let after = fs::fstat(&node).map_err(std::io::Error::from)?;
            if !stable(&before, &after) {
                return Err(Error::UnsafePath);
            }
            let reachable =
                fs::fstat(self.capture(&entry.path, cancel)?).map_err(std::io::Error::from)?;
            if !stable(&before, &reachable) {
                return Err(Error::UnsafePath);
            }
        }
        if children.checked_add(1) != Some(expected.entries().len()) {
            return Err(Error::Length);
        }
        cancel.check()
    }

    fn capture(&self, path: &str, cancel: &Cancellation) -> Result<OwnedFd, Error> {
        cancel.check()?;
        if path == "." {
            return Ok(self.directory.try_clone()?);
        }
        let (parent, leaf) = self.parent(path, cancel)?;
        let node = fs::openat(
            parent,
            leaf.as_str(),
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        cancel.check()?;
        Ok(node)
    }

    fn verify_children(
        &self,
        entry: &Entry,
        node: &OwnedFd,
        inventory: &BTreeMap<&str, &Entry>,
        cancel: &Cancellation,
    ) -> Result<usize, Error> {
        let directory = if entry.path == "." {
            self.directory.try_clone()?
        } else {
            let (parent, leaf) = self.parent(&entry.path, cancel)?;
            fs::openat(
                parent,
                leaf.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?
        };
        let pinned = fs::fstat(node).map_err(std::io::Error::from)?;
        let opened = fs::fstat(&directory).map_err(std::io::Error::from)?;
        if !stable(&pinned, &opened) {
            return Err(Error::UnsafePath);
        }
        let mut count = 0_usize;
        for child in fs::Dir::read_from(&directory).map_err(std::io::Error::from)? {
            cancel.check()?;
            let child = child.map_err(std::io::Error::from)?;
            let name = child.file_name().to_str().map_err(|_| Error::UnsafePath)?;
            if matches!(name, "." | "..") {
                continue;
            }
            let path = if entry.path == "." {
                name.to_owned()
            } else {
                format!("{}/{name}", entry.path)
            };
            if path.len() > super::super::MAX_PATH || !inventory.contains_key(path.as_str()) {
                return Err(Error::UnsafePath);
            }
            count = count.checked_add(1).ok_or(Error::Length)?;
            if count > inventory.len() {
                return Err(Error::Length);
            }
        }
        cancel.check()?;
        Ok(count)
    }
}

fn link_counts<'a>(
    expected: &'a Inspection,
    cancel: &Cancellation,
) -> Result<BTreeMap<&'a str, u64>, Error> {
    let mut counts = BTreeMap::new();
    for entry in expected.entries() {
        cancel.check()?;
        match entry.kind {
            Kind::File => {
                counts.insert(entry.path.as_str(), 1_u64);
            }
            Kind::Hardlink => {
                let target = super::super::path(&entry.link)?;
                let count = counts.get_mut(target.as_str()).ok_or(Error::UnsafePath)?;
                *count = count.checked_add(1).ok_or(Error::Length)?;
            }
            _ => {}
        }
    }
    Ok(counts)
}

fn verify_file(node: &OwnedFd, entry: &Entry, cancel: &Cancellation) -> Result<(), Error> {
    cancel.check()?;
    let before = fs::fstat(node).map_err(std::io::Error::from)?;
    if before.st_size < 0 || u64::try_from(before.st_size).map_err(|_| Error::Length)? != entry.size
    {
        return Err(Error::Length);
    }
    let descriptors = proc_descriptors(cancel)?;
    let name = node.as_raw_fd().to_string();
    let opened = fs::openat(
        descriptors,
        name.as_str(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    if !stable(&before, &fs::fstat(&opened).map_err(std::io::Error::from)?) {
        return Err(Error::UnsafePath);
    }
    let mut file = File::from(opened);
    let mut hash = Sha256::new();
    let mut bytes = [0; 8192];
    let mut total = 0_u64;
    loop {
        cancel.check()?;
        let count = match file.read(&mut bytes) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        cancel.check()?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or(Error::Length)?;
        if total > entry.size {
            return Err(Error::Length);
        }
        hash.update(&bytes[..count]);
    }
    if total != entry.size {
        return Err(Error::Length);
    }
    if <[u8; 32]>::from(hash.finalize()) != entry.content_sha256 {
        return Err(Error::Hash);
    }
    if !stable(&before, &fs::fstat(file).map_err(std::io::Error::from)?) {
        return Err(Error::UnsafePath);
    }
    cancel.check()
}

fn stable(before: &fs::Stat, after: &fs::Stat) -> bool {
    (
        before.st_dev,
        before.st_ino,
        before.st_mode,
        before.st_uid,
        before.st_gid,
        before.st_rdev,
        before.st_size,
        before.st_nlink,
        before.st_mtime,
        before.st_mtime_nsec,
        before.st_ctime,
        before.st_ctime_nsec,
    ) == (
        after.st_dev,
        after.st_ino,
        after.st_mode,
        after.st_uid,
        after.st_gid,
        after.st_rdev,
        after.st_size,
        after.st_nlink,
        after.st_mtime,
        after.st_mtime_nsec,
        after.st_ctime,
        after.st_ctime_nsec,
    )
}
