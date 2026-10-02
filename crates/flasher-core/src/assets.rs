//! Bounded HTTPS/offline ingestion and a content-addressed atomic cache.
use crate::{
    Cancellation, Error,
    http::{HttpClient, UreqHttpClient},
    manifest::{Asset, Manifest, Role, https},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub struct Cache {
    root: PathBuf,
    http: Arc<dyn HttpClient>,
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
    #[must_use]
    pub fn sha256(&self) -> &str {
        self.spec.sha256()
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

/// The only token accepted by NAND planning.
///
/// Fields are private and the only constructor runs the full manifest,
/// role/size/hash and snapshot verification pipeline. Arbitrary paths or
/// unverified bytes can therefore never reach destructive planning.
#[derive(Debug)]
pub struct VerifiedAssets {
    manifest: Manifest,
    assets: Vec<VerifiedAsset>,
}
impl VerifiedAssets {
    /// Validates a complete acquired inventory against its manifest.
    /// # Errors
    /// Rejects incomplete inventories, role/size/hash mismatches, corrupted
    /// snapshots, cancellation and I/O errors.
    pub fn verify(
        manifest: Manifest,
        mut assets: Vec<VerifiedAsset>,
        cancel: &Cancellation,
    ) -> Result<Self, Error> {
        cancel.check()?;
        if assets.len() != manifest.assets().len() {
            return Err(Error::Manifest("incomplete verified inventory"));
        }
        for (asset, spec) in assets.iter_mut().zip(manifest.assets()) {
            if !asset.matches(spec) {
                return Err(Error::Manifest(
                    "asset inventory differs from selected release",
                ));
            }
            asset.recheck(cancel)?;
        }
        Ok(Self { manifest, assets })
    }
    #[must_use]
    pub const fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    #[must_use]
    pub const fn len(&self) -> usize {
        self.assets.len()
    }
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }
    /// Looks up one verified role.
    /// # Errors
    /// Returns a manifest error when the role is absent.
    pub fn asset(&self, role: Role) -> Result<&VerifiedAsset, Error> {
        self.assets
            .iter()
            .find(|asset| asset.role() == role)
            .ok_or(Error::Manifest("verified inventory lacks a required role"))
    }
    /// Mutable lookup used by the streaming stage.
    /// # Errors
    /// Returns a manifest error when the role is absent.
    pub fn asset_mut(&mut self, role: Role) -> Result<&mut VerifiedAsset, Error> {
        self.assets
            .iter_mut()
            .find(|asset| asset.role() == role)
            .ok_or(Error::Manifest("verified inventory lacks a required role"))
    }
    /// Rechecks every private snapshot immediately before device work.
    /// # Errors
    /// Returns corruption, cancellation or I/O errors.
    pub fn recheck_all(&mut self, cancel: &Cancellation) -> Result<(), Error> {
        for asset in &mut self.assets {
            asset.recheck(cancel)?;
        }
        Ok(())
    }
}

impl Cache {
    /// Opens an existing private directory with the production HTTPS client.
    /// # Errors
    /// Rejects symlink components, non-directories and group/world-writable Unix directories.
    pub fn open(root: &Path) -> Result<Self, Error> {
        Self::open_with_http(root, Arc::new(UreqHttpClient))
    }
    /// Opens the same validated directory with an injected HTTP client.
    /// # Errors
    /// Applies every production path check before accepting the client.
    pub fn open_with_http(root: &Path, http: Arc<dyn HttpClient>) -> Result<Self, Error> {
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
            http,
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
    /// Rejects HTTP/status/length errors, unsafe redirects and invalid checksums.
    pub fn download(
        &self,
        asset: &Asset,
        cancel: &Cancellation,
        progress: impl FnMut(u64, u64),
    ) -> Result<VerifiedAsset, Error> {
        cancel.check()?;
        let mut url = https(asset.url())?;
        for _ in 0..=5 {
            cancel.check()?;
            let response = self.http.get(&url, cancel)?;
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                let location = response.location.as_deref().ok_or(Error::Network)?;
                let next = url.join(location).map_err(|_| Error::Network)?;
                url = https(next.as_str())?;
                continue;
            }
            if response.status != 200 {
                return Err(Error::Network);
            }
            if let Some(length) = response.content_length
                && length != asset.size()
            {
                return Err(Error::Length);
            }
            return self.import(asset, response.body, cancel, progress);
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
/// Resolves an entire inventory from offline files or the cache/network and
/// verifies it against the manifest.
/// # Errors
/// Stops at the first unverified or missing asset; no device action is performed.
pub fn acquire(
    manifest: &Manifest,
    cache: &Cache,
    offline: Option<&Path>,
    cancel: &Cancellation,
    mut progress: impl FnMut(Role, u64, u64),
) -> Result<VerifiedAssets, Error> {
    let assets = manifest
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
        .collect::<Result<Vec<_>, Error>>()?;
    VerifiedAssets::verify(manifest.clone(), assets, cancel)
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
    let root = fs::canonicalize(std::env::temp_dir())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(root)?)
    }
    #[cfg(not(unix))]
    {
        Ok(tempfile::tempdir_in(root)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::{ScriptedHttpClient, ScriptedResponse},
        simulation,
    };
    #[cfg(unix)]
    #[test]
    fn snapshots_and_journals_receive_private_temporary_directories() {
        use std::os::unix::fs::PermissionsExt;
        let directory = temporary_directory().unwrap();
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        regular_components(directory.path()).unwrap();
    }
    fn spec() -> Result<Asset, Error> {
        Manifest::read(simulation::MANIFEST.as_bytes())?
            .assets()
            .first()
            .cloned()
            .ok_or(Error::State)
    }
    fn manifest() -> Result<Manifest, Error> {
        Manifest::read(simulation::MANIFEST.as_bytes())
    }
    fn imported(cache: &Cache, manifest: &Manifest) -> Result<Vec<VerifiedAsset>, Error> {
        manifest
            .assets()
            .iter()
            .map(|asset| {
                cache.import(
                    asset,
                    simulation::PAYLOAD,
                    &Cancellation::default(),
                    |_, _| {},
                )
            })
            .collect()
    }
    fn scripted_cache() -> Result<(tempfile::TempDir, Cache, Arc<ScriptedHttpClient>), Error> {
        let dir = crate::assets::temporary_directory()?;
        let client = Arc::new(ScriptedHttpClient::new());
        let cache = Cache::open_with_http(dir.path(), client.clone())?;
        Ok((dir, cache, client))
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
    #[test]
    fn verified_assets_accept_a_complete_verified_inventory() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let manifest = manifest()?;
        let assets = imported(&cache, &manifest)?;
        assert_eq!(
            VerifiedAssets::verify(manifest, assets, &Cancellation::default())?.len(),
            8
        );
        Ok(())
    }
    #[test]
    fn verified_assets_reject_an_incomplete_inventory() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let manifest = manifest()?;
        let mut assets = imported(&cache, &manifest)?;
        assets.pop();
        assert!(matches!(
            VerifiedAssets::verify(manifest, assets, &Cancellation::default()),
            Err(Error::Manifest(_))
        ));
        Ok(())
    }
    #[test]
    fn verified_assets_reject_altered_hash_or_size_specs() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let original = manifest()?;
        let first = original.assets().first().ok_or(Error::State)?;
        let altered_specs = [
            ("sha256", serde_json::Value::from("0".repeat(64))),
            ("size", serde_json::Value::from(first.size() + 1)),
        ];
        for (field, replacement) in altered_specs {
            let assets = imported(&cache, &original)?;
            let mut value: serde_json::Value =
                serde_json::from_str(simulation::MANIFEST).map_err(|_| Error::State)?;
            value["assets"][0][field] = replacement;
            let altered = Manifest::read(value.to_string().as_bytes())?;
            assert!(
                matches!(
                    VerifiedAssets::verify(altered, assets, &Cancellation::default()),
                    Err(Error::Manifest(_))
                ),
                "{field} change must be rejected"
            );
        }
        Ok(())
    }
    #[test]
    fn verified_assets_reject_a_role_mismatch() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let original = manifest()?;
        let assets = imported(&cache, &original)?;
        let mut value: serde_json::Value =
            serde_json::from_str(simulation::MANIFEST).map_err(|_| Error::State)?;
        value["assets"]
            .as_array_mut()
            .ok_or(Error::State)?
            .swap(0, 1);
        let altered = Manifest::read(value.to_string().as_bytes())?;
        assert!(matches!(
            VerifiedAssets::verify(altered, assets, &Cancellation::default()),
            Err(Error::Manifest(_))
        ));
        Ok(())
    }
    #[test]
    fn verified_assets_reject_cancellation() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let manifest = manifest()?;
        let assets = imported(&cache, &manifest)?;
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            VerifiedAssets::verify(manifest, assets, &cancel),
            Err(Error::Cancelled)
        ));
        Ok(())
    }
    #[test]
    fn download_success_is_verified_and_single_use() -> Result<(), Error> {
        let (dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        client.expect_response(&url, ScriptedResponse::ok(simulation::PAYLOAD.to_vec()));
        let asset = cache.download(&a, &Cancellation::default(), |_, _| {})?;
        assert_eq!(asset.size(), a.size());
        assert_eq!(client.remaining(), 0);
        assert!(dir.path().join(a.sha256()).is_file());
        Ok(())
    }
    #[test]
    fn download_http_error_publishes_nothing() -> Result<(), Error> {
        let (dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        client.expect_response(&url, ScriptedResponse::ok(Vec::new()).status(404));
        assert!(matches!(
            cache.download(&a, &Cancellation::default(), |_, _| {}),
            Err(Error::Network)
        ));
        assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }
    #[test]
    fn download_truncated_body_is_rejected() -> Result<(), Error> {
        let (dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        client.expect_response(
            &url,
            ScriptedResponse::ok(b"short".to_vec()).content_length(a.size()),
        );
        assert!(matches!(
            cache.download(&a, &Cancellation::default(), |_, _| {}),
            Err(Error::Length)
        ));
        assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }
    #[test]
    fn download_wrong_content_length_is_rejected() -> Result<(), Error> {
        let (dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        client.expect_response(
            &url,
            ScriptedResponse::ok(simulation::PAYLOAD.to_vec()).content_length(a.size() + 1),
        );
        assert!(matches!(
            cache.download(&a, &Cancellation::default(), |_, _| {}),
            Err(Error::Length)
        ));
        assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }
    #[test]
    fn download_hash_mismatch_is_rejected() -> Result<(), Error> {
        let (dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        let mut body = simulation::PAYLOAD.to_vec();
        if let Some(first) = body.first_mut() {
            *first ^= 0xff;
        }
        client.expect_response(&url, ScriptedResponse::ok(body));
        assert!(matches!(
            cache.download(&a, &Cancellation::default(), |_, _| {}),
            Err(Error::Hash)
        ));
        assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }
    #[test]
    fn download_timeout_is_reported() -> Result<(), Error> {
        let (_dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        client.expect_error(&url, Error::Timeout);
        assert!(matches!(
            cache.download(&a, &Cancellation::default(), |_, _| {}),
            Err(Error::Timeout)
        ));
        Ok(())
    }
    #[test]
    fn download_cancellation_removes_partial_file() -> Result<(), Error> {
        let (dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        client.expect_response(&url, ScriptedResponse::ok(simulation::PAYLOAD.to_vec()));
        let cancel = Cancellation::default();
        assert!(matches!(
            cache.download(&a, &cancel, |_, _| cancel.cancel()),
            Err(Error::Cancelled)
        ));
        assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }
    #[test]
    fn download_follows_bounded_https_redirects() -> Result<(), Error> {
        let (_dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        let next = url.join("redirected").map_err(|_| Error::Network)?;
        client.expect_response(
            &url,
            ScriptedResponse::ok(Vec::new())
                .status(302)
                .location("redirected"),
        );
        client.expect_response(&next, ScriptedResponse::ok(simulation::PAYLOAD.to_vec()));
        let asset = cache.download(&a, &Cancellation::default(), |_, _| {})?;
        assert_eq!(asset.size(), a.size());
        Ok(())
    }
    #[test]
    fn download_redirect_limit_is_enforced() -> Result<(), Error> {
        let (_dir, cache, client) = scripted_cache()?;
        let a = spec()?;
        let url = https(a.url())?;
        let next = url.join("loop").map_err(|_| Error::Network)?;
        client.expect_response(
            &url,
            ScriptedResponse::ok(Vec::new())
                .status(302)
                .location("loop"),
        );
        for _ in 0..5 {
            client.expect_response(
                &next,
                ScriptedResponse::ok(Vec::new())
                    .status(302)
                    .location("loop"),
            );
        }
        assert!(matches!(
            cache.download(&a, &Cancellation::default(), |_, _| {}),
            Err(Error::Network)
        ));
        Ok(())
    }
    #[test]
    fn offline_acquisition_never_calls_http() -> Result<(), Error> {
        let (dir, cache, client) = scripted_cache()?;
        let manifest = manifest()?;
        for asset in manifest.assets() {
            fs::write(dir.path().join(asset.sha256()), simulation::PAYLOAD)?;
        }
        let verified = acquire(
            &manifest,
            &cache,
            Some(dir.path()),
            &Cancellation::default(),
            |_, _, _| {},
        )?;
        assert_eq!(verified.len(), 8);
        assert_eq!(client.calls(), [] as [url::Url; 0]);
        Ok(())
    }
    #[test]
    fn offline_missing_asset_never_calls_http() -> Result<(), Error> {
        let (dir, cache, client) = scripted_cache()?;
        let manifest = manifest()?;
        assert!(
            acquire(
                &manifest,
                &cache,
                Some(dir.path()),
                &Cancellation::default(),
                |_, _, _| {}
            )
            .is_err()
        );
        assert_eq!(client.calls(), [] as [url::Url; 0]);
        Ok(())
    }
    #[test]
    fn acquisition_cancellation_stops_before_http() -> Result<(), Error> {
        let (_dir, cache, client) = scripted_cache()?;
        let manifest = manifest()?;
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            acquire(&manifest, &cache, None, &cancel, |_, _, _| {}),
            Err(Error::Cancelled)
        ));
        assert_eq!(client.calls(), [] as [url::Url; 0]);
        Ok(())
    }
}
