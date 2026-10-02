//! Private durable host intent and verification records for the SPL trial.
//! Records diagnose interruption; reading one never authorizes or resumes writes.
use super::{Operation, SID};
use crate::{Cancellation, Error, recovery::BootReadback};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

const ORIGINAL_VERSION: u8 = 1;
const RELEASE_VERSION: u8 = 2;
const UBOOT_VERSION: u8 = 3;
const MAX_BYTES: u64 = 8192;
// Version 3 binds six U-Boot/SPL hashes in addition to the existing context.
const MAX_LINE: usize = 2048;

/// Public session identity only. No HMAC key or credentials are serialized.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub sid: [u8; 16],
    pub session: [u8; 16],
    pub daemon_sha256: [u8; 32],
    pub operation: Operation,
}
impl Context {
    fn validate(&self) -> Result<(), Error> {
        if self.sid != SID || self.session == [0; 16] || self.daemon_sha256 == [0; 32] {
            return Err(Error::Device);
        }
        Ok(())
    }
}

/// Dispatched means the host may have submitted the committed operation;
/// loss of its response is indeterminate, never success or safe automatic retry.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Stage {
    Intent,
    Prepared,
    Dispatched,
    Verified,
    FailedBeforeDispatch,
    CancelledBeforeDispatch,
    Indeterminate,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub sequence: u8,
    pub stage: Stage,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub version: u8,
    pub context: Context,
    pub restoration_sha256: String,
    pub original_spl_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_spl: Option<super::release::Pins>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uboot_images: Option<super::uboot::Pins>,
}
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Report {
    pub header: Header,
    pub entries: Vec<Entry>,
}

/// One exclusive new journal. An I/O failure poisons further transitions.
#[derive(Debug)]
pub struct Journal {
    file: File,
    report: Report,
    poisoned: bool,
}
impl Journal {
    /// Atomically publishes a new private intent record and fsyncs it before use.
    /// # Errors
    /// Rejects unsafe/non-private paths, overwrite, invalid identity, cancellation or I/O errors.
    pub fn create(path: &Path, context: Context, cancel: &Cancellation) -> Result<Self, Error> {
        cancel.check()?;
        context.validate()?;
        let parent = private_parent(path)?;
        let release_trial = context.operation.is_release_trial();
        let uboot_trial = context.operation.is_uboot_trial();
        let report = Report {
            header: Header {
                version: if uboot_trial {
                    UBOOT_VERSION
                } else if release_trial {
                    RELEASE_VERSION
                } else {
                    ORIGINAL_VERSION
                },
                context,
                restoration_sha256: super::RESTORATION_IMAGE.into(),
                original_spl_sha256: super::ORIGINAL_SPL.into(),
                release_spl: release_trial.then(super::release::Pins::expected),
                uboot_images: uboot_trial.then(super::uboot::Pins::expected),
            },
            entries: vec![Entry {
                sequence: 0,
                stage: Stage::Intent,
            }],
        };
        let mut bytes = line(&report.header)?;
        bytes.extend(line(&report.entries[0])?);
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        cancel.check()?;
        let file = temporary
            .persist_noclobber(path)
            .map_err(|e| Error::Io(e.error))?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(Self {
            file,
            report,
            poisoned: false,
        })
    }

    /// Records preparation, dispatch or a terminal failure. Verified cannot
    /// enter through this method: it requires checked physical readback.
    /// # Errors
    /// Rejects invalid/terminal transitions, cancellation or journal I/O failure.
    pub fn advance(&mut self, stage: Stage, cancel: &Cancellation) -> Result<(), Error> {
        if stage == Stage::Verified {
            return Err(Error::State);
        }
        if matches!(stage, Stage::Prepared | Stage::Dispatched) {
            cancel.check()?;
        }
        self.append(stage)
    }

    /// Validates the operation's exact readback before publishing Verified.
    /// # Errors
    /// Rejects cancellation, invalid result/transition or journal I/O failure.
    pub fn verify(&mut self, report: &BootReadback, cancel: &Cancellation) -> Result<(), Error> {
        cancel.check()?;
        self.report.header.context.operation.verify(report)?;
        cancel.check()?;
        self.append(Stage::Verified)
    }

    fn append(&mut self, stage: Stage) -> Result<(), Error> {
        let previous = self.report.entries.last().ok_or(Error::State)?.stage;
        if self.poisoned || !transition(previous, stage) {
            return Err(Error::State);
        }
        let entry = Entry {
            sequence: u8::try_from(self.report.entries.len()).map_err(|_| Error::Length)?,
            stage,
        };
        let bytes = line(&entry)?;
        if let Err(error) = self
            .file
            .write_all(&bytes)
            .and_then(|()| self.file.sync_all())
        {
            self.poisoned = true;
            return Err(Error::Io(error));
        }
        self.report.entries.push(entry);
        Ok(())
    }
}

const fn transition(previous: Stage, next: Stage) -> bool {
    matches!(
        (previous, next),
        (
            Stage::Intent,
            Stage::Prepared | Stage::FailedBeforeDispatch | Stage::CancelledBeforeDispatch
        ) | (
            Stage::Prepared,
            Stage::Dispatched | Stage::FailedBeforeDispatch | Stage::CancelledBeforeDispatch
        ) | (Stage::Dispatched, Stage::Verified | Stage::Indeterminate)
    )
}

fn private_parent(path: &Path) -> Result<&Path, Error> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(Error::UnsafePath);
    }
    let parent = path.parent().ok_or(Error::UnsafePath)?;
    crate::assets::regular_components(parent)?;
    let metadata = fs::metadata(parent)?;
    if !metadata.is_dir() {
        return Err(Error::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::UnsafePath);
        }
    }
    Ok(parent)
}

fn line(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| Error::Recovery)?;
    if bytes.len() >= MAX_LINE {
        return Err(Error::Length);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Reads only bounded private regular journals; malformed/truncated records
/// cannot be reported as verified. This grants no resumption authorization.
/// # Errors
/// Rejects unsafe paths, wrong schema/pins/order, bounds, cancellation or I/O errors.
pub fn read(path: &Path, cancel: &Cancellation) -> Result<Report, Error> {
    cancel.check()?;
    private_parent(path)?;
    crate::assets::regular_components(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(Error::UnsafePath);
    }
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
            return Err(Error::UnsafePath);
        }
    }
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(Error::Length);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES || !bytes.ends_with(b"\n") {
        return Err(Error::Length);
    }
    let mut lines = bytes[..bytes.len() - 1].split(|b| *b == b'\n');
    let header: Header = parse(lines.next().ok_or(Error::Length)?)?;
    header.context.validate()?;
    let release_trial = header.context.operation.is_release_trial();
    let uboot_trial = header.context.operation.is_uboot_trial();
    if header.version
        != if uboot_trial {
            UBOOT_VERSION
        } else if release_trial {
            RELEASE_VERSION
        } else {
            ORIGINAL_VERSION
        }
        || header.restoration_sha256 != super::RESTORATION_IMAGE
        || header.original_spl_sha256 != super::ORIGINAL_SPL
        || header.release_spl != release_trial.then(super::release::Pins::expected)
        || header.uboot_images != uboot_trial.then(super::uboot::Pins::expected)
    {
        return Err(Error::Recovery);
    }
    let mut entries: Vec<Entry> = Vec::new();
    for raw in lines {
        cancel.check()?;
        let entry: Entry = parse(raw)?;
        if entries.len() >= 4
            || usize::from(entry.sequence) != entries.len()
            || entries
                .last()
                .map_or(entry.stage != Stage::Intent, |previous| {
                    !transition(previous.stage, entry.stage)
                })
        {
            return Err(Error::State);
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(Error::Length);
    }
    cancel.check()?;
    Ok(Report { header, entries })
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    if bytes.is_empty() || bytes.len() >= MAX_LINE {
        return Err(Error::Length);
    }
    serde_json::from_slice(bytes).map_err(|_| Error::Recovery)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_journal_binds_exact_pins_and_cannot_downgrade_to_original_version() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("release.jsonl");
        let mut context = context();
        context.operation = Operation::ProgramReleasePrimary;
        let mut journal = Journal::create(&path, context, &Cancellation::default()).unwrap();
        assert_eq!(
            read(&path, &Cancellation::default())
                .unwrap()
                .header
                .version,
            2
        );
        journal
            .advance(Stage::Prepared, &Cancellation::default())
            .unwrap();
        journal
            .advance(Stage::Dispatched, &Cancellation::default())
            .unwrap();
        journal
            .verify(
                &super::super::release::scripted_fixture(),
                &Cancellation::default(),
            )
            .unwrap();
        let valid = std::fs::read_to_string(&path).unwrap();
        let (header, entries) = valid.split_once('\n').unwrap();
        for change in 0..3 {
            let mut header: serde_json::Value = serde_json::from_str(header).unwrap();
            match change {
                0 => header["version"] = 1.into(),
                1 => {
                    header.as_object_mut().unwrap().remove("release_spl");
                }
                _ => header["release_spl"]["program_sha256"] = "0".repeat(64).into(),
            }
            std::fs::write(
                &path,
                format!("{}\n{entries}", serde_json::to_string(&header).unwrap()),
            )
            .unwrap();
            assert!(read(&path, &Cancellation::default()).is_err());
        }
    }

    #[test]
    fn uboot_journal_pins_and_version_cannot_be_downgraded_or_removed() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("uboot.jsonl");
        let mut ctx = context();
        ctx.operation = Operation::ProgramReleaseUbootBackup;
        let journal = Journal::create(&path, ctx, &Cancellation::default()).unwrap();
        drop(journal);
        let text = fs::read_to_string(&path).unwrap();
        let (header, entries) = text.split_once('\n').unwrap();
        assert_eq!(
            read(&path, &Cancellation::default())
                .unwrap()
                .header
                .version,
            3
        );
        for change in 0..4 {
            let mut header: serde_json::Value = serde_json::from_str(header).unwrap();
            match change {
                0 => header["version"] = 2.into(),
                1 => {
                    header.as_object_mut().unwrap().remove("uboot_images");
                }
                2 => header["uboot_images"]["release_program_sha256"] = "0".repeat(64).into(),
                _ => {
                    header["uboot_images"]["primary_spl_sha256"] =
                        super::super::ORIGINAL_SPL.into();
                }
            }
            fs::write(&path, format!("{header}\n{entries}")).unwrap();
            assert!(read(&path, &Cancellation::default()).is_err());
        }
    }

    fn context() -> Context {
        Context {
            sid: SID,
            session: [1; 16],
            daemon_sha256: [2; 32],
            operation: Operation::RestorePrimary,
        }
    }
    fn measured_primary() -> BootReadback {
        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-spl-bch64-readbacks-7.json"
        ))
        .unwrap();
        serde_json::from_value(evidence["records"][0]["response"].clone()).unwrap()
    }

    #[test]
    fn actual_interrupted_host_record_remains_indeterminate_on_inspection() {
        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-backup-erase-interrupted-host-10.json"
        ))
        .unwrap();
        let record = &evidence["host_journal"];
        let mut bytes = serde_json::to_vec(&record["header"]).unwrap();
        bytes.push(b'\n');
        for entry in record["entries"].as_array().unwrap() {
            bytes.extend(serde_json::to_vec(entry).unwrap());
            bytes.push(b'\n');
        }
        let directory = crate::assets::temporary_directory().unwrap();
        let mut file = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        file.write_all(&bytes).unwrap();
        file.as_file().sync_all().unwrap();
        let report = read(file.path(), &Cancellation::default()).unwrap();
        assert_eq!(report.header.context.operation, Operation::EraseBackup);
        assert_eq!(report.entries.last().unwrap().stage, Stage::Indeterminate);
        assert_eq!(report.entries.len(), 4);
    }

    #[test]
    fn backup_verification_is_bound_to_its_operation_and_partition() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("backup.jsonl");
        let cancel = Cancellation::default();
        let context = Context {
            sid: SID,
            session: [1; 16],
            daemon_sha256: [2; 32],
            operation: Operation::RestoreBackup,
        };
        let mut journal = Journal::create(&path, context, &cancel).unwrap();
        journal.advance(Stage::Prepared, &cancel).unwrap();
        journal.advance(Stage::Dispatched, &cancel).unwrap();
        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-primary-restoration-trial-9.json"
        ))
        .unwrap();
        let mut report: BootReadback =
            serde_json::from_value(evidence["response"]["SplTrialVerified"]["readback"].clone())
                .unwrap();
        assert!(journal.verify(&report, &cancel).is_err());
        // Explicit host fixture for the same clean program in the backup slot;
        // no physical backup restoration is claimed by this test.
        report.region = crate::recovery::BootRegion::SplBackup;
        journal.verify(&report, &cancel).unwrap();
        assert_eq!(
            read(&path, &cancel).unwrap().entries.last().unwrap().stage,
            Stage::Verified
        );
    }

    #[test]
    fn physical_readback_is_required_before_durable_verified_terminal_state() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("trial.jsonl");
        let cancel = Cancellation::default();
        let mut journal = Journal::create(&path, context(), &cancel).unwrap();
        assert_eq!(
            read(&path, &cancel).unwrap().entries[0].stage,
            Stage::Intent
        );
        let before = fs::read(&path).unwrap();
        assert!(journal.advance(Stage::Verified, &cancel).is_err());
        assert!(journal.verify(&measured_primary(), &cancel).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        journal.advance(Stage::Prepared, &cancel).unwrap();
        journal.advance(Stage::Dispatched, &cancel).unwrap();
        journal.verify(&measured_primary(), &cancel).unwrap();
        let result = read(&path, &cancel).unwrap();
        assert_eq!(
            result.entries.iter().map(|e| e.stage).collect::<Vec<_>>(),
            [
                Stage::Intent,
                Stage::Prepared,
                Stage::Dispatched,
                Stage::Verified
            ]
        );
        assert_eq!(result.header.context, context());
        assert!(journal.advance(Stage::Indeterminate, &cancel).is_err());
        assert!(journal.verify(&measured_primary(), &cancel).is_err());
    }

    #[test]
    fn cancellation_and_lost_response_cannot_become_success_or_resume_writes() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("cancelled.jsonl");
        let cancel = Cancellation::default();
        let mut journal = Journal::create(&path, context(), &cancel).unwrap();
        cancel.cancel();
        assert!(matches!(
            journal.advance(Stage::Prepared, &cancel),
            Err(Error::Cancelled)
        ));
        journal
            .advance(Stage::CancelledBeforeDispatch, &cancel)
            .unwrap();
        assert_eq!(
            read(&path, &Cancellation::default())
                .unwrap()
                .entries
                .last()
                .unwrap()
                .stage,
            Stage::CancelledBeforeDispatch
        );
        assert!(
            journal
                .advance(Stage::Prepared, &Cancellation::default())
                .is_err()
        );

        let path = directory.path().join("lost-response.jsonl");
        let cancel = Cancellation::default();
        let mut journal = Journal::create(&path, context(), &cancel).unwrap();
        journal.advance(Stage::Prepared, &cancel).unwrap();
        journal.advance(Stage::Dispatched, &cancel).unwrap();
        cancel.cancel();
        assert!(matches!(
            journal.verify(&measured_primary(), &cancel),
            Err(Error::Cancelled)
        ));
        journal.advance(Stage::Indeterminate, &cancel).unwrap();
        assert!(
            journal
                .advance(Stage::Dispatched, &Cancellation::default())
                .is_err()
        );
        assert_eq!(
            read(&path, &Cancellation::default())
                .unwrap()
                .entries
                .last()
                .unwrap()
                .stage,
            Stage::Indeterminate
        );
    }

    #[test]
    fn write_failure_poisoning_preserves_intent_and_prevents_further_transitions() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("failed-io.jsonl");
        let cancel = Cancellation::default();
        let mut journal = Journal::create(&path, context(), &cancel).unwrap();
        journal.file = File::open(&path).unwrap(); // Real read-only descriptor: writes fail.
        assert!(matches!(
            journal.advance(Stage::Prepared, &cancel),
            Err(Error::Io(_))
        ));
        assert!(matches!(
            journal.advance(Stage::FailedBeforeDispatch, &cancel),
            Err(Error::State)
        ));
        let result = read(&path, &cancel).unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].stage, Stage::Intent);
    }

    #[test]
    fn unsafe_paths_existing_files_and_changed_identity_cannot_create_or_overwrite() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("existing.jsonl");
        fs::write(&path, b"user data").unwrap();
        assert!(Journal::create(&path, context(), &Cancellation::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"user data");
        let absent = directory.path().join("absent.jsonl");
        let mut wrong = context();
        wrong.sid = [0; 16];
        assert!(Journal::create(&absent, wrong, &Cancellation::default()).is_err());
        assert!(!absent.exists());
        assert!(
            Journal::create(
                Path::new("relative.jsonl"),
                context(),
                &Cancellation::default()
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            let link = directory.path().join("link.jsonl");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read(&link, &Cancellation::default()).is_err());
            assert!(Journal::create(&link, context(), &Cancellation::default()).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"user data");
        }
    }

    #[test]
    fn partial_reordered_oversized_or_tampered_records_are_not_verified() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("source.jsonl");
        let cancel = Cancellation::default();
        let mut journal = Journal::create(&path, context(), &cancel).unwrap();
        journal.advance(Stage::Prepared, &cancel).unwrap();
        journal.advance(Stage::Dispatched, &cancel).unwrap();
        journal.verify(&measured_primary(), &cancel).unwrap();
        let bytes = fs::read(&path).unwrap();
        let partial = bytes[..bytes.len() - 1].to_vec();
        let text = String::from_utf8(bytes).unwrap();
        let reordered = text
            .replace("\"sequence\":2", "\"sequence\":1")
            .into_bytes();
        let changed_pin = text
            .replace(super::super::RESTORATION_IMAGE, &"0".repeat(64))
            .into_bytes();
        for altered in [
            partial,
            reordered,
            changed_pin,
            vec![b'x'; 8193],
            b"{}\n".to_vec(),
        ] {
            fs::write(&path, altered).unwrap();
            assert!(read(&path, &cancel).is_err());
        }
    }
}
