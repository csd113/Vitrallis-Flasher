//! Private replay consumer: source completion must precede directory finalization.
use super::Root;
use crate::{
    Cancellation, Error,
    rootfs::{Entry, Inspection, Kind, Sink},
};
use std::{fs::File, io::Write};

pub struct Installer<'a> {
    root: Root,
    expected: &'a Inspection,
    cancel: &'a Cancellation,
    directories: Vec<&'a Entry>,
    index: usize,
    active: bool,
    file: Option<(File, u64)>,
}
impl<'a> Installer<'a> {
    pub(crate) fn new(
        root: Root,
        expected: &'a Inspection,
        cancel: &'a Cancellation,
    ) -> Result<Self, Error> {
        cancel.check()?;
        expected.installation_preflight(cancel)?;
        if rustix::process::geteuid().as_raw() != 0 {
            return Err(Error::UnsafePath);
        }
        let mut directories: Vec<_> = expected
            .entries()
            .iter()
            .filter(|entry| entry.kind == Kind::Directory)
            .collect();
        directories
            .sort_unstable_by_key(|entry| (entry.path.matches('/').count(), entry.path.as_str()));
        // Inspection permits a directory header after its children. Precreate
        // every validated directory by depth, without applying restrictive modes.
        for entry in &directories {
            cancel.check()?;
            if entry.path != "." {
                root.create_directory(&entry.path, cancel)?;
            }
        }
        Ok(Self {
            root,
            expected,
            cancel,
            directories,
            index: 0,
            active: false,
            file: None,
        })
    }

    // Only the verified-asset wrapper calls this after all source checks pass.
    pub(crate) fn complete(self) -> Result<(), Error> {
        self.cancel.check()?;
        if self.active || self.file.is_some() || self.index != self.expected.entries().len() {
            return Err(Error::Length);
        }
        for entry in self.directories.into_iter().rev() {
            self.root.finish_directory(entry, self.cancel)?;
        }
        self.cancel.check()
    }

    fn entry(&self) -> Result<&Entry, Error> {
        self.expected.entries().get(self.index).ok_or(Error::Length)
    }
}

impl Sink for Installer<'_> {
    fn begin(&mut self, entry: &Entry) -> Result<(), Error> {
        self.cancel.check()?;
        if self.active || self.file.is_some() || self.entry()? != entry {
            return Err(Error::Length);
        }
        match entry.kind {
            Kind::Directory => {}
            Kind::File => self.file = Some((self.root.create_file(&entry.path, self.cancel)?, 0)),
            Kind::Symlink => self.root.create_symlink(entry, self.cancel)?,
            Kind::Hardlink => self.root.create_hardlink(entry, self.cancel)?,
            Kind::Character => self.root.create_character(entry, self.cancel)?,
        }
        self.cancel.check()?;
        self.active = true;
        Ok(())
    }

    fn data(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Error> {
        self.cancel.check()?;
        let entry = self.entry()?;
        let size = entry.size;
        if !self.active || entry.kind != Kind::File || bytes.is_empty() || bytes.len() > 8192 {
            return Err(Error::Length);
        }
        let (file, written) = self.file.as_mut().ok_or(Error::Length)?;
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Length)?;
        if offset != *written || end > size {
            return Err(Error::Length);
        }
        file.write_all(bytes)?;
        self.cancel.check()?;
        *written = end;
        Ok(())
    }

    fn finish(&mut self, entry: &Entry) -> Result<(), Error> {
        self.cancel.check()?;
        if !self.active || self.entry()? != entry {
            return Err(Error::Length);
        }
        match entry.kind {
            Kind::File => {
                let (file, written) = self.file.take().ok_or(Error::Length)?;
                if written != entry.size || file.metadata()?.len() != entry.size {
                    return Err(Error::Length);
                }
                Root::finish_file(&file, entry, self.cancel)?;
            }
            Kind::Symlink => self.root.finish_symlink(entry, self.cancel)?,
            Kind::Character => self.root.finish_character(entry, self.cancel)?,
            Kind::Directory | Kind::Hardlink => {}
        }
        self.cancel.check()?;
        self.active = false;
        self.index += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rootfs::{
        inspect, replay_gzip,
        tests::{archive, checksum, compressed, member},
    };
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn fixture() -> (Vec<u8>, Inspection) {
        let mut file = member("./late/file", b'0', &vec![37; 20_000], "");
        file[100..108].copy_from_slice(b"0006750\0");
        file[108..116].copy_from_slice(b"0001750\0");
        file[116..124].copy_from_slice(b"0001750\0");
        repair(&mut file);
        let mut hard = member("./hard", b'1', b"", "./late/file");
        hard[100..124].copy_from_slice(&file[100..124]);
        repair(&mut hard);
        let mut link = member("./link", b'2', b"", "/late/file");
        link[100..108].copy_from_slice(b"0000777\0");
        repair(&mut link);
        let mut directory = member("./late", b'5', b"", "");
        directory[100..108].copy_from_slice(b"0000500\0");
        repair(&mut directory);
        let bytes = archive(&[member("./", b'5', b"", ""), file, hard, link, directory]);
        let inspection = inspect(bytes.as_slice(), &Cancellation::default()).unwrap();
        (bytes, inspection)
    }
    fn repair(member: &mut [u8]) {
        let mut header: [u8; 512] = member[..512].try_into().unwrap();
        checksum(&mut header);
        member[..512].copy_from_slice(&header);
    }
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
    fn installation_precreates_late_directories_preserves_links_and_finalizes_metadata() {
        let (bytes, expected) = fixture();
        let (root, directory) = root();
        let cancel = Cancellation::default();
        let installer = Installer::new(root, &expected, &cancel);
        if rustix::process::geteuid().as_raw() != 0 {
            assert!(installer.is_err());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
            return;
        }
        let mut installer = installer.unwrap();
        assert_eq!(
            std::fs::metadata(directory.path().join("late"))
                .unwrap()
                .mode()
                & 0o7777,
            0o700
        );
        replay_gzip(
            compressed(&bytes).as_slice(),
            &expected,
            &mut installer,
            &cancel,
        )
        .unwrap();
        // Even a finished replay does not finalize directories before source recheck.
        assert_eq!(
            std::fs::metadata(directory.path()).unwrap().mode() & 0o7777,
            0o700
        );
        installer.complete().unwrap();
        let file = std::fs::metadata(directory.path().join("late/file")).unwrap();
        let hard = std::fs::metadata(directory.path().join("hard")).unwrap();
        assert_eq!(
            (file.uid(), file.gid(), file.mode() & 0o7777),
            (1000, 1000, 0o6750)
        );
        assert_eq!((file.dev(), file.ino()), (hard.dev(), hard.ino()));
        assert_eq!(
            std::fs::read(directory.path().join("late/file")).unwrap(),
            vec![37; 20_000]
        );
        assert_eq!(
            std::fs::read_link(directory.path().join("link")).unwrap(),
            std::path::Path::new("/late/file")
        );
        assert_eq!(
            std::fs::metadata(directory.path().join("late"))
                .unwrap()
                .mode()
                & 0o7777,
            0o500
        );
        assert_eq!(
            std::fs::metadata(directory.path()).unwrap().mode() & 0o7777,
            0o755
        );
    }
    #[test]
    fn unsupported_inventory_fails_before_directory_creation() {
        let (_, mut expected) = fixture();
        expected.entries[2].gid = 1001;
        let (root, directory) = root();
        assert!(Installer::new(root, &expected, &Cancellation::default()).is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        assert_eq!(
            std::fs::metadata(directory.path()).unwrap().mode() & 0o7777,
            0o700
        );
    }
    #[test]
    fn late_crc_failure_preserves_partial_tree_without_directory_completion() {
        if rustix::process::geteuid().as_raw() != 0 {
            return;
        }
        let (bytes, expected) = fixture();
        let mut gzip = compressed(&bytes);
        let crc = gzip.len() - 8;
        gzip[crc] ^= 1;
        let (root, directory) = root();
        let cancel = Cancellation::default();
        let mut installer = Installer::new(root, &expected, &cancel).unwrap();
        assert!(replay_gzip(gzip.as_slice(), &expected, &mut installer, &cancel).is_err());
        drop(installer);
        assert_eq!(
            std::fs::metadata(directory.path()).unwrap().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(directory.path().join("late"))
                .unwrap()
                .mode()
                & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::read(directory.path().join("late/file")).unwrap(),
            vec![37; 20_000]
        );
    }
    #[test]
    fn invalid_offsets_and_cancellation_never_complete_or_retry() {
        if rustix::process::geteuid().as_raw() != 0 {
            return;
        }
        let (_, expected) = fixture();
        let (root, directory) = root();
        let cancel = Cancellation::default();
        let mut installer = Installer::new(root, &expected, &cancel).unwrap();
        installer.begin(&expected.entries()[0]).unwrap();
        installer.finish(&expected.entries()[0]).unwrap();
        installer.begin(&expected.entries()[1]).unwrap();
        assert!(installer.data(1, b"X").is_err());
        assert!(installer.data(0, &[0; 8193]).is_err());
        assert_eq!(
            std::fs::metadata(directory.path().join("late/file"))
                .unwrap()
                .len(),
            0
        );
        installer.data(0, b"X").unwrap();
        cancel.cancel();
        assert!(matches!(installer.data(1, b"Y"), Err(Error::Cancelled)));
        assert!(matches!(installer.complete(), Err(Error::Cancelled)));
        assert_eq!(
            std::fs::read(directory.path().join("late/file")).unwrap(),
            b"X"
        );
        assert_eq!(
            std::fs::metadata(directory.path()).unwrap().mode() & 0o7777,
            0o700
        );
    }
}
