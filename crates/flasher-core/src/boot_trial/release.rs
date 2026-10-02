//! Exact locked Hynix SPL trial, distinct from original-program restoration.
use super::{Boot0Decoder, BootReadback, BootRegion, IMAGE_BYTES, OOB, Operation, PAGE, Preflight};
use crate::{
    Cancellation, Error,
    tool::{ToolOutput, ToolRequest, ToolRunner},
};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

pub const ARTIFACT: &str = "0099342e6331e9880d704bf11eb75f1b7618be8cef05b2ae456918fa1010fc7a";
pub const PROGRAM: &str = "879cff4d6345a12091fa8084bab5a002556989b2d10ce1898905dd8666729ba0";
pub const MANIFEST: &str = "48e46f66e4c1b6927b0864f9b285ae46a8c1c6e3c74033947a80643e7f723cc2";

/// Exact diagnostic artifact binding; this is not physical manifest approval.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pins {
    pub artifact_sha256: String,
    pub program_sha256: String,
    pub manifest_sha256: String,
}
impl Pins {
    #[must_use]
    pub fn expected() -> Self {
        Self {
            artifact_sha256: ARTIFACT.into(),
            program_sha256: PROGRAM.into(),
            manifest_sha256: MANIFEST.into(),
        }
    }
}

/// Checks only the fixed primary against all four decoded locked programs.
/// # Errors
/// Rejects any other target, encoding interpretation, program or checksum.
pub fn verify_primary(report: &BootReadback) -> Result<(), Error> {
    super::verify_program(report, BootRegion::SplPrimary, super::Program::LockedHynix)
}

/// Private snapshot of the fixed locked Hynix artifact, never an arbitrary image.
#[derive(Debug)]
pub struct LockedHynixSplFile {
    file: tempfile::NamedTempFile,
    _directory: tempfile::TempDir,
}
impl LockedHynixSplFile {
    /// Validate the exact artifact, native programs and raw markers before snapshot.
    /// # Errors
    /// Rejects unsafe paths, wrong profile/length/hash, ECC errors or cancellation.
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<Self, Error> {
        let (file, directory) = super::snapshot(path, validate_image, cancel)?;
        Ok(Self {
            file,
            _directory: directory,
        })
    }
    /// Revalidate the retained snapshot immediately before dispatch.
    /// # Errors
    /// Rejects changed bytes, cancellation or I/O failure.
    pub fn revalidate(&mut self, cancel: &Cancellation) -> Result<(), Error> {
        super::revalidate_snapshot(&mut self.file, validate_image, cancel)
    }
    /// Program only the separately validated fixed release-primary operation.
    /// The caller must journal dispatch and verify native corrected readback.
    /// # Errors
    /// Rejects another operation, changed snapshot or tool failure; never retries.
    pub fn program(
        &mut self,
        checked: &Preflight,
        runner: &impl ToolRunner,
        cancel: &Cancellation,
    ) -> Result<ToolOutput, Error> {
        if checked.operation != Operation::ProgramReleasePrimary {
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
                    "/dev/mtd0",
                    path,
                ],
            )
            .timeout(Duration::from_secs(10)),
            cancel,
        )
    }
}

fn validate_image(bytes: &[u8], cancel: &Cancellation) -> Result<(), Error> {
    cancel.check()?;
    if bytes.len() != IMAGE_BYTES || format!("{:x}", Sha256::digest(bytes)) != ARTIFACT {
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
    if copies.iter().any(|copy| {
        copy.data_sha256 != PROGRAM
            || copy.checksum != super::Program::LockedHynix.checksum()
            || copy.corrected_bits != [0; 16]
    }) {
        return Err(Error::Hash);
    }
    cancel.check()
}

// Software-combined fixture: original readback framing plus native decoded
// locked artifact programs. It is not a physical locked-release NAND read.
#[cfg(test)]
pub(crate) fn scripted_fixture() -> BootReadback {
    let original: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../docs/evidence/batch3/recovery-spl-bch64-readbacks-7.json"
    ))
    .unwrap();
    let locked: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../docs/evidence/batch3/locked-release-hynix-spl-native-review.json"
    ))
    .unwrap();
    let mut report: BootReadback =
        serde_json::from_value(original["records"][0]["response"].clone()).unwrap();
    report.spl_copies = serde_json::from_value(locked["copies"].clone()).unwrap();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        boot_trial::{Observations, OriginalSplFile, SID},
        recovery::{Inventory, ReadInterpretation},
        tool::ScriptedToolRunner,
    };

    #[test]
    fn exact_locked_program_does_not_relax_original_program_verification() {
        let mut report = scripted_fixture();
        verify_primary(&report).unwrap();
        assert!(super::super::verify_spl(&report, BootRegion::SplPrimary).is_err());
        report.region = BootRegion::SplBackup;
        assert!(verify_primary(&report).is_err());
        report.region = BootRegion::SplPrimary;
        report.interpretation = ReadInterpretation::Raw;
        assert!(verify_primary(&report).is_err());
        report.interpretation = ReadInterpretation::Boot0Corrected;
        report.spl_copies[1].checksum ^= 1;
        assert!(verify_primary(&report).is_err());
    }

    #[test]
    fn release_isolation_requires_release_primary_and_original_operation_stays_strict() {
        let inventory: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-inventory-7.json"
        ))
        .unwrap();
        let inventory: Inventory =
            serde_json::from_value(inventory["response"]["Inventory"].clone()).unwrap();
        let uboot: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-uboot-corrected-readback-7.json"
        ))
        .unwrap();
        let uboot: BootReadback =
            serde_json::from_value(uboot["response"]["BootReadback"].clone()).unwrap();
        let release = scripted_fixture();
        let source: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-spl-bch64-readbacks-7.json"
        ))
        .unwrap();
        let original: BootReadback =
            serde_json::from_value(source["records"][0]["response"].clone()).unwrap();
        for (operation, protector, accepted) in [
            (Operation::EraseBackupForReleasePrimary, &release, true),
            (Operation::RestoreBackupForReleasePrimary, &release, true),
            (Operation::EraseBackup, &release, false),
            (Operation::EraseBackupForReleasePrimary, &original, false),
        ] {
            let result = Preflight::validate(
                operation,
                &Observations {
                    sid: &SID,
                    inventory: &inventory,
                    protected_spl: protector,
                    uboot: &uboot,
                    mounts: "rootfs / rootfs rw 0 0\n",
                    ubi_devices: &[],
                },
                &Cancellation::default(),
            );
            assert_eq!(result.is_ok(), accepted);
        }
    }

    #[test]
    fn snapshot_types_cannot_substitute_original_and_release_writes() {
        let directory = crate::assets::temporary_directory().unwrap();
        let mut locked = LockedHynixSplFile {
            file: tempfile::NamedTempFile::new_in(directory.path()).unwrap(),
            _directory: directory,
        };
        let directory = crate::assets::temporary_directory().unwrap();
        let mut original = OriginalSplFile {
            file: tempfile::NamedTempFile::new_in(directory.path()).unwrap(),
            _directory: directory,
        };
        let runner = ScriptedToolRunner::new();
        assert!(matches!(
            locked.program(
                &Preflight {
                    operation: Operation::RestorePrimary
                },
                &runner,
                &Cancellation::default()
            ),
            Err(Error::State)
        ));
        assert!(matches!(
            original.restore(
                &Preflight {
                    operation: Operation::ProgramReleasePrimary
                },
                &runner,
                &Cancellation::default()
            ),
            Err(Error::State)
        ));
        assert!(matches!(
            locked.program(
                &Preflight {
                    operation: Operation::ProgramReleasePrimary
                },
                &runner,
                &Cancellation::default()
            ),
            Err(Error::Hash)
        ));
        assert_eq!(runner.calls().len(), 0);
    }

    #[test]
    fn wrong_variant_corruption_paths_and_cancellation_fail_before_snapshot() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("candidate.nand");
        for length in [0, 4_521_984, IMAGE_BYTES, IMAGE_BYTES + 1] {
            std::fs::write(&path, vec![0; length]).unwrap();
            assert!(LockedHynixSplFile::open(&path, &Cancellation::default()).is_err());
            assert_eq!(std::fs::metadata(&path).unwrap().len(), length as u64);
        }
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            LockedHynixSplFile::open(&path, &cancel),
            Err(Error::Cancelled)
        ));
        #[cfg(unix)]
        {
            let link = directory.path().join("link.nand");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(matches!(
                LockedHynixSplFile::open(&link, &Cancellation::default()),
                Err(Error::UnsafePath)
            ));
        }
    }
}
