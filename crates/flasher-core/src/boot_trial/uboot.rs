//! Fixed original/release U-Boot snapshots. No NAND operation is exposed here.
use crate::{Cancellation, Error};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
};

pub const PROGRAM_BYTES: usize = 4_194_304;
pub const RELEASE_PROGRAM: &str =
    "2c5de011e950263c1e940c0a926863404d3dc4a936ae20823226d5b2b5dbafb8";
pub const BUNDLE: &str = "982e47c825762cb6a88b1c21561ef624bd61dfb1a3a89191f3ccbc7b09e9350b";
const CHUNK: usize = 8192;
const BUNDLE_BYTES: usize = PROGRAM_BYTES * 2;

/// Two exact padded programs, retained as private open snapshots.
///
/// A fixed bundle alternates original/release 8 KiB chunks so the existing RAM-image gzip encoder
/// can compress their shared bytes. It contains no paths, addresses or commands.
#[derive(Debug)]
pub struct Images {
    original: tempfile::NamedTempFile,
    release: tempfile::NamedTempFile,
    _directory: tempfile::TempDir,
}
impl Images {
    /// Validate bundle and both decoded program hashes before filesystem mutation.
    /// # Errors
    /// Rejects unsafe input paths, wrong length/hash, cancellation or I/O failure.
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<Self, Error> {
        cancel.check()?;
        crate::assets::regular_components(path)?;
        let source = File::open(path)?;
        if !source.metadata()?.is_file() || source.metadata()?.len() != BUNDLE_BYTES as u64 {
            return Err(Error::Length);
        }
        let mut bytes = Vec::new();
        source
            .take(BUNDLE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        validate(&bytes, cancel)?;
        let directory = crate::assets::temporary_directory()?;
        let mut original = tempfile::NamedTempFile::new_in(directory.path())?;
        let mut release = tempfile::NamedTempFile::new_in(directory.path())?;
        for pair in bytes.as_chunks::<{ CHUNK * 2 }>().0 {
            cancel.check()?;
            original.write_all(&pair[..CHUNK])?;
            release.write_all(&pair[CHUNK..])?;
        }
        original.as_file().sync_all()?;
        release.as_file().sync_all()?;
        let mut images = Self {
            original,
            release,
            _directory: directory,
        };
        images.revalidate(cancel)?;
        Ok(images)
    }

    /// Recheck the retained descriptors, never a replaced bundle pathname.
    /// # Errors
    /// Rejects changed bytes/length, cancellation or I/O failure.
    pub fn revalidate(&mut self, cancel: &Cancellation) -> Result<(), Error> {
        verify_file(self.original.as_file_mut(), super::ORIGINAL_UBOOT, cancel)?;
        verify_file(self.release.as_file_mut(), RELEASE_PROGRAM, cancel)
    }
}

fn validate(bytes: &[u8], cancel: &Cancellation) -> Result<(), Error> {
    cancel.check()?;
    if bytes.len() != BUNDLE_BYTES || format!("{:x}", Sha256::digest(bytes)) != BUNDLE {
        return Err(Error::Hash);
    }
    let mut original = Sha256::new();
    let mut release = Sha256::new();
    for pair in bytes.as_chunks::<{ CHUNK * 2 }>().0 {
        cancel.check()?;
        original.update(&pair[..CHUNK]);
        release.update(&pair[CHUNK..]);
    }
    if format!("{:x}", original.finalize()) != super::ORIGINAL_UBOOT
        || format!("{:x}", release.finalize()) != RELEASE_PROGRAM
    {
        return Err(Error::Hash);
    }
    cancel.check()
}

fn verify_file(file: &mut File, expected: &str, cancel: &Cancellation) -> Result<(), Error> {
    cancel.check()?;
    if file.metadata()?.len() != PROGRAM_BYTES as u64 {
        return Err(Error::Length);
    }
    file.rewind()?;
    let mut digest = Sha256::new();
    let mut total = 0;
    let mut bytes = [0; CHUNK];
    loop {
        cancel.check()?;
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        total += count;
        if total > PROGRAM_BYTES {
            return Err(Error::Length);
        }
        digest.update(&bytes[..count]);
    }
    if total != PROGRAM_BYTES {
        return Err(Error::Length);
    }
    if format!("{:x}", digest.finalize()) != expected {
        return Err(Error::Hash);
    }
    file.rewind()?;
    cancel.check()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_rejects_wrong_sizes_corruption_symlink_and_cancellation() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("bundle.bin");
        for size in [0, PROGRAM_BYTES, BUNDLE_BYTES, BUNDLE_BYTES + 1] {
            std::fs::write(&path, vec![0; size]).unwrap();
            assert!(Images::open(&path, &Cancellation::default()).is_err());
            assert_eq!(std::fs::metadata(&path).unwrap().len(), size as u64);
        }
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            Images::open(&path, &cancel),
            Err(Error::Cancelled)
        ));
        #[cfg(unix)]
        {
            let link = directory.path().join("link.bin");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(matches!(
                Images::open(&link, &Cancellation::default()),
                Err(Error::UnsafePath)
            ));
        }
    }

    #[test]
    fn retained_program_revalidation_rejects_length_and_digest_changes() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0; PROGRAM_BYTES]).unwrap();
        assert!(matches!(
            verify_file(&mut file, RELEASE_PROGRAM, &Cancellation::default()),
            Err(Error::Hash)
        ));
        file.set_len(PROGRAM_BYTES as u64 + 1).unwrap();
        assert!(matches!(
            verify_file(&mut file, RELEASE_PROGRAM, &Cancellation::default()),
            Err(Error::Length)
        ));
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            verify_file(&mut file, RELEASE_PROGRAM, &cancel),
            Err(Error::Cancelled)
        ));
    }
}
