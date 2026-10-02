//! Linux descriptor-relative creation beneath an empty private installation root.
//! This is a filesystem capability, not a NAND or physical-plan authorization.
use crate::{Cancellation, Error};
use rustix::{
    fd::OwnedFd,
    fs::{self, FileType, Mode, OFlags},
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
}
