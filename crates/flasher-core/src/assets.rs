//! Bounded HTTPS/offline ingestion and a content-addressed atomic cache.
use crate::{
    Cancellation, Error,
    manifest::{Asset, Manifest, Role, https},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub struct Cache {
    root: PathBuf,
}
/// A private verified snapshot, independent of subsequent cache path replacement.
#[derive(Debug)]
pub struct VerifiedAsset {
    spec: Asset,
    snapshot: NamedTempFile,
}
impl VerifiedAsset {
    #[must_use]
    pub const fn role(&self) -> Role {
        self.spec.role()
    }
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.spec.size()
    }
    /// Rechecks the exact open file, not a potentially replaced pathname.
    /// # Errors
    /// Returns corruption, cancellation or I/O errors.
    pub fn recheck(&mut self, cancel: &Cancellation) -> Result<(), Error> {
        self.snapshot.as_file_mut().rewind()?;
        verify(self.snapshot.as_file_mut(), &self.spec, cancel, |_, _| {})?;
        self.snapshot.as_file_mut().rewind()?;
        Ok(())
    }
    pub(crate) fn matches(&self, asset: &Asset) -> bool {
        self.spec.role() == asset.role()
            && self.spec.size() == asset.size()
            && self.spec.sha256() == asset.sha256()
    }
    pub(crate) fn stream(
        &mut self,
        cancel: &Cancellation,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), Error> {
        self.snapshot.as_file_mut().rewind()?;
        verify(
            self.snapshot.as_file_mut(),
            &self.spec,
            cancel,
            &mut progress,
        )
    }
}
impl Cache {
    /// Opens an existing private directory. The caller creates it explicitly.
    /// # Errors
    /// Rejects symlink components, non-directories and group/world-writable Unix directories.
    pub fn open(root: &Path) -> Result<Self, Error> {
        regular_components(root)?;
        let metadata = fs::metadata(root)?;
        if !metadata.is_dir() {
            return Err(Error::UnsafePath);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o022 != 0 {
                return Err(Error::UnsafePath);
            }
        }
        Ok(Self {
            root: fs::canonicalize(root)?,
        })
    }
    /// Validates cached content on every use, then takes a private verified snapshot.
    /// # Errors
    /// Returns corruption or unsafe-path errors rather than trusting a cache hit.
    pub fn lookup(
        &self,
        asset: &Asset,
        cancel: &Cancellation,
    ) -> Result<Option<VerifiedAsset>, Error> {
        cancel.check()?;
        let path = self.root.join(asset.sha256());
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
            Ok(m) if !m.is_file() || m.file_type().is_symlink() => Err(Error::UnsafePath),
            Ok(_) => self.snapshot(asset, File::open(path)?, cancel).map(Some),
        }
    }
    /// Imports bytes transactionally. Nothing is published until size/hash verification succeeds.
    /// # Errors
    /// Returns cancellation, truncation, overflow, hash, race, or I/O errors.
    pub fn import(
        &self,
        asset: &Asset,
        reader: impl Read,
        cancel: &Cancellation,
        progress: impl FnMut(u64, u64),
    ) -> Result<VerifiedAsset, Error> {
        cancel.check()?;
        let mut temporary = NamedTempFile::new_in(&self.root)?;
        copy_verified(reader, temporary.as_file_mut(), asset, cancel, progress)?;
        temporary.as_file().sync_all()?;
        cancel.check()?;
        let destination = self.root.join(asset.sha256());
        match temporary.persist_noclobber(&destination) {
            Ok(_) => {}
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.error.into()),
        }
        self.lookup(asset, cancel)?.ok_or(Error::UnsafePath)
    }
    /// Imports only a hash-named regular file; no archive extraction is done on the host.
    /// # Errors
    /// Rejects unsafe paths and unverified bytes.
    pub fn offline(
        &self,
        asset: &Asset,
        directory: &Path,
        cancel: &Cancellation,
    ) -> Result<VerifiedAsset, Error> {
        let path = directory.join(asset.sha256());
        regular_components(&path)?;
        if !fs::symlink_metadata(&path)?.is_file() {
            return Err(Error::UnsafePath);
        }
        self.import(asset, File::open(path)?, cancel, |_, _| {})
    }
    /// Downloads with TLS verification, five explicit HTTPS-only redirects and bounded time/bytes.
    /// # Errors
    /// Rejects HTTP/status/encoding/length errors, unsafe redirects and invalid checksums.
    pub fn download(
        &self,
        asset: &Asset,
        cancel: &Cancellation,
        progress: impl FnMut(u64, u64),
    ) -> Result<VerifiedAsset, Error> {
        cancel.check()?;
        let agent = crate::transport::agent(cancel);
        let mut url = https(asset.url())?;
        for _ in 0..=5 {
            cancel.check()?;
            let mut response = agent
                .get(url.as_str())
                .header("Accept-Encoding", "identity")
                .call()
                .map_err(|error| match cancel.check() {
                    Err(cancelled) => cancelled,
                    Ok(()) => {
                        if matches!(error, ureq::Error::Timeout(_)) {
                            Error::Timeout
                        } else {
                            Error::Network
                        }
                    }
                })?;
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or(Error::Network)?;
                let next = url.join(location).map_err(|_| Error::Network)?;
                url = https(next.as_str())?;
                continue;
            }
            if response.status().as_u16() != 200
                || response.headers().contains_key("content-encoding")
            {
                return Err(Error::Network);
            }
            if let Some(length) = response.headers().get("content-length") {
                let length = length
                    .to_str()
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or(Error::Length)?;
                if length != asset.size() {
                    return Err(Error::Length);
                }
            }
            return self.import(asset, response.body_mut().as_reader(), cancel, progress);
        }
        Err(Error::Network)
    }
    fn snapshot(
        &self,
        asset: &Asset,
        reader: impl Read,
        cancel: &Cancellation,
    ) -> Result<VerifiedAsset, Error> {
        let mut snapshot = NamedTempFile::new_in(&self.root)?;
        copy_verified(reader, snapshot.as_file_mut(), asset, cancel, |_, _| {})?;
        snapshot.as_file_mut().seek(SeekFrom::Start(0))?;
        Ok(VerifiedAsset {
            spec: asset.clone(),
            snapshot,
        })
    }
}
/// Resolves an entire inventory from offline files or the cache/network.
/// # Errors
/// Stops at the first unverified or missing asset; no device action is performed.
pub fn acquire(
    manifest: &Manifest,
    cache: &Cache,
    offline: Option<&Path>,
    cancel: &Cancellation,
    mut progress: impl FnMut(Role, u64, u64),
) -> Result<Vec<VerifiedAsset>, Error> {
    manifest
        .assets()
        .iter()
        .map(|a| {
            if let Some(hit) = cache.lookup(a, cancel)? {
                progress(a.role(), a.size(), a.size());
                return Ok(hit);
            }
            if let Some(path) = offline {
                let result = cache.offline(a, path, cancel)?;
                progress(a.role(), a.size(), a.size());
                Ok(result)
            } else {
                cache.download(a, cancel, |n, total| progress(a.role(), n, total))
            }
        })
        .collect()
}
fn verify(
    reader: impl Read,
    asset: &Asset,
    cancel: &Cancellation,
    progress: impl FnMut(u64, u64),
) -> Result<(), Error> {
    copy_verified(reader, &mut std::io::sink(), asset, cancel, progress)
}
fn copy_verified(
    mut reader: impl Read,
    writer: &mut impl Write,
    asset: &Asset,
    cancel: &Cancellation,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), Error> {
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut bytes = [0_u8; 16 * 1024];
    loop {
        cancel.check()?;
        // Read at most one byte past the declared length to detect overflow.
        let limit = usize::try_from((asset.size() - total + 1).min(bytes.len() as u64))
            .map_err(|_| Error::Length)?;
        let count = match reader.read(&mut bytes[..limit]) {
            Ok(count) => count,
            Err(error) => {
                cancel.check()?;
                return Err(error.into());
            }
        };
        cancel.check()?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > asset.size() {
            return Err(Error::Length);
        }
        digest.update(&bytes[..count]);
        writer.write_all(&bytes[..count])?;
        progress(total, asset.size());
    }
    if total != asset.size() {
        return Err(Error::Length);
    }
    if format!("{:x}", digest.finalize()) != asset.sha256() {
        return Err(Error::Hash);
    }
    cancel.check()
}
pub(crate) fn regular_components(path: &Path) -> Result<(), Error> {
    let mut partial = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir) {
            return Err(Error::UnsafePath);
        }
        partial.push(part);
        if fs::symlink_metadata(&partial)?.file_type().is_symlink() {
            return Err(Error::UnsafePath);
        }
    }
    Ok(())
}

/// Creates a private temporary directory beneath the resolved OS temporary root.
/// # Errors
/// Returns errors resolving or creating the directory. Resolving handles macOS's `/var` alias.
pub fn temporary_directory() -> Result<tempfile::TempDir, Error> {
    Ok(tempfile::tempdir_in(fs::canonicalize(
        std::env::temp_dir(),
    )?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation;
    fn spec() -> Result<Asset, Error> {
        Manifest::read(simulation::MANIFEST.as_bytes())?
            .assets()
            .first()
            .cloned()
            .ok_or(Error::State)
    }
    #[test]
    fn import_rechecks_cache() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let a = spec()?;
        let cancel = Cancellation::default();
        cache.import(&a, simulation::PAYLOAD, &cancel, |_, _| {})?;
        assert!(cache.lookup(&a, &cancel)?.is_some());
        fs::write(dir.path().join(a.sha256()), b"corrupt")?;
        assert!(matches!(cache.lookup(&a, &cancel), Err(Error::Length)));
        Ok(())
    }
    #[test]
    fn invalid_downloads_publish_nothing() -> Result<(), Error> {
        for bytes in [
            b"short".to_vec(),
            vec![b'x'; simulation::PAYLOAD.len()],
            vec![b'x'; simulation::PAYLOAD.len() + 1],
        ] {
            let dir = crate::assets::temporary_directory()?;
            let cache = Cache::open(dir.path())?;
            assert!(
                cache
                    .import(
                        &spec()?,
                        bytes.as_slice(),
                        &Cancellation::default(),
                        |_, _| {}
                    )
                    .is_err()
            );
            assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        }
        Ok(())
    }
    #[test]
    fn cancellation_removes_partial_file() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let cancel = Cancellation::default();
        assert!(matches!(
            cache.import(&spec()?, simulation::PAYLOAD, &cancel, |_, _| cancel
                .cancel()),
            Err(Error::Cancelled)
        ));
        assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }
    #[test]
    fn cached_snapshot_survives_path_replacement() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let cancel = Cancellation::default();
        let a = spec()?;
        let mut snapshot = cache.import(&a, simulation::PAYLOAD, &cancel, |_, _| {})?;
        fs::write(dir.path().join(a.sha256()), b"corrupt")?;
        snapshot.recheck(&cancel)?;
        Ok(())
    }
    #[test]
    fn retries_after_failed_import() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let cancel = Cancellation::default();
        let a = spec()?;
        assert!(
            cache
                .import(&a, b"".as_slice(), &cancel, |_, _| {})
                .is_err()
        );
        cache.import(&a, simulation::PAYLOAD, &cancel, |_, _| {})?;
        Ok(())
    }
    #[test]
    fn offline_missing_asset_fails() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        assert!(
            cache
                .offline(&spec()?, dir.path(), &Cancellation::default())
                .is_err()
        );
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_cache_and_offline_file() -> Result<(), Error> {
        use std::os::unix::fs::symlink;
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let a = spec()?;
        symlink("missing", dir.path().join(a.sha256()))?;
        assert!(matches!(
            cache.lookup(&a, &Cancellation::default()),
            Err(Error::UnsafePath)
        ));
        assert!(
            cache
                .offline(&a, dir.path(), &Cancellation::default())
                .is_err()
        );
        symlink(dir.path(), dir.path().join("alias"))?;
        assert!(Cache::open(&dir.path().join("alias")).is_err());
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn rejects_shared_cache() -> Result<(), Error> {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::assets::temporary_directory()?;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777))?;
        assert!(matches!(Cache::open(dir.path()), Err(Error::UnsafePath)));
        Ok(())
    }
}
