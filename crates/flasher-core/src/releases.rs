//! Compiled release selection. Candidate research assets are never approvals.
use crate::{Error, manifest::Manifest, profile::Profile, simulation};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Simulation,
    ApprovedPhysical,
}
#[derive(Debug)]
pub struct Release {
    pub id: &'static str,
    pub label: &'static str,
    pub profile: Profile,
}
const SIMULATED: &[Release] = &[
    Release {
        id: "simulation-debian13",
        label: "Debian 13 + stock PocketHome simulation (nonbootable)",
        profile: Profile::Stock,
    },
    Release {
        id: "simulation-debian13-vitrallis-default",
        label: "Debian 13 + Vitrallis default simulation (nonbootable)",
        profile: Profile::VitrallisDefault,
    },
];
/// The physical catalog stays empty until complete image/protocol approval.
#[must_use]
pub const fn catalog(channel: Channel) -> &'static [Release] {
    match channel {
        Channel::Simulation => SIMULATED,
        Channel::ApprovedPhysical => &[],
    }
}
/// Selects only a compiled release identifier; never resolves a moving latest URL.
/// # Errors
/// Rejects absent, unknown and unapproved physical releases.
pub fn select(channel: Channel, id: &str) -> Result<Manifest, Error> {
    if channel != Channel::Simulation || !catalog(channel).iter().any(|r| r.id == id) {
        return Err(Error::Manifest("release is not in the approved catalog"));
    }
    let release = catalog(channel)
        .iter()
        .find(|r| r.id == id)
        .ok_or(Error::State)?;
    let source = match release.profile {
        Profile::Stock => simulation::MANIFEST,
        Profile::VitrallisDefault => simulation::VITRALLIS_MANIFEST,
    };
    let manifest = Manifest::read(source.as_bytes())?;
    if manifest.profile() != release.profile || manifest.release() != release.id {
        return Err(Error::Manifest(
            "compiled release and image profile disagree",
        ));
    }
    Ok(manifest)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_choices_bind_different_confirmation_digests() -> Result<(), Error> {
        let stock = select(Channel::Simulation, Profile::Stock.simulation_release())?;
        let shell = select(
            Channel::Simulation,
            Profile::VitrallisDefault.simulation_release(),
        )?;
        assert_eq!(stock.profile(), Profile::Stock);
        assert_eq!(shell.profile(), Profile::VitrallisDefault);
        assert_ne!(stock.digest(), shell.digest());
        for profile in Profile::ALL {
            assert!(select(Channel::ApprovedPhysical, profile.simulation_release()).is_err());
        }
        Ok(())
    }
    #[test]
    fn lock_approvals_agree_with_the_empty_physical_catalog() -> Result<(), Error> {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../upstream-lock.json"))
                .map_err(|_| Error::State)?;
        let approved = lock["approved_physical_manifest_sha256"]
            .as_array()
            .ok_or(Error::State)?;
        assert_eq!(
            catalog(Channel::ApprovedPhysical).is_empty(),
            approved.is_empty()
        );
        for digest in approved {
            let value = digest.as_str().ok_or(Error::State)?;
            assert_eq!(value.len(), 64);
            assert!(
                value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
        }
        Ok(())
    }

    #[test]
    fn candidates_cannot_be_selected_as_physical_releases() {
        assert!(catalog(Channel::ApprovedPhysical).is_empty());
        assert!(select(Channel::ApprovedPhysical, "simulation-debian13").is_err());
        assert!(select(Channel::Simulation, "latest").is_err());
        assert!(select(Channel::Simulation, "simulation-debian13").is_ok());
    }
}
