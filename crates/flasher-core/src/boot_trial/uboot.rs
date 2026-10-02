//! Pinned original/release U-Boot snapshots and closed sacrificial-unit diagnostics.
use super::Operation;
use crate::{
    Cancellation, Error,
    recovery::{BootReadback, BootRegion, Inventory, ReadInterpretation},
    tool::{ToolOutput, ToolRequest, ToolRunner},
};
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

/// Diagnostic provenance, never physical manifest approval.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pins {
    pub bundle_sha256: String,
    pub original_program_sha256: String,
    pub release_program_sha256: String,
    pub manifest_sha256: String,
    pub primary_spl_sha256: String,
    pub backup_spl_sha256: String,
}
impl Pins {
    #[must_use]
    pub fn expected() -> Self {
        Self {
            bundle_sha256: BUNDLE.into(),
            original_program_sha256: super::ORIGINAL_UBOOT.into(),
            release_program_sha256: RELEASE_PROGRAM.into(),
            manifest_sha256: super::release::MANIFEST.into(),
            primary_spl_sha256: super::release::PROGRAM.into(),
            backup_spl_sha256: super::ORIGINAL_SPL.into(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Program {
    Original,
    Release,
}
impl Program {
    const fn digest(self) -> &'static str {
        match self {
            Self::Original => super::ORIGINAL_UBOOT,
            Self::Release => RELEASE_PROGRAM,
        }
    }
}

pub(super) fn verify_program(
    report: &BootReadback,
    region: BootRegion,
    program: Program,
) -> Result<(), Error> {
    super::common_readback(report)?;
    verify_corrections(report)?;
    if !matches!(region, BootRegion::UBoot | BootRegion::FourthBootBlock)
        || report.region != region
        || report.interpretation != ReadInterpretation::KernelCorrected
        || report.data_sha256 != program.digest()
        || !report.spl_copies.is_empty()
        || !report.erased_data_pages.is_empty()
        || !report.erased_oob_pages.is_empty()
        || report.first_data_bytes
            != [
                184, 0, 0, 234, 20, 240, 159, 229, 20, 240, 159, 229, 20, 240, 159, 229, 20, 240,
                159, 229, 20, 240, 159, 229, 20, 240, 159, 229, 20, 240, 159, 229,
            ]
    {
        return Err(Error::Device);
    }
    Ok(())
}
// nanddump reports successful per-page corrections on stderr. Accept only its exact
// measured grammar and ordered offsets. The last OOB read follows nanddump's last
// ECCGETSTATS; the final sysfs counter can include up to one page's corrections.
fn verify_corrections(report: &BootReadback) -> Result<(), Error> {
    let mut total = 0_u64;
    let mut previous = None;
    for line in report.tool_stderr.lines() {
        let (bits, offset) = line
            .strip_prefix("ECC: ")
            .and_then(|line| line.split_once(" corrected bitflip(s) at offset 0x"))
            .ok_or(Error::Device)?;
        let bits: u64 = bits.parse().map_err(|_| Error::Device)?;
        let offset = u64::from_str_radix(offset, 16).map_err(|_| Error::Device)?;
        if bits == 0
            || bits > 2 * 56 * 16
            || offset >= super::BLOCK
            || offset % super::PAGE as u64 != 0
            || previous.is_some_and(|p| offset <= p)
            || line != format!("ECC: {bits} corrected bitflip(s) at offset 0x{offset:08x}")
        {
            return Err(Error::Device);
        }
        total = total.checked_add(bits).ok_or(Error::Device)?;
        previous = Some(offset);
    }
    let delta = report
        .corrected_bits_after
        .checked_sub(report.corrected_bits_before)
        .ok_or(Error::Device)?;
    if delta < total || delta - total > 56 * 16 {
        return Err(Error::Device);
    }
    Ok(())
}

pub(super) fn verify_erased(report: &BootReadback, region: BootRegion) -> Result<(), Error> {
    if !matches!(region, BootRegion::UBoot | BootRegion::FourthBootBlock)
        || !report.tool_stderr.is_empty()
    {
        return Err(Error::Device);
    }
    super::verify_erased_block(report, region)
}

/// Fresh device-local observations for the exact mixed SPL chain measured in session 16.
pub struct Observations<'a> {
    pub sid: &'a [u8],
    pub inventory: &'a Inventory,
    pub mounts: &'a str,
    pub ubi_devices: &'a [String],
    pub spl_primary: &'a BootReadback,
    pub spl_backup: &'a BootReadback,
    pub primary: &'a BootReadback,
    pub backup: &'a BootReadback,
}
/// Closed U-Boot diagnostic prerequisites; no caller-supplied target path.
#[derive(Debug)]
pub struct Preflight {
    operation: Operation,
}
impl Preflight {
    /// Requires the known RAM-only device, both measured SPL programs and an intact opposite U-Boot.
    /// # Errors
    /// Rejects changed hardware, any ECC failure, wrong current programs or an unsupported operation.
    pub fn validate(
        operation: Operation,
        observed: &Observations<'_>,
        cancel: &Cancellation,
    ) -> Result<Self, Error> {
        verify_protected(operation, observed, cancel)?;
        match operation {
            Operation::ProgramReleaseUbootBackup => {
                verify_program(observed.primary, BootRegion::UBoot, Program::Original)?;
                verify_erased(observed.backup, BootRegion::FourthBootBlock)?;
            }
            Operation::EraseOriginalUbootPrimary => {
                verify_program(observed.primary, BootRegion::UBoot, Program::Original)?;
                verify_program(
                    observed.backup,
                    BootRegion::FourthBootBlock,
                    Program::Release,
                )?;
            }
            Operation::RestoreOriginalUbootPrimary => {
                super::common_readback(observed.primary)?;
                if observed.primary.region != BootRegion::UBoot
                    || observed.primary.interpretation != ReadInterpretation::Raw
                    || !observed.primary.tool_stderr.is_empty()
                    || !observed.primary.spl_copies.is_empty()
                {
                    return Err(Error::Device);
                }
            }
            Operation::ProgramReleaseUbootPrimary => {
                verify_erased(observed.primary, BootRegion::UBoot)?;
                verify_program(
                    observed.backup,
                    BootRegion::FourthBootBlock,
                    Program::Release,
                )?;
            }
            Operation::EraseReleaseUbootBackup => {
                verify_program(observed.primary, BootRegion::UBoot, Program::Release)?;
                verify_program(
                    observed.backup,
                    BootRegion::FourthBootBlock,
                    Program::Release,
                )?;
            }
            Operation::RestoreReleaseUbootBackup => {
                verify_program(observed.primary, BootRegion::UBoot, Program::Release)?;
                verify_erased(observed.backup, BootRegion::FourthBootBlock)?;
            }
            _ => return Err(Error::State),
        }
        cancel.check()?;
        Ok(Self { operation })
    }
    /// Erase only the validated fixed slot and check all raw data/OOB pages.
    /// # Errors
    /// Returns erase, readback, cancellation or verification failure without retry.
    pub fn erase_and_verify(
        &self,
        runner: &impl ToolRunner,
        mut readback: impl FnMut() -> Result<BootReadback, Error>,
        cancel: &Cancellation,
    ) -> Result<BootReadback, Error> {
        runner.run(&super::erase_request(self.target_path()?), cancel)?;
        let report = readback()?;
        verify_erased(&report, self.operation.target())?;
        cancel.check()?;
        Ok(report)
    }
    const fn target_path(&self) -> Result<&'static str, Error> {
        match self.operation.target() {
            BootRegion::UBoot => Ok("/dev/mtd2"),
            BootRegion::FourthBootBlock => Ok("/dev/mtd3"),
            _ => Err(Error::State),
        }
    }
}

/// Recheck the untouched boot chain after mutation; this never issues write authority.
/// # Errors
/// Rejects changed identity, mounts, hardware, SPLs or the operation's opposite U-Boot.
pub fn verify_protected(
    operation: Operation,
    observed: &Observations<'_>,
    cancel: &Cancellation,
) -> Result<(), Error> {
    cancel.check()?;
    if !operation.is_uboot_trial() || observed.sid != super::SID {
        return Err(Error::Device);
    }
    super::ram_only(observed.mounts, observed.ubi_devices)?;
    super::hardware(observed.inventory)?;
    super::release::verify_primary(observed.spl_primary)?;
    super::verify_spl(observed.spl_backup, BootRegion::SplBackup)?;
    match operation {
        Operation::ProgramReleaseUbootBackup => {
            verify_program(observed.primary, BootRegion::UBoot, Program::Original)?;
        }
        Operation::EraseOriginalUbootPrimary
        | Operation::RestoreOriginalUbootPrimary
        | Operation::ProgramReleaseUbootPrimary => verify_program(
            observed.backup,
            BootRegion::FourthBootBlock,
            Program::Release,
        )?,
        Operation::EraseReleaseUbootBackup | Operation::RestoreReleaseUbootBackup => {
            verify_program(observed.primary, BootRegion::UBoot, Program::Release)?;
        }
        _ => return Err(Error::State),
    }
    cancel.check()
}

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

    /// Writes a closed validated slot through normal kernel ECC, after erase verification.
    /// # Errors
    /// Rejects erasure-only operations, changed snapshots or process/cancellation failure.
    pub fn program(
        &mut self,
        checked: &Preflight,
        runner: &impl ToolRunner,
        cancel: &Cancellation,
    ) -> Result<ToolOutput, Error> {
        let original = match checked.operation {
            Operation::RestoreOriginalUbootPrimary => true,
            Operation::ProgramReleaseUbootBackup
            | Operation::ProgramReleaseUbootPrimary
            | Operation::RestoreReleaseUbootBackup => false,
            _ => return Err(Error::State),
        };
        self.revalidate(cancel)?;
        let file = if original {
            &self.original
        } else {
            &self.release
        };
        let path = file.path().to_str().ok_or(Error::UnsafePath)?;
        runner.run(
            &ToolRequest::new(
                "/usr/sbin/nandwrite",
                ["--noskipbad", "--quiet", checked.target_path()?, path],
            )
            .timeout(std::time::Duration::from_secs(10)),
            cancel,
        )
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

    // Software fixtures combine session 16 framing with the pinned release program hash.
    // They prove policy checks, never physical release U-Boot acceptance.
    fn chain() -> (Inventory, Vec<BootReadback>) {
        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-boot-chain-after-backup-restoration-16.json"
        ))
        .unwrap();
        let inventory = serde_json::from_value(evidence["inventory"]["Inventory"].clone()).unwrap();
        let reports = serde_json::from_value(evidence["records"].clone()).unwrap();
        (inventory, reports)
    }
    fn observe<'a>(inventory: &'a Inventory, reports: &'a [BootReadback]) -> Observations<'a> {
        Observations {
            sid: &super::super::SID,
            inventory,
            mounts: "rootfs / rootfs rw 0 0\n",
            ubi_devices: &[],
            spl_primary: &reports[0],
            spl_backup: &reports[1],
            primary: &reports[2],
            backup: &reports[3],
        }
    }
    fn release_report(region: BootRegion) -> BootReadback {
        let (_, mut reports) = chain();
        let mut report = reports.remove(2);
        report.region = region;
        report.data_sha256 = RELEASE_PROGRAM.into();
        report
    }

    #[test]
    fn six_closed_operations_require_their_exact_before_state() {
        let cancel = Cancellation::default();
        for operation in [
            Operation::ProgramReleaseUbootBackup,
            Operation::EraseOriginalUbootPrimary,
            Operation::RestoreOriginalUbootPrimary,
            Operation::ProgramReleaseUbootPrimary,
            Operation::EraseReleaseUbootBackup,
            Operation::RestoreReleaseUbootBackup,
        ] {
            let (inventory, mut reports) = chain();
            match operation {
                Operation::ProgramReleaseUbootBackup => {}
                Operation::EraseOriginalUbootPrimary => {
                    reports[3] = release_report(BootRegion::FourthBootBlock);
                }
                Operation::RestoreOriginalUbootPrimary | Operation::ProgramReleaseUbootPrimary => {
                    let mut erased = reports.remove(3);
                    erased.region = BootRegion::UBoot;
                    reports[2] = erased;
                    reports.push(release_report(BootRegion::FourthBootBlock));
                }
                Operation::EraseReleaseUbootBackup => {
                    reports[2] = release_report(BootRegion::UBoot);
                    reports[3] = release_report(BootRegion::FourthBootBlock);
                }
                Operation::RestoreReleaseUbootBackup => {
                    reports[2] = release_report(BootRegion::UBoot);
                }
                _ => unreachable!(),
            }
            let observed = observe(&inventory, &reports);
            let checked = Preflight::validate(operation, &observed, &cancel).unwrap();
            assert_eq!(
                checked.target_path().unwrap(),
                match operation.target() {
                    BootRegion::UBoot => "/dev/mtd2",
                    BootRegion::FourthBootBlock => "/dev/mtd3",
                    _ => unreachable!(),
                }
            );
            let protected = operation.protected_region().index() as usize;
            reports[protected].data_sha256 = "0".repeat(64);
            assert!(
                Preflight::validate(operation, &observe(&inventory, &reports), &cancel).is_err()
            );
        }
    }

    #[test]
    fn changed_identity_spl_ecc_bad_blocks_or_mounted_ubi_deny_write_authority() {
        let cancel = Cancellation::default();
        let operation = Operation::ProgramReleaseUbootBackup;
        for change in 0..7 {
            let (mut inventory, mut reports) = chain();
            match change {
                0 => reports[0].spl_copies[0].data_sha256 = super::super::ORIGINAL_SPL.into(),
                1 => reports[1].spl_copies[1].data_sha256 = super::super::release::PROGRAM.into(),
                2 => reports[2].ecc_failures_after = 1,
                3 => reports[3].page_marker_bytes[255] = [0, 0],
                4 => inventory.mtd[3].bad_blocks = 1,
                _ => {}
            }
            let mut observed = observe(&inventory, &reports);
            if change == 5 {
                observed.sid = &[0; 16];
            }
            if change == 6 {
                observed.mounts = "ubi0:rootfs / ubifs rw 0 0\n";
            }
            assert!(Preflight::validate(operation, &observed, &cancel).is_err());
        }
        let (inventory, reports) = chain();
        cancel.cancel();
        assert!(matches!(
            Preflight::validate(operation, &observe(&inventory, &reports), &cancel),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn uboot_verification_rejects_raw_bytes_wrong_slot_program_and_ecc_failure() {
        let mut report = release_report(BootRegion::FourthBootBlock);
        Operation::ProgramReleaseUbootBackup
            .verify(&report)
            .unwrap();
        assert!(
            Operation::RestoreOriginalUbootPrimary
                .verify(&report)
                .is_err()
        );
        report.interpretation = ReadInterpretation::Raw;
        assert!(
            Operation::ProgramReleaseUbootBackup
                .verify(&report)
                .is_err()
        );
        report.interpretation = ReadInterpretation::KernelCorrected;
        report.data_sha256 = super::super::ORIGINAL_UBOOT.into();
        assert!(
            Operation::ProgramReleaseUbootBackup
                .verify(&report)
                .is_err()
        );
        report.data_sha256 = RELEASE_PROGRAM.into();
        report.ecc_failures_after = 1;
        assert!(
            Operation::ProgramReleaseUbootBackup
                .verify(&report)
                .is_err()
        );
        let (_, reports) = chain();
        assert!(super::super::verify_erased(&reports[3], BootRegion::FourthBootBlock).is_err());
        verify_erased(&reports[3], BootRegion::FourthBootBlock).unwrap();
    }

    #[test]
    fn correction_diagnostics_must_match_counters_and_ordered_page_offsets() {
        let (_, mut reports) = chain();
        let report = &mut reports[2];
        verify_corrections(report).unwrap();
        let original = report.tool_stderr.clone();
        for changed in [
            format!("{original}warning\n"),
            original.replace("offset 0x00004000", "offset 0x00000000"),
            original.replace("ECC: 26", "ECC: 0"),
        ] {
            report.tool_stderr = changed;
            assert!(verify_corrections(report).is_err());
        }
        report.tool_stderr = original;
        report.corrected_bits_after = report.corrected_bits_before;
        assert!(verify_corrections(report).is_err());
    }

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
