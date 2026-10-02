//! Linux descriptor-relative creation beneath an empty private installation root.
//! This is a filesystem capability, not a NAND or physical-plan authorization.
mod installer;
pub(crate) use installer::Installer;

use crate::{Cancellation, Error};
use rustix::{
    fd::{AsFd, AsRawFd, OwnedFd},
    fs::{self, AtFlags, FileType, Mode, OFlags},
    process::{Gid, Uid},
};
use std::fs::File;

/// Owns an empty directory descriptor. Every parent is reopened with
/// `O_DIRECTORY | O_NOFOLLOW`; leaves are created exclusively without replacement.
#[derive(Debug)]
pub struct Root {
    directory: OwnedFd,
}
impl Root {
    /// Accept an already-open empty directory owned by this process with mode 0700.
    /// No pathname is reopened and no filesystem mutation is performed.
    /// # Errors
    /// Rejects non-directories, non-private/unowned/nonempty roots or cancellation.
    pub fn new(directory: File, cancel: &Cancellation) -> Result<Self, Error> {
        cancel.check()?;
        let stat = fs::fstat(&directory).map_err(std::io::Error::from)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
            || stat.st_mode & 0o7777 != 0o700
            || stat.st_uid != rustix::process::geteuid().as_raw()
        {
            return Err(Error::UnsafePath);
        }
        let entries = fs::Dir::read_from(&directory).map_err(std::io::Error::from)?;
        for entry in entries {
            cancel.check()?;
            let entry = entry.map_err(std::io::Error::from)?;
            if !matches!(entry.file_name().to_bytes(), b"." | b"..") {
                return Err(Error::UnsafePath);
            }
        }
        cancel.check()?;
        Ok(Self {
            directory: directory.into(),
        })
    }

    /// Create one archive-relative directory with initial mode 0700.
    /// Existing parents must be directories; no existing leaf is replaced.
    /// # Errors
    /// Rejects unsafe paths, symlink parents, existing leaves and cancellation.
    pub fn create_directory(&self, relative: &str, cancel: &Cancellation) -> Result<(), Error> {
        let (parent, leaf) = self.parent(relative, cancel)?;
        cancel.check()?;
        fs::mkdirat(parent, leaf.as_str(), Mode::from_raw_mode(0o700))
            .map_err(std::io::Error::from)?;
        cancel.check()
    }

    /// Create a regular file exclusively with initial mode 0600.
    /// File contents and final metadata/completion remain the caller's responsibility.
    /// # Errors
    /// Rejects unsafe paths, symlink parents, existing leaves and cancellation.
    pub fn create_file(&self, relative: &str, cancel: &Cancellation) -> Result<File, Error> {
        let (parent, leaf) = self.parent(relative, cancel)?;
        cancel.check()?;
        let file = fs::openat(
            parent,
            leaf.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?;
        cancel.check()?;
        Ok(file.into())
    }

    /// Store an inspected symlink without resolving its target.
    /// # Errors
    /// Rejects unsafe targets/paths, other kinds, existing leaves or cancellation.
    pub fn create_symlink(&self, entry: &super::Entry, cancel: &Cancellation) -> Result<(), Error> {
        validate_owner(entry)?;
        if entry.kind != super::Kind::Symlink
            || entry.mode != 0o777
            || super::path(&entry.path)? != entry.path
        {
            return Err(Error::UnsafePath);
        }
        super::validate_link(entry, &std::collections::BTreeMap::new())?;
        let (parent, leaf) = self.parent(&entry.path, cancel)?;
        cancel.check()?;
        fs::symlinkat(entry.link.as_str(), parent, leaf.as_str()).map_err(std::io::Error::from)?;
        cancel.check()
    }

    /// Link an earlier regular file, preserving its inode and metadata.
    /// # Errors
    /// Rejects unsafe/symlink targets, metadata mismatch, replacement or cancellation.
    pub fn create_hardlink(
        &self,
        entry: &super::Entry,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        if entry.kind != super::Kind::Hardlink {
            return Err(Error::UnsafePath);
        }
        let (source_parent, source_leaf) = self.parent(&entry.link, cancel)?;
        let source = fs::openat(
            &source_parent,
            source_leaf.as_str(),
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let before = fs::fstat(&source).map_err(std::io::Error::from)?;
        validate_metadata(&before, entry, FileType::RegularFile)?;
        let (parent, leaf) = self.parent(&entry.path, cancel)?;
        cancel.check()?;
        fs::linkat(
            source_parent,
            source_leaf.as_str(),
            &parent,
            leaf.as_str(),
            AtFlags::empty(),
        )
        .map_err(std::io::Error::from)?;
        let after = fs::statat(parent, leaf.as_str(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(std::io::Error::from)?;
        if after.st_dev != before.st_dev || after.st_ino != before.st_ino {
            return Err(Error::UnsafePath);
        }
        validate_metadata(&after, entry, FileType::RegularFile)?;
        cancel.check()
    }

    /// Apply numeric ownership and mode to a held regular-file descriptor.
    /// Ownership precedes chmod so setuid/setgid bits are restored after chown.
    /// # Errors
    /// Rejects invalid metadata/kinds, syscall failures or cancellation.
    pub fn finish_file(
        file: &File,
        entry: &super::Entry,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        if entry.kind != super::Kind::File {
            return Err(Error::UnsafePath);
        }
        set_metadata(file, entry, FileType::RegularFile, cancel)?;
        cancel.check()?;
        fs::fsync(file).map_err(std::io::Error::from)?;
        cancel.check()
    }

    /// Apply directory metadata after its descendants have been installed.
    /// # Errors
    /// Rejects other kinds, symlink paths, invalid metadata or cancellation.
    pub fn finish_directory(
        &self,
        entry: &super::Entry,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        if entry.kind != super::Kind::Directory {
            return Err(Error::UnsafePath);
        }
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
        set_metadata(&directory, entry, FileType::Directory, cancel)?;
        cancel.check()?;
        fs::fsync(directory).map_err(std::io::Error::from)?;
        cancel.check()
    }

    /// Apply symlink ownership to its captured inode without following the link.
    /// # Errors
    /// Rejects other kinds, unsupported symlink mode, invalid owner IDs or cancellation.
    pub fn finish_symlink(&self, entry: &super::Entry, cancel: &Cancellation) -> Result<(), Error> {
        if entry.kind != super::Kind::Symlink || entry.mode != 0o777 {
            return Err(Error::UnsafePath);
        }
        validate_owner(entry)?;
        let (parent, leaf) = self.parent(&entry.path, cancel)?;
        let link = fs::openat(
            parent,
            leaf.as_str(),
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let stat = fs::fstat(&link).map_err(std::io::Error::from)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Symlink {
            return Err(Error::UnsafePath);
        }
        cancel.check()?;
        fs::chownat(
            &link,
            "",
            Some(Uid::from_raw(entry.uid)),
            Some(Gid::from_raw(entry.gid)),
            AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(std::io::Error::from)?;
        validate_metadata(
            &fs::fstat(&link).map_err(std::io::Error::from)?,
            entry,
            FileType::Symlink,
        )?;
        cancel.check()
    }

    /// Create only a reviewed standard character node, initially mode 0600.
    /// This never opens the device for I/O.
    /// # Errors
    /// Rejects changed identities, unsafe paths, existing leaves or cancellation.
    pub fn create_character(
        &self,
        entry: &super::Entry,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        validate_character(entry)?;
        if rustix::process::geteuid().as_raw() != 0 {
            return Err(Error::UnsafePath);
        }
        let (parent, leaf) = self.parent(&entry.path, cancel)?;
        cancel.check()?;
        fs::mknodat(
            parent,
            leaf.as_str(),
            FileType::CharacterDevice,
            Mode::from_raw_mode(0o600),
            fs::makedev(entry.major, entry.minor),
        )
        .map_err(std::io::Error::from)?;
        cancel.check()
    }

    /// Apply character-node metadata through a captured inode, without device I/O.
    /// # Errors
    /// Rejects changed nodes, unavailable/untrusted procfs, syscall errors or cancellation.
    pub fn finish_character(
        &self,
        entry: &super::Entry,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        validate_character(entry)?;
        let (parent, leaf) = self.parent(&entry.path, cancel)?;
        let node = fs::openat(
            parent,
            leaf.as_str(),
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let stat = fs::fstat(&node).map_err(std::io::Error::from)?;
        check_character(&stat, entry)?;
        cancel.check()?;
        fs::chownat(
            &node,
            "",
            Some(Uid::from_raw(entry.uid)),
            Some(Gid::from_raw(entry.gid)),
            AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(std::io::Error::from)?;
        chmod_captured(&node, entry.mode, cancel)?;
        let after = fs::fstat(node).map_err(std::io::Error::from)?;
        check_character(&after, entry)?;
        validate_metadata(&after, entry, FileType::CharacterDevice)?;
        cancel.check()
    }

    fn parent(&self, relative: &str, cancel: &Cancellation) -> Result<(OwnedFd, String), Error> {
        cancel.check()?;
        let normalized = super::path(relative)?;
        if normalized == "." {
            return Err(Error::UnsafePath);
        }
        let (parents, leaf) = normalized.rsplit_once('/').unwrap_or(("", &normalized));
        let mut directory = self.directory.try_clone()?;
        if !parents.is_empty() {
            for component in parents.split('/') {
                cancel.check()?;
                directory = fs::openat(
                    directory,
                    component,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(std::io::Error::from)?;
                cancel.check()?;
            }
        }
        Ok((directory, leaf.to_owned()))
    }
}

fn validate_character(entry: &super::Entry) -> Result<(), Error> {
    if entry.kind != super::Kind::Character || super::path(&entry.path)? != entry.path {
        return Err(Error::UnsafePath);
    }
    validate_owner(entry)?;
    if entry.mode > 0o777 {
        return Err(Error::UnsafePath);
    }
    super::validate_device(entry)
}

fn check_character(stat: &fs::Stat, entry: &super::Entry) -> Result<(), Error> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::CharacterDevice
        || fs::major(stat.st_rdev) != entry.major
        || fs::minor(stat.st_rdev) != entry.minor
    {
        return Err(Error::UnsafePath);
    }
    Ok(())
}

fn chmod_captured(node: &OwnedFd, mode: u32, cancel: &Cancellation) -> Result<(), Error> {
    cancel.check()?;
    // Only this kernel-controlled namespace is followed, never an archive link.
    // The locked rustix API cannot apply chmod directly to an O_PATH descriptor.
    let proc = fs::open(
        "/proc",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    if fs::fstatfs(&proc).map_err(std::io::Error::from)?.f_type != fs::PROC_SUPER_MAGIC {
        return Err(Error::UnsafePath);
    }
    let descriptors = fs::openat(
        proc,
        "self/fd",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    if fs::fstatfs(&descriptors)
        .map_err(std::io::Error::from)?
        .f_type
        != fs::PROC_SUPER_MAGIC
    {
        return Err(Error::UnsafePath);
    }
    let name = node.as_raw_fd().to_string();
    let pinned = fs::fstat(node).map_err(std::io::Error::from)?;
    for applying in [false, true] {
        cancel.check()?;
        if applying {
            fs::chmodat(
                &descriptors,
                name.as_str(),
                Mode::from_raw_mode(mode),
                AtFlags::empty(),
            )
            .map_err(std::io::Error::from)?;
        }
        let target = fs::statat(&descriptors, name.as_str(), AtFlags::empty())
            .map_err(std::io::Error::from)?;
        if target.st_dev != pinned.st_dev || target.st_ino != pinned.st_ino {
            return Err(Error::UnsafePath);
        }
    }
    cancel.check()
}

const fn validate_owner(entry: &super::Entry) -> Result<(), Error> {
    if entry.uid == u32::MAX || entry.gid == u32::MAX || entry.mode > 0o7777 {
        return Err(Error::UnsafePath);
    }
    Ok(())
}

fn validate_metadata(stat: &fs::Stat, entry: &super::Entry, kind: FileType) -> Result<(), Error> {
    validate_owner(entry)?;
    if FileType::from_raw_mode(stat.st_mode) != kind
        || stat.st_uid != entry.uid
        || stat.st_gid != entry.gid
        || stat.st_mode & 0o7777 != entry.mode
    {
        return Err(Error::UnsafePath);
    }
    Ok(())
}

fn set_metadata(
    fd: &impl AsFd,
    entry: &super::Entry,
    kind: FileType,
    cancel: &Cancellation,
) -> Result<(), Error> {
    cancel.check()?;
    validate_owner(entry)?;
    let stat = fs::fstat(fd).map_err(std::io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != kind {
        return Err(Error::UnsafePath);
    }
    fs::fchown(
        fd,
        Some(Uid::from_raw(entry.uid)),
        Some(Gid::from_raw(entry.gid)),
    )
    .map_err(std::io::Error::from)?;
    cancel.check()?;
    fs::fchmod(fd, Mode::from_raw_mode(entry.mode)).map_err(std::io::Error::from)?;
    validate_metadata(&fs::fstat(fd).map_err(std::io::Error::from)?, entry, kind)?;
    cancel.check()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::unix::fs::{PermissionsExt, symlink},
    };

    fn root() -> (Root, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = Root::new(
            File::open(directory.path()).unwrap(),
            &Cancellation::default(),
        )
        .unwrap();
        (root, directory)
    }

    #[test]
    fn nested_creation_remains_beneath_the_open_root_and_never_replaces_leaves() {
        let (root, directory) = root();
        let cancel = Cancellation::default();
        root.create_directory("./etc", &cancel).unwrap();
        let mut file = root.create_file("etc/config", &cancel).unwrap();
        file.write_all(b"verified bytes").unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("etc/config")).unwrap(),
            b"verified bytes"
        );
        assert!(root.create_file("etc/config", &cancel).is_err());
        assert!(root.create_directory("etc", &cancel).is_err());
        assert_eq!(
            std::fs::metadata(directory.path().join("etc"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn unsafe_paths_and_symlink_parents_or_leaves_cannot_escape_or_overwrite() {
        let (root, directory) = root();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("kept"), b"outside").unwrap();
        symlink(outside.path(), directory.path().join("link")).unwrap();
        symlink(outside.path().join("kept"), directory.path().join("leaf")).unwrap();
        let cancel = Cancellation::default();
        for path in [
            "../escaped",
            "/absolute",
            "a/../escaped",
            "a//b",
            ".",
            "link/escaped",
            "leaf",
            "a\\b",
        ] {
            assert!(root.create_file(path, &cancel).is_err());
            assert!(root.create_directory(path, &cancel).is_err());
        }
        assert_eq!(
            std::fs::read(outside.path().join("kept")).unwrap(),
            b"outside"
        );
        assert!(!outside.path().join("escaped").exists());
    }

    #[test]
    fn root_requires_empty_private_owned_directory_and_cancellation_precedes_mutation() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            Root::new(
                File::open(directory.path()).unwrap(),
                &Cancellation::default()
            )
            .is_err()
        );
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(directory.path().join("existing"), b"kept").unwrap();
        assert!(
            Root::new(
                File::open(directory.path()).unwrap(),
                &Cancellation::default()
            )
            .is_err()
        );
        assert!(
            Root::new(
                File::open(directory.path().join("existing")).unwrap(),
                &Cancellation::default()
            )
            .is_err()
        );
        let (root, empty) = root();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            root.create_file("absent", &cancel),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            root.create_directory("absent", &cancel),
            Err(Error::Cancelled)
        ));
        assert_eq!(std::fs::read_dir(empty.path()).unwrap().count(), 0);
    }
    fn entry(path: &str, kind: super::super::Kind, mode: u32, link: &str) -> super::super::Entry {
        super::super::Entry {
            path: path.into(),
            kind,
            mode,
            uid: rustix::process::geteuid().as_raw(),
            gid: rustix::process::getegid().as_raw(),
            link: link.into(),
            major: 0,
            minor: 0,
            size: 0,
            content_sha256: [0; 32],
        }
    }

    #[test]
    fn ownership_mode_and_links_preserve_inodes_without_following_targets() {
        use super::super::Kind;
        use std::os::unix::fs::MetadataExt;
        let (root, directory) = root();
        let cancel = Cancellation::default();
        root.create_directory("etc", &cancel).unwrap();
        let mut file = root.create_file("etc/source", &cancel).unwrap();
        file.write_all(b"contents").unwrap();
        let mut regular = entry("etc/source", Kind::File, 0o6750, "");
        if rustix::process::geteuid().as_raw() == 0 {
            regular.uid = 1000;
            regular.gid = 1000;
        }
        Root::finish_file(&file, &regular, &cancel).unwrap();
        let mut hardlink = entry("etc/hard", Kind::Hardlink, 0o6750, "./etc/source");
        hardlink.uid = regular.uid;
        hardlink.gid = regular.gid;
        root.create_hardlink(&hardlink, &cancel).unwrap();
        let source = std::fs::metadata(directory.path().join("etc/source")).unwrap();
        let hard = std::fs::metadata(directory.path().join("etc/hard")).unwrap();
        assert_eq!(source.ino(), hard.ino());
        assert_eq!((source.uid(), source.gid()), (regular.uid, regular.gid));
        assert_eq!(source.permissions().mode() & 0o7777, 0o6750);
        let mut link = entry("etc/link", Kind::Symlink, 0o777, "source");
        link.uid = regular.uid;
        link.gid = regular.gid;
        root.create_symlink(&link, &cancel).unwrap();
        root.finish_symlink(&link, &cancel).unwrap();
        let symbolic = std::fs::symlink_metadata(directory.path().join("etc/link")).unwrap();
        assert_eq!((symbolic.uid(), symbolic.gid()), (link.uid, link.gid));
        assert_eq!(
            std::fs::read_link(directory.path().join("etc/link")).unwrap(),
            std::path::Path::new("source")
        );
        root.finish_directory(&entry("etc", Kind::Directory, 0o755, ""), &cancel)
            .unwrap();
        assert_eq!(
            std::fs::metadata(directory.path().join("etc"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o755
        );
    }

    #[test]
    fn invalid_link_targets_and_owner_sentinels_fail_before_metadata_changes() {
        use super::super::Kind;
        let (root, directory) = root();
        let outside = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        let file = root.create_file("source", &cancel).unwrap();
        let mut invalid = entry("source", Kind::File, 0o7777, "");
        invalid.uid = u32::MAX;
        assert!(Root::finish_file(&file, &invalid, &cancel).is_err());
        assert_eq!(
            file.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );
        assert!(
            root.create_symlink(
                &entry("escape", Kind::Symlink, 0o777, "../outside"),
                &cancel
            )
            .is_err()
        );
        assert!(!directory.path().join("escape").exists());
        symlink(outside.path(), directory.path().join("foreign")).unwrap();
        assert!(
            root.create_hardlink(&entry("hard", Kind::Hardlink, 0o600, "foreign"), &cancel)
                .is_err()
        );
        assert!(!directory.path().join("hard").exists());
        let link = entry("link", Kind::Symlink, 0o777, "source");
        root.create_symlink(&link, &cancel).unwrap();
        let mut unsupported = link;
        unsupported.mode = 0o600;
        assert!(root.finish_symlink(&unsupported, &cancel).is_err());
        assert_eq!(
            file.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );
    }
    #[test]
    fn reviewed_character_node_metadata_uses_captured_inode_without_opening_device() {
        use super::super::Kind;
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let (root, directory) = root();
        let cancel = Cancellation::default();
        root.create_directory("dev", &cancel).unwrap();
        let mut node = entry("dev/null", Kind::Character, 0o666, "");
        node.uid = 0;
        node.gid = 0;
        node.major = 1;
        node.minor = 3;
        if rustix::process::geteuid().as_raw() != 0 {
            assert!(root.create_character(&node, &cancel).is_err());
            return;
        }
        root.create_character(&node, &cancel).unwrap();
        root.finish_character(&node, &cancel).unwrap();
        let stat = std::fs::symlink_metadata(directory.path().join("dev/null")).unwrap();
        assert!(stat.file_type().is_char_device());
        assert_eq!((fs::major(stat.rdev()), fs::minor(stat.rdev())), (1, 3));
        assert_eq!(stat.permissions().mode() & 0o7777, 0o666);
        assert_eq!((stat.uid(), stat.gid()), (0, 0));
        assert!(root.create_character(&node, &cancel).is_err());
    }

    #[test]
    fn changed_character_identity_or_symlink_node_rejected_without_target_mutation() {
        use super::super::Kind;
        let (root, directory) = root();
        let cancel = Cancellation::default();
        root.create_directory("dev", &cancel).unwrap();
        let mut node = entry("dev/null", Kind::Character, 0o666, "");
        node.uid = 0;
        node.gid = 0;
        node.major = 1;
        node.minor = 5;
        assert!(root.create_character(&node, &cancel).is_err());
        assert!(!directory.path().join("dev/null").exists());
        node.minor = 3;
        let file = root.create_file("kept", &cancel).unwrap();
        symlink("../kept", directory.path().join("dev/null")).unwrap();
        assert!(root.finish_character(&node, &cancel).is_err());
        assert_eq!(
            file.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );
    }
}
