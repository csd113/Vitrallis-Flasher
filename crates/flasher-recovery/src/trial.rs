//! Closed mtd0/mtd1 diagnostic. All preconditions originate on this RAM device.
use flasher_core::{
    Cancellation, Error,
    boot_trial::{
        self, Observations, Operation, OriginalSplFile, Preflight,
        journal::{Journal, Stage},
        release::LockedHynixSplFile,
        uboot,
    },
    recovery::{
        BootRegion, Channel, Credentials, ReadInterpretation, Request, Response, spl_trial::Ticket,
    },
    tool::{SystemToolRunner, ToolRequest, ToolRunner},
};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

const RESTORATION: &str = "/run/vitrallis-original-spl.nand";
const RELEASE: &str = "/run/vitrallis-locked-hynix-spl.nand";
const UBOOT: &str = "/run/vitrallis-uboot-pair.bin";
// One erase/fallback and one restoration per RAM boot. A reconnect never resumes
// a ticket, and failed/indeterminate operations consume the same bounded budget.
const MAX_DISPATCHES: usize = 2;

#[derive(Default)]
pub struct Service {
    journals: Vec<tempfile::TempDir>,
}
struct Prepared {
    ticket: Ticket,
    candidates: Candidates,
}
struct Candidates {
    restoration: OriginalSplFile,
    release: Option<LockedHynixSplFile>,
    uboot: Option<uboot::Images>,
}

fn tool_capabilities(cancel: &Cancellation) -> Result<(), Error> {
    for (program, required) in [
        (
            "/usr/sbin/flash_erase",
            &["--noskipbad", "--quiet", "<block count>"][..],
        ),
        (
            "/usr/sbin/nandwrite",
            &["--noecc", "--oob", "--noskipbad", "--quiet"][..],
        ),
    ] {
        let output = SystemToolRunner.run(&ToolRequest::new(program, ["--help"]), cancel)?;
        validate_tool_help(&output.stdout, required)?;
    }
    Ok(())
}

fn validate_tool_help(help: &str, required: &[&str]) -> Result<(), Error> {
    if required.iter().any(|flag| !help.contains(flag)) {
        return Err(Error::Device);
    }
    Ok(())
}

impl Candidates {
    fn revalidate(&mut self, operation: Operation, cancel: &Cancellation) -> Result<(), Error> {
        self.restoration.revalidate(cancel)?;
        if operation.is_release_trial() != self.release.is_some() {
            return Err(Error::State);
        }
        if let Some(candidate) = &mut self.release {
            candidate.revalidate(cancel)?;
        }
        if operation.is_uboot_trial() != self.uboot.is_some() {
            return Err(Error::State);
        }
        if let Some(images) = &mut self.uboot {
            images.revalidate(cancel)?;
        }
        Ok(())
    }
}

enum Checked {
    Spl(Preflight),
    Uboot(uboot::Preflight),
}

fn preflight(operation: Operation, cancel: &Cancellation) -> Result<Checked, Error> {
    if operation.is_uboot_trial() {
        return with_uboot_observations(operation, false, cancel, |observed| {
            uboot::Preflight::validate(operation, observed, cancel).map(Checked::Uboot)
        });
    }
    tool_capabilities(cancel)?;
    let mut sid = [0; 16];
    File::open("/sys/bus/nvmem/devices/sunxi-sid0/nvmem")?.read_exact(&mut sid)?;
    let inventory = super::inventory(cancel)?;
    let protected_spl = super::readback::read(
        operation.protected_region(),
        ReadInterpretation::Boot0Corrected,
        cancel,
    )?;
    let uboot = super::readback::read(
        BootRegion::UBoot,
        ReadInterpretation::KernelCorrected,
        cancel,
    )?;
    let mounts = super::text(Path::new("/proc/mounts"))?;
    let ubi_devices = match fs::read_dir("/sys/class/ubi") {
        Ok(entries) => entries
            .map(|entry| entry?.file_name().into_string().map_err(|_| Error::State))
            .collect::<Result<Vec<_>, Error>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(Error::Io(error)),
    };
    Preflight::validate(
        operation,
        &Observations {
            sid: &sid,
            inventory: &inventory,
            protected_spl: &protected_spl,
            uboot: &uboot,
            mounts: &mounts,
            ubi_devices: &ubi_devices,
        },
        cancel,
    )
    .map(Checked::Spl)
}

fn with_uboot_observations<T>(
    operation: Operation,
    after: bool,
    cancel: &Cancellation,
    check: impl FnOnce(&uboot::Observations<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    tool_capabilities(cancel)?;
    let mut sid = [0; 16];
    File::open("/sys/bus/nvmem/devices/sunxi-sid0/nvmem")?.read_exact(&mut sid)?;
    let inventory = super::inventory(cancel)?;
    let spl_primary = super::readback::read(
        BootRegion::SplPrimary,
        ReadInterpretation::Boot0Corrected,
        cancel,
    )?;
    let spl_backup = super::readback::read(
        BootRegion::SplBackup,
        ReadInterpretation::Boot0Corrected,
        cancel,
    )?;
    let primary_raw = if after {
        operation == Operation::EraseOriginalUbootPrimary
    } else {
        matches!(
            operation,
            Operation::RestoreOriginalUbootPrimary | Operation::ProgramReleaseUbootPrimary
        )
    };
    let backup_raw = if after {
        operation == Operation::EraseReleaseUbootBackup
    } else {
        matches!(
            operation,
            Operation::ProgramReleaseUbootBackup | Operation::RestoreReleaseUbootBackup
        )
    };
    let interpretation = |raw| {
        if raw {
            ReadInterpretation::Raw
        } else {
            ReadInterpretation::KernelCorrected
        }
    };
    let primary = super::readback::read(BootRegion::UBoot, interpretation(primary_raw), cancel)?;
    let backup = super::readback::read(
        BootRegion::FourthBootBlock,
        interpretation(backup_raw),
        cancel,
    )?;
    let mounts = super::text(Path::new("/proc/mounts"))?;
    let ubi_devices = match fs::read_dir("/sys/class/ubi") {
        Ok(entries) => entries
            .map(|entry| entry?.file_name().into_string().map_err(|_| Error::State))
            .collect::<Result<Vec<_>, Error>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(Error::Io(error)),
    };
    check(&uboot::Observations {
        sid: &sid,
        inventory: &inventory,
        mounts: &mounts,
        ubi_devices: &ubi_devices,
        spl_primary: &spl_primary,
        spl_backup: &spl_backup,
        primary: &primary,
        backup: &backup,
    })
}

fn prepare(operation: Operation, cancel: &Cancellation) -> Result<(Prepared, Response), Error> {
    let _preflight = preflight(operation, cancel)?;
    // Restoration is mandatory even for erase: recovery must already have the
    // exact clean original program available before removing either SPL block.
    let restoration = OriginalSplFile::open(Path::new(RESTORATION), cancel)?;
    let release = if operation.is_release_trial() {
        Some(LockedHynixSplFile::open(Path::new(RELEASE), cancel)?)
    } else {
        None
    };
    if matches!(
        operation,
        Operation::ErasePrimary | Operation::EraseBackup | Operation::EraseBackupForReleasePrimary
    ) {
        let existing_target = super::readback::read(
            operation.target(),
            ReadInterpretation::Boot0Corrected,
            cancel,
        )?;
        boot_trial::verify_spl(&existing_target, operation.target())?;
    }
    let uboot = if operation.is_uboot_trial() {
        Some(uboot::Images::open(Path::new(UBOOT), cancel)?)
    } else {
        None
    };
    let (ticket, response) = Ticket::issue(operation)?;
    Ok((
        Prepared {
            ticket,
            candidates: Candidates {
                restoration,
                release,
                uboot,
            },
        },
        Response::SplTrialPrepared(response),
    ))
}

impl Service {
    pub(crate) fn serve(
        &mut self,
        channel: &mut Channel,
        credentials: &Credentials,
        implementation: [u8; 32],
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        // The prepared ticket/snapshot die with this connection, including on
        // authentication/framing/disconnect failure. No cross-connection replay.
        let mut prepared = None;
        channel.serve_reviewed(
            |request| match request {
                Request::Ping => Ok(Response::Pong),
                Request::Inventory => Ok(Response::Inventory(Box::new(super::inventory(cancel)?))),
                Request::RootfsMap => Ok(Response::RootfsMap(Box::new(super::rootfs_map(cancel)?))),
                Request::PhysicalMarker => Ok(Response::PhysicalMarker(Box::new(
                    super::marker::read(cancel)?,
                ))),
                Request::BootReadback {
                    region,
                    interpretation,
                } => {
                    interpretation.validate(region)?;
                    Ok(Response::BootReadback(Box::new(super::readback::read(
                        region,
                        interpretation,
                        cancel,
                    )?)))
                }
                Request::ReturnToFel => Ok(Response::RestartAccepted),
                Request::PrepareSplTrial { operation } => {
                    if prepared.is_some() || self.journals.len() >= MAX_DISPATCHES {
                        return Err(Error::State);
                    }
                    let (state, response) = prepare(operation, cancel)?;
                    prepared = Some(state);
                    Ok(response)
                }
                Request::ExecuteSplTrial { operation, token } => {
                    let state = prepared.take().ok_or(Error::State)?;
                    state.ticket.consume(operation, &token)?;
                    self.execute(
                        state.candidates,
                        operation,
                        credentials,
                        implementation,
                        cancel,
                    )
                }
            },
            cancel,
        )
    }

    fn execute(
        &mut self,
        mut candidates: Candidates,
        operation: Operation,
        credentials: &Credentials,
        implementation: [u8; 32],
        cancel: &Cancellation,
    ) -> Result<Response, Error> {
        if self.journals.len() >= MAX_DISPATCHES {
            return Err(Error::State);
        }
        // Fresh SID, geometry, mounts and the protected boot chain immediately before intent.
        let checked = preflight(operation, cancel)?;
        candidates.revalidate(operation, cancel)?;
        let directory = flasher_core::assets::temporary_directory()?;
        let path = directory.path().join("trial.jsonl");
        let mut journal = Journal::create(
            &path,
            credentials.trial_context(implementation, operation),
            cancel,
        )?;
        journal.advance(Stage::Prepared, cancel)?;
        journal.advance(Stage::Dispatched, cancel)?;
        eprintln!(
            "Pinned boot diagnostic dispatched: {operation:?}; RAM journal {}",
            path.display()
        );
        self.journals.push(directory); // retain diagnosis even after disconnect/failure
        // Receipt of authenticated Execute is the commit boundary. Device work
        // completes independently of host socket lifetime; it never retries.
        let committed = Cancellation::default();
        let result =
            mutate(&checked, &mut candidates, operation, &committed).and_then(|readback| {
                journal.verify(&readback, &committed)?;
                Ok(Response::SplTrialVerified {
                    operation,
                    readback: Box::new(readback),
                })
            });
        if result.is_err() {
            journal.advance(Stage::Indeterminate, &committed)?;
        }
        result
    }
}

fn mutate(
    checked: &Checked,
    candidates: &mut Candidates,
    operation: Operation,
    cancel: &Cancellation,
) -> Result<flasher_core::recovery::BootReadback, Error> {
    let read_erased = || super::readback::read(operation.target(), ReadInterpretation::Raw, cancel);
    let erased = match checked {
        Checked::Spl(proof) => proof.erase_and_verify(&SystemToolRunner, read_erased, cancel)?,
        Checked::Uboot(proof) => proof.erase_and_verify(&SystemToolRunner, read_erased, cancel)?,
    };
    let programmed = match operation {
        Operation::RestorePrimary
        | Operation::RestoreBackup
        | Operation::RestoreBackupForReleasePrimary => {
            let Checked::Spl(proof) = checked else {
                return Err(Error::State);
            };
            candidates
                .restoration
                .restore(proof, &SystemToolRunner, cancel)?;
            true
        }
        Operation::ProgramReleasePrimary => {
            candidates.release.as_mut().ok_or(Error::State)?.program(
                match checked {
                    Checked::Spl(proof) => proof,
                    Checked::Uboot(_) => return Err(Error::State),
                },
                &SystemToolRunner,
                cancel,
            )?;
            true
        }
        Operation::ProgramReleaseUbootBackup
        | Operation::RestoreOriginalUbootPrimary
        | Operation::ProgramReleaseUbootPrimary
        | Operation::RestoreReleaseUbootBackup => {
            let Checked::Uboot(proof) = checked else {
                return Err(Error::State);
            };
            candidates.uboot.as_mut().ok_or(Error::State)?.program(
                proof,
                &SystemToolRunner,
                cancel,
            )?;
            true
        }
        Operation::ErasePrimary
        | Operation::EraseBackup
        | Operation::EraseBackupForReleasePrimary
        | Operation::EraseOriginalUbootPrimary
        | Operation::EraseReleaseUbootBackup => false,
    };
    let readback = if programmed {
        super::readback::read(
            operation.target(),
            if operation.is_uboot_trial() {
                ReadInterpretation::KernelCorrected
            } else {
                ReadInterpretation::Boot0Corrected
            },
            cancel,
        )?
    } else {
        erased
    };
    // Also prove the untouched opposite SPL block and U-Boot still pass after mutation.
    if operation.is_uboot_trial() {
        with_uboot_observations(operation, true, cancel, |observed| {
            uboot::verify_protected(operation, observed, cancel)
        })?;
    } else {
        let _after = preflight(operation, cancel)?;
    }
    Ok(readback)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_fixed_utility_options_fail_before_preflight() {
        let required = ["--noecc", "--oob", "--noskipbad", "--quiet"];
        validate_tool_help(&required.join(" "), &required).unwrap();
        for missing in required {
            let help = required
                .iter()
                .filter(|flag| **flag != missing)
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            assert!(validate_tool_help(&help, &required).is_err());
        }
    }
}
