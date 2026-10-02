//! Reviewable NAND installation planning. Batch 2 can plan; it can never execute.
//!
//! A [`NandPlan`] describes the exact ordered host/device operations a future
//! install would attempt, including byte lengths and digests taken from
//! [`VerifiedAssets`]. Physical facts that Batch 1/2 have not established stay
//! unresolved as explicit [`PlanGate`]s, and [`NandPlan::authorize_execution`]
//! always fails: no destructive production plan can execute in this build.
//! Restricted original-SPL trial helpers are separate developer diagnostics;
//! they neither approve a release nor satisfy these production gates.
use crate::{
    Error,
    assets::VerifiedAssets,
    device::{IdentifiedTarget, Nand},
    manifest::Role,
    session::Stage,
};

/// Primary SPL slot (one erase block), as established in `docs/boot-layout.md`.
pub const SPL_OFFSET: u64 = 0x0;
/// Backup SPL slot.
pub const SPL_BACKUP_OFFSET: u64 = 0x40_0000;
/// Padded U-Boot slot.
pub const UBOOT_OFFSET: u64 = 0x80_0000;
/// Hynix U-Boot fallback slot, physically booted in Batch 3 session 17.
pub const UBOOT_BACKUP_OFFSET: u64 = 0xC0_0000;
/// Rootfs UBI volume start.
pub const ROOTFS_OFFSET: u64 = 0x100_0000;
/// Established NAND page size.
pub const PAGE_BYTES: u64 = 16_384;
/// Upstream erases the whole boot region before rewriting it.
pub const BOOT_REGION_BYTES: u64 = 0x100_0000;
/// Padded U-Boot must occupy exactly one erase block.
pub const BOOT_SLOT_BYTES: u64 = 0x40_0000;

/// What one planned step does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanKind {
    BootRecovery,
    EraseBootRegion,
    WriteSpl,
    WriteUboot,
    InstallRootfs,
    VerifyBootloader,
    VerifyRootfs,
}

/// Where a step acts. `Ram { address: None }` records an unresolved address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Ram { address: Option<u32> },
    Nand { offset: u64 },
    UbiVolume { name: &'static str },
}

/// Ordering constraints a step depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prerequisite {
    FreshIdentification,
    VerifiedAssets,
    RecoveryBooted,
    BootRegionErased,
    BootloaderWritten,
    RootfsInstalled,
}

/// How a step's result must be checked before progress continues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verification {
    TransportAck,
    Readback {
        offset: u64,
        length: u64,
        sha256: String,
    },
    OnDeviceInventory {
        description: &'static str,
    },
}

/// One ordered, reviewable operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NandStep {
    pub kind: PlanKind,
    pub source: Option<Role>,
    pub placement: Placement,
    pub length: Option<u64>,
    pub sha256: Option<String>,
    pub prerequisites: Vec<Prerequisite>,
    pub verification: Verification,
    pub stage: Stage,
}

/// A physical question that must be resolved before execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateId {
    UnapprovedManifest,
    AuthenticatedRecovery,
    RecoveryLoadAddress,
    NandGeometry,
    BadBlocks,
    UbootBackupAmbiguity,
    BromAcceptance,
    SplBehaviour,
}

/// One unresolved gate with a short reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanGate {
    pub id: GateId,
    pub description: &'static str,
}
const GATES: &[PlanGate] = &[
    PlanGate {
        id: GateId::UnapprovedManifest,
        description: "no physical manifest is approved; no execution authorization exists",
    },
    PlanGate {
        id: GateId::AuthenticatedRecovery,
        description: "diagnostic SID-bound recovery is measured; approved release recovery identity and executor binding remain unresolved",
    },
    PlanGate {
        id: GateId::RecoveryLoadAddress,
        description: "diagnostic recovery load addresses are measured; release-specific recovery asset approval remains unresolved",
    },
    PlanGate {
        id: GateId::NandGeometry,
        description: "Hynix geometry is measured; final ECC-aware verification and executable per-part platform policy remain unresolved",
    },
    PlanGate {
        id: GateId::BadBlocks,
        description: "bad-block handling and power-loss behavior during bootloader rewrite are unverified",
    },
    PlanGate {
        id: GateId::UbootBackupAmbiguity,
        description: "0xC00000 is claimed as both redundant U-Boot and environment; no operation is planned there",
    },
    PlanGate {
        id: GateId::BromAcceptance,
        description: "BROM/SPL acceptance of the generated NAND images is unverified on both parts",
    },
    PlanGate {
        id: GateId::SplBehaviour,
        description: "actual SPL primary/backup selection behavior is unverified on hardware",
    },
];

/// An immutable review document. It cannot be executed by this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NandPlan {
    release: String,
    manifest_digest: String,
    target: IdentifiedTarget,
    steps: Vec<NandStep>,
    gates: Vec<PlanGate>,
}
impl NandPlan {
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }
    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }
    #[must_use]
    pub const fn target(&self) -> &IdentifiedTarget {
        &self.target
    }
    #[must_use]
    pub fn steps(&self) -> &[NandStep] {
        &self.steps
    }
    #[must_use]
    pub fn gates(&self) -> &[PlanGate] {
        &self.gates
    }
    /// Always false in Batch 2; there is no executor.
    #[must_use]
    pub const fn is_executable(&self) -> bool {
        false
    }
    /// The hard execution gate. Batch 2 has no code path that can succeed.
    /// # Errors
    /// Always returns `PhysicalBlocked`.
    pub const fn authorize_execution(&self) -> Result<(), Error> {
        Err(Error::PhysicalBlocked)
    }
}

/// Builds the review plan for one verified artifact set and one identified target.
///
/// # Errors
/// Fails closed when the target is unknown, the target's SPL role is absent from
/// the verified inventory, required roles are missing, or the planned layout is
/// invalid. It never performs device I/O.
pub fn plan(assets: &VerifiedAssets, target: &IdentifiedTarget) -> Result<NandPlan, Error> {
    let spl_role = target.spl_role();
    let spl = assets.asset(spl_role)?;
    let recovery = assets.asset(Role::Recovery)?;
    let uboot = assets.asset(Role::UbootNand)?;
    let rootfs = assets.asset(Role::Rootfs)?;
    let mut steps = Vec::from(boot_steps(spl_role, spl, recovery, uboot));
    if target.nand() == Nand::Hynix {
        steps.push(uboot_backup_step(uboot, PlanKind::WriteUboot, Stage::Flash));
    }
    steps.push(install_rootfs_step(rootfs));
    steps.extend(verify_steps(uboot, rootfs));
    if target.nand() == Nand::Hynix {
        steps.push(uboot_backup_step(
            uboot,
            PlanKind::VerifyBootloader,
            Stage::Verify,
        ));
    }
    let plan = NandPlan {
        release: assets.manifest().release().to_owned(),
        manifest_digest: assets.manifest().digest().to_owned(),
        target: target.clone(),
        steps,
        gates: GATES
            .iter()
            .copied()
            .filter(|gate| target.nand() != Nand::Hynix || gate.id != GateId::UbootBackupAmbiguity)
            .collect(),
    };
    validate(&plan)?;
    Ok(plan)
}

fn readback(offset: u64, asset: &crate::assets::VerifiedAsset) -> Verification {
    Verification::Readback {
        offset,
        length: asset.size(),
        sha256: asset.sha256().to_owned(),
    }
}

fn boot_steps(
    spl_role: Role,
    spl: &crate::assets::VerifiedAsset,
    recovery: &crate::assets::VerifiedAsset,
    uboot: &crate::assets::VerifiedAsset,
) -> [NandStep; 5] {
    [
        NandStep {
            kind: PlanKind::BootRecovery,
            source: Some(Role::Recovery),
            placement: Placement::Ram { address: None },
            length: Some(recovery.size()),
            sha256: Some(recovery.sha256().to_owned()),
            prerequisites: vec![
                Prerequisite::FreshIdentification,
                Prerequisite::VerifiedAssets,
            ],
            verification: Verification::TransportAck,
            stage: Stage::Flash,
        },
        NandStep {
            kind: PlanKind::EraseBootRegion,
            source: None,
            placement: Placement::Nand { offset: SPL_OFFSET },
            length: Some(BOOT_REGION_BYTES),
            sha256: None,
            prerequisites: vec![Prerequisite::RecoveryBooted],
            verification: Verification::TransportAck,
            stage: Stage::Flash,
        },
        NandStep {
            kind: PlanKind::WriteSpl,
            source: Some(spl_role),
            placement: Placement::Nand { offset: SPL_OFFSET },
            length: Some(spl.size()),
            sha256: Some(spl.sha256().to_owned()),
            prerequisites: vec![Prerequisite::BootRegionErased],
            verification: readback(SPL_OFFSET, spl),
            stage: Stage::Flash,
        },
        NandStep {
            kind: PlanKind::WriteSpl,
            source: Some(spl_role),
            placement: Placement::Nand {
                offset: SPL_BACKUP_OFFSET,
            },
            length: Some(spl.size()),
            sha256: Some(spl.sha256().to_owned()),
            prerequisites: vec![Prerequisite::BootRegionErased],
            verification: readback(SPL_BACKUP_OFFSET, spl),
            stage: Stage::Flash,
        },
        NandStep {
            kind: PlanKind::WriteUboot,
            source: Some(Role::UbootNand),
            placement: Placement::Nand {
                offset: UBOOT_OFFSET,
            },
            length: Some(uboot.size()),
            sha256: Some(uboot.sha256().to_owned()),
            prerequisites: vec![Prerequisite::BootRegionErased],
            verification: readback(UBOOT_OFFSET, uboot),
            stage: Stage::Flash,
        },
    ]
}

fn uboot_backup_step(
    uboot: &crate::assets::VerifiedAsset,
    kind: PlanKind,
    stage: Stage,
) -> NandStep {
    NandStep {
        kind,
        source: Some(Role::UbootNand),
        placement: Placement::Nand {
            offset: UBOOT_BACKUP_OFFSET,
        },
        length: Some(uboot.size()),
        sha256: Some(uboot.sha256().to_owned()),
        prerequisites: vec![if kind == PlanKind::WriteUboot {
            Prerequisite::BootRegionErased
        } else {
            Prerequisite::BootloaderWritten
        }],
        verification: readback(UBOOT_BACKUP_OFFSET, uboot),
        stage,
    }
}

fn install_rootfs_step(rootfs: &crate::assets::VerifiedAsset) -> NandStep {
    NandStep {
        kind: PlanKind::InstallRootfs,
        source: Some(Role::Rootfs),
        placement: Placement::UbiVolume { name: "rootfs" },
        length: Some(rootfs.size()),
        sha256: Some(rootfs.sha256().to_owned()),
        prerequisites: vec![Prerequisite::BootloaderWritten],
        verification: Verification::OnDeviceInventory {
            description: "device-side UBIFS volume inventory after sync and detach",
        },
        stage: Stage::Flash,
    }
}

fn verify_steps(
    uboot: &crate::assets::VerifiedAsset,
    rootfs: &crate::assets::VerifiedAsset,
) -> [NandStep; 2] {
    [
        NandStep {
            kind: PlanKind::VerifyBootloader,
            source: Some(Role::UbootNand),
            placement: Placement::Nand {
                offset: UBOOT_OFFSET,
            },
            length: Some(uboot.size()),
            sha256: Some(uboot.sha256().to_owned()),
            prerequisites: vec![Prerequisite::BootloaderWritten],
            verification: readback(UBOOT_OFFSET, uboot),
            stage: Stage::Verify,
        },
        NandStep {
            kind: PlanKind::VerifyRootfs,
            source: Some(Role::Rootfs),
            placement: Placement::UbiVolume { name: "rootfs" },
            length: Some(rootfs.size()),
            sha256: Some(rootfs.sha256().to_owned()),
            prerequisites: vec![Prerequisite::RootfsInstalled],
            verification: Verification::OnDeviceInventory {
                description: "canonical UBIFS inventory including metadata, ECC and bad blocks",
            },
            stage: Stage::Verify,
        },
    ]
}

/// Re-derives every plan invariant. Used by [`plan`] and by safety tests.
/// # Errors
/// Rejects a wrong SPL variant, missing exact length/digest, out-of-order
/// stages or overlapping/invalid NAND placements.
pub fn validate(plan: &NandPlan) -> Result<(), Error> {
    for step in &plan.steps {
        match step.kind {
            PlanKind::WriteSpl => {
                if step.source != Some(plan.target.spl_role()) {
                    return Err(Error::Device);
                }
                require_exact(step)?;
            }
            PlanKind::WriteUboot | PlanKind::VerifyBootloader | PlanKind::VerifyRootfs => {
                require_exact(step)?;
            }
            PlanKind::InstallRootfs => {
                if step.source.is_none() || step.length.is_none() || step.sha256.is_none() {
                    return Err(Error::Manifest("rootfs step lacks a verified source"));
                }
            }
            PlanKind::BootRecovery | PlanKind::EraseBootRegion => {}
        }
    }
    check_layout(&plan.steps)
}

const fn require_exact(step: &NandStep) -> Result<(), Error> {
    if step.length.is_none() || step.sha256.is_none() || step.source.is_none() {
        return Err(Error::Manifest(
            "planned write lacks an exact length and digest",
        ));
    }
    Ok(())
}

/// Validates ordering and non-overlap of every NAND placement.
/// # Errors
/// Rejects unaligned offsets, boot writes beyond the boot region, integer
/// overflow and overlapping ranges.
pub fn check_layout(steps: &[NandStep]) -> Result<(), Error> {
    let mut regions: Vec<(u64, u64)> = Vec::new();
    let mut last_stage = 0_usize;
    for step in steps {
        if step.stage.index() < last_stage {
            return Err(Error::Manifest("planned steps are out of stage order"));
        }
        last_stage = step.stage.index();
        let Placement::Nand { offset } = step.placement else {
            continue;
        };
        if offset % PAGE_BYTES != 0 {
            return Err(Error::Manifest("planned NAND offset is not page aligned"));
        }
        if let Some(length) = step.length {
            let end = offset
                .checked_add(length)
                .ok_or(Error::Manifest("planned NAND range overflows"))?;
            if step.kind != PlanKind::InstallRootfs && end > ROOTFS_OFFSET {
                return Err(Error::Manifest(
                    "planned bootloader write exceeds the boot region",
                ));
            }
            // Only writes occupy a range. An erase covers slots that later
            // writes repopulate, and a readback verify intentionally reads a
            // range a write already occupies.
            if matches!(step.kind, PlanKind::WriteSpl | PlanKind::WriteUboot) {
                for (start, existing_end) in &regions {
                    if offset < *existing_end && *start < end {
                        return Err(Error::Manifest("planned NAND ranges overlap"));
                    }
                }
                regions.push((offset, end));
            }
        }
    }
    Ok(())
}

/// Maps a known NAND part to exactly one SPL variant.
#[must_use]
pub const fn spl_role(nand: Nand) -> Role {
    match nand {
        Nand::Hynix => Role::SplHynix,
        Nand::Toshiba => Role::SplToshiba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Cancellation,
        assets::{Cache, VerifiedAssets},
        device::TargetInfo,
        simulation,
    };
    fn verified() -> Result<(VerifiedAssets, tempfile::TempDir), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let manifest = crate::releases::select(
            crate::releases::Channel::Simulation,
            crate::profile::Profile::Stock.simulation_release(),
        )?;
        let assets = manifest
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
            .collect::<Result<Vec<_>, _>>()?;
        let verified = VerifiedAssets::verify(manifest, assets, &Cancellation::default())?;
        Ok((verified, dir))
    }
    fn target(nand: Nand) -> Result<IdentifiedTarget, Error> {
        IdentifiedTarget::identify(TargetInfo::fixture(nand))
    }
    #[test]
    fn plan_uses_exactly_the_target_spl_variant() -> Result<(), Error> {
        let (assets, _dir) = verified()?;
        for (nand, expected) in [
            (Nand::Hynix, Role::SplHynix),
            (Nand::Toshiba, Role::SplToshiba),
        ] {
            let plan = plan(&assets, &target(nand)?)?;
            let sources: Vec<_> = plan.steps().iter().filter_map(|step| step.source).collect();
            assert!(sources.contains(&expected));
            let other = match nand {
                Nand::Hynix => Role::SplToshiba,
                Nand::Toshiba => Role::SplHynix,
            };
            assert!(
                !sources.contains(&other),
                "{nand:?} used the other SPL variant"
            );
            assert_eq!(plan.target().nand(), nand);
            assert_eq!(plan.steps().len(), if nand == Nand::Hynix { 10 } else { 8 });
        }
        Ok(())
    }
    #[test]
    fn measured_hynix_backup_has_exact_source_and_readback_without_authorization()
    -> Result<(), Error> {
        let (assets, _dir) = verified()?;
        let plan = plan(&assets, &target(Nand::Hynix)?)?;
        let uboot = assets.asset(Role::UbootNand)?;
        let backup: Vec<_> = plan
            .steps()
            .iter()
            .filter(|step| {
                step.placement
                    == Placement::Nand {
                        offset: UBOOT_BACKUP_OFFSET,
                    }
            })
            .collect();
        assert_eq!(backup.len(), 2);
        assert_eq!(backup[0].kind, PlanKind::WriteUboot);
        assert_eq!(backup[1].kind, PlanKind::VerifyBootloader);
        for step in backup {
            assert_eq!(step.source, Some(Role::UbootNand));
            assert_eq!(step.length, Some(uboot.size()));
            assert_eq!(step.sha256.as_deref(), Some(uboot.sha256()));
            assert_eq!(step.verification, readback(UBOOT_BACKUP_OFFSET, uboot));
        }
        assert!(
            !plan
                .gates()
                .iter()
                .any(|gate| gate.id == GateId::UbootBackupAmbiguity)
        );
        assert!(!plan.is_executable());
        assert!(matches!(
            plan.authorize_execution(),
            Err(Error::PhysicalBlocked)
        ));
        let unmeasured = super::plan(&assets, &target(Nand::Toshiba)?)?;
        assert!(!unmeasured.steps().iter().any(|step| {
            step.placement
                == Placement::Nand {
                    offset: UBOOT_BACKUP_OFFSET,
                }
        }));
        assert!(
            unmeasured
                .gates()
                .iter()
                .any(|gate| gate.id == GateId::UbootBackupAmbiguity)
        );
        Ok(())
    }

    #[test]
    fn wrong_spl_variant_fails_plan_validation() -> Result<(), Error> {
        let (assets, _dir) = verified()?;
        let mut plan = plan(&assets, &target(Nand::Hynix)?)?;
        let wrong = plan
            .steps
            .iter_mut()
            .find(|step| step.kind == PlanKind::WriteSpl)
            .ok_or(Error::State)?;
        wrong.source = Some(Role::SplToshiba);
        assert!(matches!(validate(&plan), Err(Error::Device)));
        Ok(())
    }
    #[test]
    fn missing_exact_length_or_digest_fails_validation() -> Result<(), Error> {
        let (assets, _dir) = verified()?;
        let mut plan = plan(&assets, &target(Nand::Hynix)?)?;
        let step = plan
            .steps
            .iter_mut()
            .find(|step| step.kind == PlanKind::WriteUboot)
            .ok_or(Error::State)?;
        step.sha256 = None;
        assert!(matches!(validate(&plan), Err(Error::Manifest(_))));
        Ok(())
    }
    #[test]
    fn overlapping_or_misaligned_layout_fails() {
        let step = NandStep {
            kind: PlanKind::WriteSpl,
            source: Some(Role::SplHynix),
            placement: Placement::Nand { offset: 0x10 },
            length: Some(16),
            sha256: Some("0".repeat(64)),
            prerequisites: Vec::new(),
            verification: Verification::TransportAck,
            stage: Stage::Flash,
        };
        assert!(matches!(
            check_layout(std::slice::from_ref(&step)),
            Err(Error::Manifest(_))
        ));
        let first = NandStep {
            placement: Placement::Nand { offset: 0 },
            length: Some(0x1000),
            ..step.clone()
        };
        let second = NandStep {
            placement: Placement::Nand { offset: 0x800 },
            length: Some(0x1000),
            ..step
        };
        assert!(matches!(
            check_layout(&[first, second]),
            Err(Error::Manifest(_))
        ));
    }
    #[test]
    fn boot_writes_cannot_cross_into_the_rootfs_region() {
        let step = NandStep {
            kind: PlanKind::WriteUboot,
            source: Some(Role::UbootNand),
            placement: Placement::Nand {
                offset: UBOOT_OFFSET,
            },
            length: Some(BOOT_REGION_BYTES),
            sha256: Some("0".repeat(64)),
            prerequisites: Vec::new(),
            verification: Verification::TransportAck,
            stage: Stage::Flash,
        };
        assert!(matches!(check_layout(&[step]), Err(Error::Manifest(_))));
    }
    #[test]
    fn plan_is_never_executable() -> Result<(), Error> {
        let (assets, _dir) = verified()?;
        let plan = plan(&assets, &target(Nand::Toshiba)?)?;
        assert!(!plan.is_executable());
        assert!(matches!(
            plan.authorize_execution(),
            Err(Error::PhysicalBlocked)
        ));
        assert!(
            plan.gates()
                .iter()
                .any(|gate| gate.id == GateId::UnapprovedManifest)
        );
        assert!(
            plan.gates()
                .iter()
                .any(|gate| gate.id == GateId::UbootBackupAmbiguity)
        );
        Ok(())
    }
    #[test]
    fn unknown_target_never_reaches_planning() {
        let mut info = TargetInfo::fixture(Nand::Hynix);
        info.nand_part = "unknown".into();
        assert!(IdentifiedTarget::identify(info).is_err());
    }
    #[test]
    fn spl_role_mapping_is_closed() {
        assert_eq!(spl_role(Nand::Hynix), Role::SplHynix);
        assert_eq!(spl_role(Nand::Toshiba), Role::SplToshiba);
    }
}
