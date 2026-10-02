//! Restricted pinned-SPL diagnostics for the sacrificial Batch 3 unit.
//!
//! This is not release approval or production `NandPlan` authorization. The
//! recovery caller must collect every observation locally, in the authenticated
//! RAM session, immediately before a diagnostic mutation. No manifest/UI address,
//! command, image choice or arbitrary target can enter this policy.
use crate::{
    Cancellation, Error,
    boot0::Boot0Decoder,
    recovery::{BootReadback, BootRegion, Inventory, ReadInterpretation},
    tool::{ToolOutput, ToolRequest, ToolRunner},
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
    time::Duration,
};

pub mod journal;
pub mod release;
pub mod uboot;

/// Closed original and exact locked-release SPL diagnostics; no addresses or paths.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    ErasePrimary,
    RestorePrimary,
    EraseBackup,
    RestoreBackup,
    ProgramReleasePrimary,
    EraseBackupForReleasePrimary,
    RestoreBackupForReleasePrimary,
    ProgramReleaseUbootBackup,
    EraseOriginalUbootPrimary,
    RestoreOriginalUbootPrimary,
    ProgramReleaseUbootPrimary,
    EraseReleaseUbootBackup,
    RestoreReleaseUbootBackup,
}
impl Operation {
    /// Closed diagnostic target; no user-supplied addresses.
    #[must_use]
    pub const fn target(self) -> BootRegion {
        match self {
            Self::EraseOriginalUbootPrimary
            | Self::RestoreOriginalUbootPrimary
            | Self::ProgramReleaseUbootPrimary => BootRegion::UBoot,
            Self::ProgramReleaseUbootBackup
            | Self::EraseReleaseUbootBackup
            | Self::RestoreReleaseUbootBackup => BootRegion::FourthBootBlock,
            Self::ErasePrimary | Self::RestorePrimary | Self::ProgramReleasePrimary => {
                BootRegion::SplPrimary
            }
            Self::EraseBackup
            | Self::RestoreBackup
            | Self::EraseBackupForReleasePrimary
            | Self::RestoreBackupForReleasePrimary => BootRegion::SplBackup,
        }
    }
    /// The opposite SPL block must retain the operation's exact protected program.
    #[must_use]
    pub const fn protected_region(self) -> BootRegion {
        match self {
            Self::EraseOriginalUbootPrimary
            | Self::RestoreOriginalUbootPrimary
            | Self::ProgramReleaseUbootPrimary => BootRegion::FourthBootBlock,
            Self::ProgramReleaseUbootBackup
            | Self::EraseReleaseUbootBackup
            | Self::RestoreReleaseUbootBackup => BootRegion::UBoot,
            Self::ErasePrimary | Self::RestorePrimary | Self::ProgramReleasePrimary => {
                BootRegion::SplBackup
            }
            Self::EraseBackup
            | Self::RestoreBackup
            | Self::EraseBackupForReleasePrimary
            | Self::RestoreBackupForReleasePrimary => BootRegion::SplPrimary,
        }
    }
    #[must_use]
    pub const fn is_release_trial(self) -> bool {
        matches!(
            self,
            Self::ProgramReleasePrimary
                | Self::EraseBackupForReleasePrimary
                | Self::RestoreBackupForReleasePrimary
        )
    }
    #[must_use]
    pub const fn is_uboot_trial(self) -> bool {
        matches!(
            self,
            Self::ProgramReleaseUbootBackup
                | Self::EraseOriginalUbootPrimary
                | Self::RestoreOriginalUbootPrimary
                | Self::ProgramReleaseUbootPrimary
                | Self::EraseReleaseUbootBackup
                | Self::RestoreReleaseUbootBackup
        )
    }
    /// Checks this operation's completion without granting any new authority.
    /// # Errors
    /// Rejects wrong targets, programs, ECC interpretation or incomplete erasure.
    pub fn verify(self, report: &BootReadback) -> Result<(), Error> {
        match self {
            Self::ErasePrimary | Self::EraseBackup | Self::EraseBackupForReleasePrimary => {
                verify_erased(report, self.target())
            }
            Self::RestorePrimary | Self::RestoreBackup | Self::RestoreBackupForReleasePrimary => {
                verify_spl(report, self.target())
            }
            Self::ProgramReleasePrimary => release::verify_primary(report),
            Self::EraseOriginalUbootPrimary | Self::EraseReleaseUbootBackup => {
                uboot::verify_erased(report, self.target())
            }
            Self::RestoreOriginalUbootPrimary => {
                uboot::verify_program(report, self.target(), uboot::Program::Original)
            }
            Self::ProgramReleaseUbootBackup
            | Self::ProgramReleaseUbootPrimary
            | Self::RestoreReleaseUbootBackup => {
                uboot::verify_program(report, self.target(), uboot::Program::Release)
            }
        }
    }
}

pub const SID: [u8; 16] = [
    0x17, 0x42, 0x25, 0x16, 0x58, 0x38, 0x30, 0x50, 0x30, 0x30, 0x33, 0x31, 0xc0, 0x88, 0x02, 0x0e,
];
pub const ORIGINAL_SPL: &str = "a2640b992973e0ff042ea37de543d89bf9f855d1b64e5f485657267980d5f3cb";
pub const ORIGINAL_UBOOT: &str = "c76993ede3ceab2ba56e37b027c43896f4e4a79058cf4197aa7d1a7118b10224";
pub const RESTORATION_IMAGE: &str =
    "d6ac65c582c19ff609de3c02b1ff77938127ce05166e13f4e3d2b7b4bb4d3e03";
pub const IMAGE_BYTES: usize = 4_620_288;
const BLOCK: u64 = 4_194_304;
const PAGE: usize = 16_384;
const OOB: usize = 1_664;

#[derive(Clone, Copy)]
enum Program {
    Original,
    LockedHynix,
}
impl Program {
    const fn digest(self) -> &'static str {
        match self {
            Self::Original => ORIGINAL_SPL,
            Self::LockedHynix => release::PROGRAM,
        }
    }
    const fn checksum(self) -> u32 {
        match self {
            Self::Original => 0x5605_fb91,
            Self::LockedHynix => 0x5305_ee97,
        }
    }
}

/// Fresh local recovery observations; the protected SPL is opposite the trial target.
pub struct Observations<'a> {
    pub sid: &'a [u8],
    pub inventory: &'a Inventory,
    pub protected_spl: &'a BootReadback,
    pub uboot: &'a BootReadback,
    pub mounts: &'a str,
    pub ubi_devices: &'a [String],
}

/// Proof of this diagnostic's local preconditions, not physical release approval.
#[derive(Debug)]
pub struct Preflight {
    operation: Operation,
}
impl Preflight {
    /// Checks the known unit, RAM-only state and the intact opposite SPL/U-Boot chain.
    /// # Errors
    /// Rejects unknown hardware, changed geometry/digests, ECC failures or cancellation.
    pub fn validate(
        operation: Operation,
        observed: &Observations<'_>,
        cancel: &Cancellation,
    ) -> Result<Self, Error> {
        cancel.check()?;
        if observed.sid != SID {
            return Err(Error::Device);
        }
        ram_only(observed.mounts, observed.ubi_devices)?;
        hardware(observed.inventory)?;
        if matches!(
            operation,
            Operation::EraseBackupForReleasePrimary | Operation::RestoreBackupForReleasePrimary
        ) {
            release::verify_primary(observed.protected_spl)?;
        } else {
            verify_spl(observed.protected_spl, operation.protected_region())?;
        }
        let uboot = observed.uboot;
        common_readback(uboot)?;
        if uboot.region != BootRegion::UBoot
            || uboot.interpretation != ReadInterpretation::KernelCorrected
            || uboot.data_sha256 != ORIGINAL_UBOOT
            || !uboot.spl_copies.is_empty()
        {
            return Err(Error::Device);
        }
        cancel.check()?;
        Ok(Self { operation })
    }

    /// Erases the validated SPL block and verifies its entire raw data/OOB readback.
    /// The caller must durably record dispatch first. No retry occurs.
    /// # Errors
    /// Returns erase failure, cancellation or non-erased/invalid readback.
    pub fn erase_and_verify(
        &self,
        runner: &impl ToolRunner,
        mut readback: impl FnMut() -> Result<BootReadback, Error>,
        cancel: &Cancellation,
    ) -> Result<BootReadback, Error> {
        self.erase(runner, cancel)?;
        let report = readback()?;
        verify_erased(&report, self.target())?;
        cancel.check()?;
        Ok(report)
    }

    /// Erases exactly one validated SPL block; no bad-block skipping or retry.
    /// The caller must journal intent before calling and verify the raw result.
    /// # Errors
    /// Returns cancellation, timeout, process failure or I/O errors.
    pub fn erase(
        &self,
        runner: &impl ToolRunner,
        cancel: &Cancellation,
    ) -> Result<ToolOutput, Error> {
        cancel.check()?;
        runner.run(&erase_request(self.target_path()?), cancel)
    }
    /// The fixed partition validated for this trial.
    #[must_use]
    pub const fn target(&self) -> BootRegion {
        self.operation.target()
    }

    const fn target_path(&self) -> Result<&'static str, Error> {
        match self.operation.target() {
            BootRegion::SplPrimary => Ok("/dev/mtd0"),
            BootRegion::SplBackup => Ok("/dev/mtd1"),
            _ => Err(Error::State),
        }
    }
}

fn hardware(inventory: &Inventory) -> Result<(), Error> {
    if inventory.model != "NextThing PocketC.H.I.P."
        || !inventory
            .compatible
            .iter()
            .any(|s| s == "nextthing,pocketchip")
        || !inventory
            .compatible
            .iter()
            .any(|s| s == "allwinner,sun5i-r8")
        || inventory.kernel != "6.12.107+deb13-chip"
        || inventory.command_line != "console=ttyS0,115200 panic=0 rdinit=/init"
        || inventory.memory_reg != [0x40, 0, 0, 0, 0x1e, 0, 0xf0, 0]
        || inventory.mtd.len() != 5
        || !inventory
            .boot_log
            .contains("nand: device found, Manufacturer ID: 0xad, Chip ID: 0xde")
        || !inventory.boot_log.contains("nand: Hynix H27UCG8T2ETR-BC")
        || !inventory
            .boot_log
            .contains("nand: 8192 MiB, MLC, erase size: 4096 KiB, page size: 16384, OOB size: 1664")
    {
        return Err(Error::Device);
    }
    for (index, mtd) in inventory.mtd.iter().enumerate() {
        let boot = index < 4;
        if usize::from(mtd.index) != index
            || mtd.offset != u64::try_from(index).map_err(|_| Error::Device)? * BLOCK
            || mtd.page_size != PAGE as u64
            || mtd.oob_size != OOB as u64
            || mtd.ecc_strength != 56
            || mtd.ecc_step != 1024
            || mtd.ecc_failures != 0
            || (boot
                && (mtd.size != BLOCK
                    || mtd.erase_size != BLOCK
                    || mtd.bad_blocks != 0
                    || mtd.bbt_blocks != 0))
            || (!boot
                && (mtd.size != 4_286_578_688
                    || mtd.erase_size != 2_097_152
                    || mtd.bad_blocks != 61
                    || mtd.bbt_blocks != 4))
        {
            return Err(Error::Device);
        }
    }
    Ok(())
}

/// Requires RAM/kernel pseudo mounts and no attached UBI devices.
/// # Errors
/// Rejects malformed mount records, disk-backed filesystems or attached UBI.
pub fn ram_only(mounts: &str, ubi_devices: &[String]) -> Result<(), Error> {
    // The class-wide version attribute is harmless; any other entry is
    // unexpected or an attached UBI device/volume and must fail closed.
    if mounts.is_empty() || ubi_devices.iter().any(|name| name != "version") {
        return Err(Error::State);
    }
    for line in mounts.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 6
            || !matches!(
                fields[2],
                "rootfs"
                    | "ramfs"
                    | "tmpfs"
                    | "proc"
                    | "sysfs"
                    | "devtmpfs"
                    | "devpts"
                    | "configfs"
                    | "debugfs"
                    | "securityfs"
                    | "tracefs"
                    | "cgroup"
                    | "cgroup2"
                    | "bpf"
            )
            || fields[0].starts_with("/dev/mtd")
            || fields[0].starts_with("/dev/ubi")
        {
            return Err(Error::State);
        }
    }
    Ok(())
}

fn common_readback(report: &BootReadback) -> Result<(), Error> {
    if report.data_bytes != BLOCK
        || report.oob_bytes != 425_984
        || report.page_marker_bytes.as_slice() != [[0xff; 2]; 256]
        || report.corrected_bits_after < report.corrected_bits_before
        || report.ecc_failures_before != 0
        || report.ecc_failures_after != 0
    {
        return Err(Error::Device);
    }
    Ok(())
}

/// Verifies every corrected copy against the exact original installed SPL.
/// # Errors
/// Rejects missing copies, wrong interpretation/digest/header, ECC errors or wrong region.
pub fn verify_spl(report: &BootReadback, region: BootRegion) -> Result<(), Error> {
    verify_program(report, region, Program::Original)
}
fn verify_program(
    report: &BootReadback,
    region: BootRegion,
    program: Program,
) -> Result<(), Error> {
    common_readback(report)?;
    if !matches!(region, BootRegion::SplPrimary | BootRegion::SplBackup)
        || report.region != region
        || report.interpretation != ReadInterpretation::Boot0Corrected
        || report.spl_copies.len() != 4
    {
        return Err(Error::Device);
    }
    for (index, copy) in report.spl_copies.iter().enumerate() {
        if usize::from(copy.copy) != index
            || copy.data_sha256 != program.digest()
            || copy.corrected_bits.iter().any(|bits| *bits > 64)
            || copy.header_bytes.len() != 32
            || &copy.header_bytes[4..12] != b"eGON.BT0"
            || &copy.header_bytes[20..24] != b"SPL\x02"
            || copy.checksum != program.checksum()
        {
            return Err(Error::Device);
        }
    }
    Ok(())
}

/// Verifies full raw data/OOB erasure, not command acceptance alone.
/// # Errors
/// Rejects a wrong region, non-erased byte/marker/digest or ECC error.
pub fn verify_erased_primary(report: &BootReadback) -> Result<(), Error> {
    verify_erased(report, BootRegion::SplPrimary)
}

/// Verifies complete raw erasure of one closed SPL block.
/// # Errors
/// Rejects non-SPL regions, wrong partition, un-erased data/OOB or ECC failures.
pub fn verify_erased(report: &BootReadback, region: BootRegion) -> Result<(), Error> {
    if !matches!(region, BootRegion::SplPrimary | BootRegion::SplBackup) {
        return Err(Error::Device);
    }
    verify_erased_block(report, region)
}
fn verify_erased_block(report: &BootReadback, region: BootRegion) -> Result<(), Error> {
    common_readback(report)?;
    if !matches!(
        region,
        BootRegion::SplPrimary
            | BootRegion::SplBackup
            | BootRegion::UBoot
            | BootRegion::FourthBootBlock
    ) || report.region != region
        || report.interpretation != ReadInterpretation::Raw
        || !report.spl_copies.is_empty()
        || report.first_data_bytes != [0xff; 32]
        || report.erased_data_pages != (0..256).collect::<Vec<_>>()
        || report.erased_oob_pages != (0..256).collect::<Vec<_>>()
        || report.data_sha256 != erased_digest(BLOCK)
        || report.oob_sha256 != erased_digest(425_984)
        || report.interleaved_sha256 != erased_digest(IMAGE_BYTES as u64)
    {
        return Err(Error::Device);
    }
    Ok(())
}

fn erased_digest(mut length: u64) -> String {
    let bytes = [0xff; 1024];
    let mut hash = Sha256::new();
    while length > 0 {
        hash.update(bytes);
        length -= 1024; // All reviewed lengths are exact multiples of 1024.
    }
    format!("{:x}", hash.finalize())
}

fn erase_request(target: &str) -> ToolRequest {
    ToolRequest::new(
        "/usr/sbin/flash_erase",
        ["--noskipbad", "--quiet", target, "0", "1"],
    )
    .timeout(Duration::from_secs(10))
}

/// Private snapshot of the exact clean original-SPL encoding, retained through write.
#[derive(Debug)]
pub struct OriginalSplFile {
    file: tempfile::NamedTempFile,
    _directory: tempfile::TempDir,
}
impl OriginalSplFile {
    /// Validates the pinned restoration encoding before creating a private snapshot.
    /// # Errors
    /// Rejects unsafe paths, wrong size/hash, bad markers, ECC/headers or cancellation.
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<Self, Error> {
        let (file, directory) = snapshot(path, validate_image, cancel)?;
        Ok(Self {
            file,
            _directory: directory,
        })
    }

    /// Revalidates the retained open snapshot before a destructive operation.
    /// # Errors
    /// Returns cancellation, changed bytes or I/O failure.
    pub fn revalidate(&mut self, cancel: &Cancellation) -> Result<(), Error> {
        revalidate_snapshot(&mut self.file, validate_image, cancel)
    }

    /// Rechecks the open snapshot and writes only the validated fixed SPL partition.
    /// The caller must journal intent and verify corrected readback before success.
    /// # Errors
    /// Returns hash/cancellation/process/timeout/I/O failure. No automatic retry occurs.
    pub fn restore(
        &mut self,
        preflight: &Preflight,
        runner: &impl ToolRunner,
        cancel: &Cancellation,
    ) -> Result<ToolOutput, Error> {
        if !matches!(
            preflight.operation,
            Operation::RestorePrimary
                | Operation::RestoreBackup
                | Operation::RestoreBackupForReleasePrimary
        ) {
            return Err(Error::State);
        }
        self.revalidate(cancel)?;
        let path = self.file.path().to_str().ok_or(Error::UnsafePath)?;
        runner.run(
            &ToolRequest::new(
                "/usr/sbin/nandwrite",
                [
                    "--noecc",
                    "--oob",
                    "--noskipbad",
                    "--quiet",
                    preflight.target_path()?,
                    path,
                ],
            )
            .timeout(Duration::from_secs(10)),
            cancel,
        )
    }
}

fn snapshot(
    path: &Path,
    validate: fn(&[u8], &Cancellation) -> Result<(), Error>,
    cancel: &Cancellation,
) -> Result<(tempfile::NamedTempFile, tempfile::TempDir), Error> {
    cancel.check()?;
    crate::assets::regular_components(path)?;
    let file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() != IMAGE_BYTES as u64 {
        return Err(Error::Length);
    }
    let mut bytes = Vec::new();
    file.take(IMAGE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    validate(&bytes, cancel)?;
    let directory = crate::assets::temporary_directory()?;
    let mut file = tempfile::NamedTempFile::new_in(directory.path())?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    cancel.check()?;
    Ok((file, directory))
}

fn revalidate_snapshot(
    file: &mut tempfile::NamedTempFile,
    validate: fn(&[u8], &Cancellation) -> Result<(), Error>,
    cancel: &Cancellation,
) -> Result<(), Error> {
    cancel.check()?;
    file.as_file_mut().rewind()?;
    let mut bytes = Vec::new();
    file.as_file_mut()
        .take(IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    validate(&bytes, cancel)
}

fn validate_image(bytes: &[u8], cancel: &Cancellation) -> Result<(), Error> {
    cancel.check()?;
    if bytes.len() != IMAGE_BYTES || format!("{:x}", Sha256::digest(bytes)) != RESTORATION_IMAGE {
        return Err(Error::Hash);
    }
    let mut data = Vec::with_capacity(4_194_304);
    for page in bytes.as_chunks::<{ PAGE + OOB }>().0 {
        if page[PAGE..PAGE + 2] != [0xff; 2] {
            return Err(Error::Device);
        }
        data.extend_from_slice(&page[..PAGE]);
    }
    let copies = Boot0Decoder::new()?.reports(&data, cancel)?;
    if copies
        .iter()
        .any(|c| c.data_sha256 != ORIGINAL_SPL || c.corrected_bits != [0; 16])
    {
        return Err(Error::Hash);
    }
    cancel.check()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ScriptedToolRunner;
    use serde_json::Value;

    const MOUNTS: &str = "rootfs / rootfs rw 0 0\nproc /proc proc rw 0 0\nsys /sys sysfs rw 0 0\n";

    fn measured() -> (Inventory, BootReadback, BootReadback) {
        let inventory: Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/recovery-inventory-7.json"
        ))
        .unwrap();
        let spl: Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/recovery-spl-bch64-readbacks-7.json"
        ))
        .unwrap();
        let uboot: Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/recovery-uboot-corrected-readback-7.json"
        ))
        .unwrap();
        (
            serde_json::from_value(inventory["response"]["Inventory"].clone()).unwrap(),
            serde_json::from_value(spl["records"][1]["response"].clone()).unwrap(),
            serde_json::from_value(uboot["response"]["BootReadback"].clone()).unwrap(),
        )
    }

    fn prepare(
        inventory: &Inventory,
        backup: &BootReadback,
        uboot: &BootReadback,
    ) -> Result<Preflight, Error> {
        Preflight::validate(
            Operation::ErasePrimary,
            &Observations {
                sid: &SID,
                inventory,
                protected_spl: backup,
                uboot,
                mounts: MOUNTS,
                ubi_devices: &[],
            },
            &Cancellation::default(),
        )
    }

    #[test]
    fn decoded_locked_release_does_not_verify_as_original_restoration() {
        let evidence: Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/locked-release-hynix-spl-native-review.json"
        ))
        .unwrap();
        let (_, mut report, _) = measured();
        report.region = BootRegion::SplPrimary;
        report.spl_copies = serde_json::from_value(evidence["copies"].clone()).unwrap();
        assert!(
            report
                .spl_copies
                .iter()
                .all(|copy| copy.corrected_bits.iter().all(|bits| *bits == 0))
        );
        assert!(verify_spl(&report, BootRegion::SplPrimary).is_err());
    }

    #[test]
    fn measured_backup_erasure_after_host_loss_is_not_primary_verification() {
        let evidence: Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/recovery-backup-after-host-interruption-10.json"
        ))
        .unwrap();
        let report: BootReadback =
            serde_json::from_value(evidence["response"]["BootReadback"].clone()).unwrap();
        verify_erased(&report, BootRegion::SplBackup).unwrap();
        assert!(verify_erased_primary(&report).is_err());
        assert!(verify_erased(&report, BootRegion::FourthBootBlock).is_err());
    }

    #[test]
    fn erase_only_preflight_cannot_authorize_a_restoration_write() {
        let (inventory, backup, uboot) = measured();
        let checked = prepare(&inventory, &backup, &uboot).unwrap();
        let directory = crate::assets::temporary_directory().unwrap();
        let file = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        let mut restoration = OriginalSplFile {
            file,
            _directory: directory,
        };
        let runner = ScriptedToolRunner::new();
        assert!(matches!(
            restoration.restore(&checked, &runner, &Cancellation::default()),
            Err(Error::State)
        ));
        assert_eq!(runner.calls().len(), 0);
    }

    #[test]
    fn backup_trial_requires_actual_restored_primary_and_scopes_only_mtd1() {
        let (inventory, backup, uboot) = measured();
        let evidence: Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/recovery-primary-restoration-trial-9.json"
        ))
        .unwrap();
        let primary: BootReadback =
            serde_json::from_value(evidence["response"]["SplTrialVerified"]["readback"].clone())
                .unwrap();
        for operation in [Operation::EraseBackup, Operation::RestoreBackup] {
            let observed = Observations {
                sid: &SID,
                inventory: &inventory,
                protected_spl: &primary,
                uboot: &uboot,
                mounts: MOUNTS,
                ubi_devices: &[],
            };
            let checked =
                Preflight::validate(operation, &observed, &Cancellation::default()).unwrap();
            assert_eq!(checked.target(), BootRegion::SplBackup);
            let runner = ScriptedToolRunner::new();
            runner.expect_error(erase_request("/dev/mtd1"), Error::Process(Some(1)));
            assert!(matches!(
                checked.erase(&runner, &Cancellation::default()),
                Err(Error::Process(Some(1)))
            ));
            assert_eq!(
                runner.calls()[0].args,
                ["--noskipbad", "--quiet", "/dev/mtd1", "0", "1"]
            );
            let wrong = Observations {
                protected_spl: &backup,
                ..observed
            };
            assert!(Preflight::validate(operation, &wrong, &Cancellation::default()).is_err());
        }
        for operation in [Operation::ErasePrimary, Operation::RestorePrimary] {
            let wrong = Observations {
                sid: &SID,
                inventory: &inventory,
                protected_spl: &primary,
                uboot: &uboot,
                mounts: MOUNTS,
                ubi_devices: &[],
            };
            assert!(Preflight::validate(operation, &wrong, &Cancellation::default()).is_err());
        }
    }

    #[test]
    fn erase_failure_never_reads_or_retries_and_invalid_readback_never_succeeds() {
        let (inventory, backup, uboot) = measured();
        let checked = prepare(&inventory, &backup, &uboot).unwrap();
        let runner = ScriptedToolRunner::new();
        runner.expect_error(erase_request("/dev/mtd0"), Error::Process(Some(1)));
        let mut reads = 0;
        let result = checked.erase_and_verify(
            &runner,
            || {
                reads += 1;
                Err(Error::State)
            },
            &Cancellation::default(),
        );
        assert!(matches!(result, Err(Error::Process(Some(1)))));
        assert_eq!(reads, 0);
        assert_eq!(runner.calls().len(), 1);
        let runner = ScriptedToolRunner::new();
        runner.expect_ok(
            erase_request("/dev/mtd0"),
            ToolOutput {
                exit: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let result = checked.erase_and_verify(
            &runner,
            || {
                reads += 1;
                Err(Error::Device)
            },
            &Cancellation::default(),
        );
        assert!(matches!(result, Err(Error::Device)));
        assert_eq!(reads, 1);
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn measured_fixture_allows_only_fixed_primary_erase_and_propagates_failure() {
        let (inventory, backup, uboot) = measured();
        let ready = prepare(&inventory, &backup, &uboot).unwrap();
        let runner = ScriptedToolRunner::new();
        runner.expect_error(erase_request("/dev/mtd0"), Error::Process(Some(1)));
        assert!(matches!(
            ready.erase(&runner, &Cancellation::default()),
            Err(Error::Process(Some(1)))
        ));
        assert_eq!(runner.calls().len(), 1);
        let call = &runner.calls()[0];
        assert_eq!(call.program, Path::new("/usr/sbin/flash_erase"));
        assert_eq!(call.args, ["--noskipbad", "--quiet", "/dev/mtd0", "0", "1"]);
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            ready.erase(&runner, &cancel),
            Err(Error::Cancelled)
        ));
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn wrong_identity_geometry_bad_blocks_or_boot_chain_never_authorizes_trial() {
        let (inventory, backup, uboot) = measured();
        let other_sid = [1; 16];
        assert!(
            Preflight::validate(
                Operation::ErasePrimary,
                &Observations {
                    sid: &other_sid,
                    inventory: &inventory,
                    protected_spl: &backup,
                    uboot: &uboot,
                    mounts: MOUNTS,
                    ubi_devices: &[],
                },
                &Cancellation::default()
            )
            .is_err()
        );
        let mutations: [fn(&mut Inventory); 8] = [
            |i| i.model = "Other board".into(),
            |i| i.boot_log.clear(),
            |i| i.mtd[0].offset = BLOCK,
            |i| i.mtd[1].bad_blocks = 1,
            |i| i.mtd[0].page_size = 4096,
            |i| i.mtd[2].ecc_failures = 1,
            |i| i.mtd[4].bad_blocks = 62,
            |i| {
                let mut alias: crate::recovery::MtdInfo =
                    serde_json::from_value(serde_json::to_value(&i.mtd[4]).unwrap()).unwrap();
                alias.index = 5;
                alias.name = "BBM.probe".into();
                alias.offset = 947_912_704;
                alias.size = BLOCK;
                alias.erase_size = BLOCK;
                i.mtd.push(alias);
            },
        ];
        for mutate in mutations {
            let (mut inventory, backup, uboot) = measured();
            mutate(&mut inventory);
            assert!(prepare(&inventory, &backup, &uboot).is_err());
        }
        let mutations: [fn(&mut BootReadback); 5] = [
            |r| {
                let _ = r.spl_copies.pop();
            },
            |r| r.spl_copies[0].data_sha256 = "b".repeat(64),
            |r| r.spl_copies[1].copy = 0,
            |r| r.interpretation = ReadInterpretation::Raw,
            |r| r.page_marker_bytes[0] = [0; 2],
        ];
        for mutate in mutations {
            let (inventory, mut backup, uboot) = measured();
            mutate(&mut backup);
            assert!(prepare(&inventory, &backup, &uboot).is_err());
        }
        let (inventory, backup, mut uboot) = measured();
        uboot.data_sha256 = "c".repeat(64);
        assert!(prepare(&inventory, &backup, &uboot).is_err());
    }

    #[test]
    fn mounted_nand_and_attached_or_unknown_ubi_state_fail_closed() {
        ram_only(MOUNTS, &[]).unwrap();
        ram_only(MOUNTS, &["version".into()]).unwrap();
        for mounts in [
            "",
            "malformed",
            "ubi0:rootfs / ubifs rw 0 0",
            "/dev/mtd0 /mnt tmpfs rw 0 0",
            "/dev/sda / ext4 rw 0 0",
        ] {
            assert!(ram_only(mounts, &[]).is_err());
        }
        for name in ["ubi0", "ubi0_0", "unexpected"] {
            assert!(ram_only(MOUNTS, &[name.into()]).is_err());
        }
    }

    #[test]
    fn actual_erased_capture_semantics_require_all_bytes_and_correct_partition() {
        let raw: Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/recovery-primary-erase-trial-8.json"
        ))
        .unwrap();
        let mut report: BootReadback =
            serde_json::from_value(raw["response"]["SplTrialVerified"]["readback"].clone())
                .unwrap();
        report.region = BootRegion::FourthBootBlock;
        assert!(verify_erased_primary(&report).is_err());
        report.region = BootRegion::SplPrimary;
        verify_erased_primary(&report).unwrap();
        report.erased_data_pages.pop();
        assert!(verify_erased_primary(&report).is_err());
        report.erased_data_pages.push(255);
        report.data_sha256 = "d".repeat(64);
        assert!(verify_erased_primary(&report).is_err());
    }

    #[test]
    fn restoration_snapshot_rejects_corrupted_or_cancelled_input_before_mutation() {
        assert!(matches!(
            validate_image(&vec![0xff; IMAGE_BYTES], &Cancellation::default()),
            Err(Error::Hash)
        ));
        assert!(matches!(
            validate_image(&[], &Cancellation::default()),
            Err(Error::Hash)
        ));
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            OriginalSplFile::open(Path::new("/missing"), &cancel),
            Err(Error::Cancelled)
        ));
    }
}
