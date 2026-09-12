//! Strict image contract. A valid manifest is not an authorization to flash.
use crate::{Error, profile::Profile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Read};
use url::Url;

pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MAX_ASSET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Uboot,
    Kernel,
    Dtb,
    Recovery,
    SplHynix,
    SplToshiba,
    UbootNand,
    Rootfs,
}
impl Role {
    pub const ALL: [Self; 8] = [
        Self::Uboot,
        Self::Kernel,
        Self::Dtb,
        Self::Recovery,
        Self::SplHynix,
        Self::SplToshiba,
        Self::UbootNand,
        Self::Rootfs,
    ];
    #[must_use]
    pub const fn limit(self) -> u64 {
        match self {
            Self::Rootfs => MAX_ASSET_BYTES,
            Self::Recovery => 40 * 1024 * 1024,
            Self::Kernel => 16 * 1024 * 1024,
            Self::Dtb => 1024 * 1024,
            Self::SplHynix | Self::SplToshiba => 8 * 1024 * 1024,
            Self::Uboot | Self::UbootNand => 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub repository: String,
    pub commit: String,
    pub license: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAsset {
    pub role: Role,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub provenance: Provenance,
}
#[derive(Debug, Clone)]
pub struct Asset(RawAsset);
impl Asset {
    #[must_use]
    pub const fn role(&self) -> Role {
        self.0.role
    }
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.0.size
    }
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.0.sha256
    }
    #[must_use]
    pub fn url(&self) -> &str {
        &self.0.url
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawManifest {
    pub schema_version: u32,
    pub release: String,
    pub board: String,
    pub soc: String,
    pub os: String,
    pub architecture: String,
    pub installer_protocol: u32,
    pub minimum_flasher: String,
    pub profile: Profile,
    pub vitrallis: String,
    pub assets: Vec<RawAsset>,
}
#[derive(Debug, Clone)]
pub struct Manifest {
    release: String,
    profile: Profile,
    digest: String,
    assets: Vec<Asset>,
}
impl Manifest {
    /// Opens a bounded manifest from a regular file with no symlink components.
    /// # Errors
    /// Rejects unsafe paths, I/O failures and invalid contracts.
    pub fn open(path: &std::path::Path) -> Result<Self, Error> {
        crate::assets::regular_components(path)?;
        if !std::fs::symlink_metadata(path)?.is_file() {
            return Err(Error::UnsafePath);
        }
        Self::read(std::fs::File::open(path)?)
    }
    /// Parses at most 64 KiB, rejects unknown fields and validates every asset.
    /// # Errors
    /// Rejects unsupported versions, devices, URLs, sizes and provenance.
    pub fn read(reader: impl Read) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        reader
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(Error::Manifest("document too large"));
        }
        let raw: RawManifest = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Manifest("malformed JSON or unknown fields"))?;
        if raw.schema_version != 1 || raw.installer_protocol != 1 || raw.minimum_flasher != "0.1.0"
        {
            return Err(Error::Manifest("unsupported contract version"));
        }
        if raw.board != "pocketchip"
            || raw.soc != "allwinner-r8"
            || raw.os != "debian-13-trixie"
            || raw.architecture != "armhf"
        {
            return Err(Error::Manifest("incompatible board, SoC or OS"));
        }
        if raw.vitrallis
            != match raw.profile {
                Profile::Stock => "not-installed",
                Profile::VitrallisDefault => "blocked",
            }
        {
            return Err(Error::Manifest(
                "desktop profile and Vitrallis compatibility disagree",
            ));
        }
        if !identifier(&raw.release) || raw.assets.len() != Role::ALL.len() {
            return Err(Error::Manifest("invalid release or asset inventory"));
        }
        let mut roles = BTreeSet::new();
        for asset in &raw.assets {
            if !roles.insert(asset.role)
                || asset.size == 0
                || asset.size > asset.role.limit()
                || !hex(&asset.sha256, 64)
            {
                return Err(Error::Manifest("invalid role, size or SHA-256"));
            }
            https(&asset.url)?;
            https(&asset.provenance.repository)?;
            if !hex(&asset.provenance.commit, 40) || !identifier(&asset.provenance.license) {
                return Err(Error::Manifest("missing immutable provenance"));
            }
        }
        Ok(Self {
            release: raw.release,
            profile: raw.profile,
            digest: format!("{:x}", Sha256::digest(&bytes)),
            assets: raw.assets.into_iter().map(Asset).collect(),
        })
    }
    #[must_use]
    pub const fn profile(&self) -> Profile {
        self.profile
    }
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
    #[must_use]
    pub fn assets(&self) -> &[Asset] {
        &self.assets
    }
}
pub(crate) fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._+".contains(&b))
}
/// Validates transport URLs, also used for each redirect.
/// # Errors
/// Rejects non-HTTPS, credentials, fragments, nonstandard ports and local hosts.
pub fn https(value: &str) -> Result<Url, Error> {
    if value.len() > 2048 || value.bytes().any(|b| b.is_ascii_control() || b == b'\\') {
        return Err(Error::Manifest("invalid HTTPS URL"));
    }
    let u = Url::parse(value).map_err(|_| Error::Manifest("invalid HTTPS URL"))?;
    let host = u.host_str().ok_or(Error::Manifest("URL has no host"))?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
        || u.port().is_some_and(|p| p != 443)
        || !host.contains('.')
        || host.ends_with('.')
        || host.ends_with(".localhost")
        || host.rsplit('.').next() == Some("local")
        || host.parse::<std::net::IpAddr>().is_ok()
        || host.starts_with('[')
    {
        return Err(Error::Manifest(
            "HTTPS public hostname without credentials required",
        ));
    }
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation;
    fn altered(change: impl FnOnce(&mut serde_json::Value)) -> Result<Manifest, Error> {
        let mut value: serde_json::Value =
            serde_json::from_str(simulation::MANIFEST).map_err(|_| Error::State)?;
        change(&mut value);
        Manifest::read(value.to_string().as_bytes())
    }
    #[test]
    fn fixture_is_valid() -> Result<(), Error> {
        assert_eq!(
            Manifest::read(simulation::MANIFEST.as_bytes())?
                .assets()
                .len(),
            8
        );
        Ok(())
    }
    #[test]
    fn rejects_unknown_commands() {
        assert!(altered(|v| v["command"] = "erase".into()).is_err());
    }
    #[test]
    fn rejects_unknown_nested_fields() {
        assert!(altered(|v| v["assets"][0]["command"] = "erase".into()).is_err());
    }
    #[test]
    fn rejects_duplicate_fields() {
        assert!(Manifest::read(b"{\"schema_version\":1,\"schema_version\":1}".as_slice()).is_err());
    }
    #[test]
    fn rejects_malformed_and_oversized() {
        assert!(Manifest::read(b"{".as_slice()).is_err());
        assert!(Manifest::read(std::io::repeat(b' ').take(MAX_MANIFEST_BYTES + 1)).is_err());
    }
    #[test]
    fn rejects_versions_and_wrong_board() {
        for key in [
            "board",
            "soc",
            "architecture",
            "os",
            "minimum_flasher",
            "vitrallis",
            "profile",
        ] {
            assert!(altered(|v| v[key] = "unknown".into()).is_err());
        }
        for key in ["schema_version", "installer_protocol"] {
            assert!(altered(|v| v[key] = 2.into()).is_err());
        }
    }
    #[test]
    fn rejects_malformed_hashes_and_provenance() {
        for hash in ["", "ABCDEF", &"0".repeat(63), &"x".repeat(64)] {
            assert!(altered(|v| v["assets"][0]["sha256"] = hash.into()).is_err());
        }
        assert!(altered(|v| v["assets"][0]["provenance"]["commit"] = "master".into()).is_err());
        assert!(altered(|v| v["assets"][0]["provenance"]["license"] = "".into()).is_err());
    }
    #[test]
    fn rejects_invalid_inventory_and_size() {
        assert!(altered(|v| v["assets"][0]["role"] = "rootfs".into()).is_err());
        for size in [0, MAX_ASSET_BYTES + 1] {
            assert!(altered(|v| v["assets"][0]["size"] = size.into()).is_err());
        }
        assert!(altered(|v| v["assets"] = serde_json::json!([])).is_err());
    }
    #[test]
    fn profile_is_required_and_cannot_claim_shell_readiness() {
        assert!(
            altered(|v| {
                v.as_object_mut().map(|o| o.remove("profile"));
            })
            .is_err()
        );
        assert!(altered(|v| v["vitrallis"] = "blocked".into()).is_err());
        assert!(altered(|v| v["profile"] = "vitrallis-default".into()).is_err());
        assert!(
            altered(|v| {
                v["profile"] = "vitrallis-default".into();
                v["vitrallis"] = "ready".into();
            })
            .is_err()
        );
    }
    #[test]
    fn https_rejects_unsafe_urls() {
        for url in [
            "http://example.com/a",
            "file:///a",
            "https://u:p@example.com/a",
            "https://example.com/#a",
            "https://localhost/a",
            "https://127.0.0.1/a",
            "https://[::1]/a",
            "https://example.com:22/a",
            "https://example.com/\n",
            "https://host.local/a",
        ] {
            assert!(https(url).is_err(), "{url}");
        }
        assert!(https("https://example.com/release/asset?download=1").is_ok());
    }
}
