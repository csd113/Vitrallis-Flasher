//! Closed mtd0/mtd1 diagnostic. All preconditions originate on this RAM device.
use flasher_core::{
    Cancellation, Error,
    boot_trial::{
        self, Observations, Operation, OriginalSplFile, Preflight,
        journal::{Journal, Stage},
        release::LockedHynixSplFile,
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
// One erase/fallback and one restoration per RAM boot. A reconnect never resumes
// a ticket, and failed/indeterminate operations consume the same bounded budget.
const MAX_DISPATCHES: usize = 2;

#[derive(Default)]
pub struct Service {
    journals: Vec<tempfile::TempDir>,
}
struct Prepared {
    ticket: Ticket,
    restoration: OriginalSplFile,
    release: Option<LockedHynixSplFile>,
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

fn preflight(operation: Operation, cancel: &Cancellation) -> Result<Preflight, Error> {
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
    let (ticket, response) = Ticket::issue(operation)?;
    Ok((
        Prepared {
            ticket,
            restoration,
            release,
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
                        state.restoration,
                        state.release,
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
        mut restoration: OriginalSplFile,
        mut release: Option<LockedHynixSplFile>,
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
        restoration.revalidate(cancel)?;
        if operation.is_release_trial() != release.is_some() {
            return Err(Error::State);
        }
        if let Some(candidate) = &mut release {
            candidate.revalidate(cancel)?;
        }
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
            "Pinned SPL diagnostic dispatched: {operation:?}; RAM journal {}",
            path.display()
        );
        self.journals.push(directory); // retain diagnosis even after disconnect/failure
        // Receipt of authenticated Execute is the commit boundary. Device work
        // completes independently of host socket lifetime; it never retries.
        let committed = Cancellation::default();
        let result = (|| {
            let erased = checked.erase_and_verify(
                &SystemToolRunner,
                || super::readback::read(checked.target(), ReadInterpretation::Raw, &committed),
                &committed,
            )?;
            let programmed = match operation {
                Operation::RestorePrimary
                | Operation::RestoreBackup
                | Operation::RestoreBackupForReleasePrimary => {
                    restoration.restore(&checked, &SystemToolRunner, &committed)?;
                    true
                }
                Operation::ProgramReleasePrimary => {
                    release.as_mut().ok_or(Error::State)?.program(
                        &checked,
                        &SystemToolRunner,
                        &committed,
                    )?;
                    true
                }
                Operation::ErasePrimary
                | Operation::EraseBackup
                | Operation::EraseBackupForReleasePrimary => false,
            };
            let readback = if programmed {
                super::readback::read(
                    checked.target(),
                    ReadInterpretation::Boot0Corrected,
                    &committed,
                )?
            } else {
                erased
            };
            // Also prove the untouched opposite SPL block and U-Boot still pass after mutation.
            let _after = preflight(operation, &committed)?;
            journal.verify(&readback, &committed)?;
            Ok(Response::SplTrialVerified {
                operation,
                readback: Box::new(readback),
            })
        })();
        if result.is_err() {
            journal.advance(Stage::Indeterminate, &committed)?;
        }
        result
    }
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
