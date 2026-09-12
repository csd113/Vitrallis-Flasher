//! Reproducible offline fixtures. These bytes are deliberately not bootable.
use crate::{
    Cancellation, Error,
    assets::{Cache, VerifiedAsset},
    manifest::Manifest,
    profile::Profile,
};
pub const MANIFEST: &str = include_str!("../../../manifests/simulation.json");
pub const VITRALLIS_MANIFEST: &str = include_str!("../../../manifests/simulation-vitrallis.json");
pub const PAYLOAD: &[u8] = b"Vitrallis recovery simulation fixture. NOT A BOOTABLE IMAGE.\n";
/// Returns a complete verified mock asset set using the normal cache pipeline.
/// # Errors
/// Returns fixture-contract, cache or cancellation errors.
pub fn prepare(
    cache: &Cache,
    cancel: &Cancellation,
) -> Result<(Manifest, Vec<VerifiedAsset>), Error> {
    prepare_profile(cache, cancel, Profile::default())
}
/// Prepares a profile-specific, nonbootable simulation through the same asset checks.
/// # Errors
/// Returns fixture-contract, cache or cancellation errors.
pub fn prepare_profile(
    cache: &Cache,
    cancel: &Cancellation,
    profile: Profile,
) -> Result<(Manifest, Vec<VerifiedAsset>), Error> {
    let manifest = crate::releases::select(
        crate::releases::Channel::Simulation,
        profile.simulation_release(),
    )?;
    let assets = manifest
        .assets()
        .iter()
        .map(|a| cache.import(a, PAYLOAD, cancel, |_, _| {}))
        .collect::<Result<_, _>>()?;
    Ok((manifest, assets))
}
