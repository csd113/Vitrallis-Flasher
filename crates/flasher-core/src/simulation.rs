//! Reproducible offline fixtures. These bytes are deliberately not bootable.
use crate::{
    Cancellation, Error,
    assets::{Cache, VerifiedAssets},
    profile::Profile,
};
pub const MANIFEST: &str = include_str!("../../../manifests/simulation.json");
pub const VITRALLIS_MANIFEST: &str = include_str!("../../../manifests/simulation-vitrallis.json");
pub const PAYLOAD: &[u8] = b"Vitrallis recovery simulation fixture. NOT A BOOTABLE IMAGE.\n";
/// Returns a complete verified mock asset set using the normal cache pipeline.
/// # Errors
/// Returns fixture-contract, cache or cancellation errors.
pub fn prepare(cache: &Cache, cancel: &Cancellation) -> Result<VerifiedAssets, Error> {
    prepare_profile(cache, cancel, Profile::default())
}
/// Prepares a profile-specific, nonbootable simulation through the same asset checks.
/// # Errors
/// Returns fixture-contract, cache or cancellation errors.
pub fn prepare_profile(
    cache: &Cache,
    cancel: &Cancellation,
    profile: Profile,
) -> Result<VerifiedAssets, Error> {
    let manifest = crate::releases::select(
        crate::releases::Channel::Simulation,
        profile.simulation_release(),
    )?;
    let assets = manifest
        .assets()
        .iter()
        .map(|a| cache.import(a, PAYLOAD, cancel, |_, _| {}))
        .collect::<Result<Vec<_>, _>>()?;
    VerifiedAssets::verify(manifest, assets, cancel)
}
