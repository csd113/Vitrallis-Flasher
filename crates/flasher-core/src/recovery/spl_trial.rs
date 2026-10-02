//! Two-phase, single-connection original-SPL trial with durable host intent.
use super::{Channel, Request, Response};
use crate::{
    Cancellation, Error,
    boot_trial::{
        self, Operation,
        journal::{Context, Journal, Stage},
    },
    recovery::BootReadback,
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{Duration, Instant},
};

const LIFETIME: Duration = Duration::from_secs(30);

/// Fixed restoration and backup bindings checked before the host dispatches.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Prepared {
    pub operation: Operation,
    pub token: [u8; 32],
    pub restoration_sha256: String,
    pub original_spl_sha256: String,
    pub original_uboot_sha256: String,
}
impl Prepared {
    /// Checks exact known candidate and backup chain; this is not release approval.
    /// # Errors
    /// Rejects a wrong operation, zero token or changed digest.
    pub fn validate(&self, operation: Operation) -> Result<(), Error> {
        if self.operation != operation
            || self.token == [0; 32]
            || self.restoration_sha256 != boot_trial::RESTORATION_IMAGE
            || self.original_spl_sha256 != boot_trial::ORIGINAL_SPL
            || self.original_uboot_sha256 != boot_trial::ORIGINAL_UBOOT
        {
            return Err(Error::Recovery);
        }
        Ok(())
    }
}

/// Device-local, expiring, single-use authority. Keep it inside one connection.
/// Consuming it before checking also consumes malformed/stale execute attempts.
#[derive(Debug)]
pub struct Ticket {
    operation: Operation,
    token: [u8; 32],
    issued: Instant,
}
impl Ticket {
    /// Creates a fresh ticket only after device-local preflight and snapshot validation.
    /// # Errors
    /// Returns operating-system random-source failure.
    pub fn issue(operation: Operation) -> Result<(Self, Prepared), Error> {
        let mut token = [0; 32];
        super::random(&mut token)?;
        let ticket = Self {
            operation,
            token,
            issued: Instant::now(),
        };
        let prepared = Prepared {
            operation,
            token,
            restoration_sha256: boot_trial::RESTORATION_IMAGE.into(),
            original_spl_sha256: boot_trial::ORIGINAL_SPL.into(),
            original_uboot_sha256: boot_trial::ORIGINAL_UBOOT.into(),
        };
        Ok((ticket, prepared))
    }
    /// Consumes the one prepared operation; callers must recheck local hardware afterward.
    /// # Errors
    /// Rejects a changed token/operation or an expired preparation.
    pub fn consume(self, operation: Operation, token: &[u8; 32]) -> Result<(), Error> {
        self.check(operation, token, self.issued.elapsed())
    }
    fn check(
        &self,
        operation: Operation,
        token: &[u8; 32],
        elapsed: Duration,
    ) -> Result<(), Error> {
        // This token is already inside an authenticated frame. Constant-time HMAC
        // protects the transport; token equality grants no unauthenticated access.
        if operation != self.operation || token != &self.token || elapsed >= LIFETIME {
            return Err(Error::Recovery);
        }
        Ok(())
    }
}

pub(super) fn verify(operation: Operation, readback: &BootReadback) -> Result<(), Error> {
    match operation {
        Operation::ErasePrimary => boot_trial::verify_erased_primary(readback),
        Operation::RestorePrimary => {
            boot_trial::verify_spl(readback, super::BootRegion::SplPrimary)
        }
    }
}

/// Checks the live device and restoration snapshot, then disconnects without dispatch.
/// The preparation ticket dies with this connection; this operation cannot write NAND.
/// # Errors
/// Rejects unsafe identity files, failed authentication or device-local preconditions.
pub fn prepare(
    config: &Path,
    binary: &Path,
    operation: Operation,
    cancel: &Cancellation,
) -> Result<Response, Error> {
    super::diagnostic_request(
        config,
        binary,
        &Request::PrepareSplTrial { operation },
        cancel,
    )
}

/// Runs the fixed diagnostic through authenticated recovery, never automatic retry.
///
/// The journal must be a new absolute path in a private directory. Cancellation
/// after dispatch is indeterminate: the device completes its committed operation.
/// # Errors
/// Rejects unsafe files, wrong target, failed preparation/authentication, cancellation,
/// invalid readback or journal failure. A lost execute response is never success.
pub fn run(
    config: &Path,
    binary: &Path,
    operation: Operation,
    path: &Path,
    cancel: &Cancellation,
) -> Result<Response, Error> {
    let (credentials, implementation) = super::diagnostic_identity(config, binary)?;
    let context = Context {
        sid: credentials.sid,
        session: credentials.session,
        daemon_sha256: implementation,
        operation,
    };
    let mut journal = Journal::create(path, context, cancel)?;
    let address = std::net::SocketAddr::from(([192, 168, 81, 1], 3333));
    let mut channel = match Channel::connect(address, &credentials, &implementation, cancel) {
        Ok(channel) => channel,
        Err(error) => {
            journal.advance(failure_stage(false, &error), cancel)?;
            return Err(error);
        }
    };
    perform(
        &mut journal,
        operation,
        |request| channel.request(request, cancel),
        cancel,
    )
}

fn perform(
    journal: &mut Journal,
    operation: Operation,
    mut request: impl FnMut(&Request) -> Result<Response, Error>,
    cancel: &Cancellation,
) -> Result<Response, Error> {
    let mut dispatched = false;
    let result = (|| {
        let Response::SplTrialPrepared(prepared) =
            request(&Request::PrepareSplTrial { operation })?
        else {
            return Err(Error::Recovery);
        };
        prepared.validate(operation)?;
        journal.advance(Stage::Prepared, cancel)?;
        journal.advance(Stage::Dispatched, cancel)?;
        dispatched = true;
        let response = request(&Request::ExecuteSplTrial {
            operation,
            token: prepared.token,
        })?;
        let Response::SplTrialVerified {
            operation: reported,
            readback,
        } = &response
        else {
            return Err(Error::Recovery);
        };
        if *reported != operation {
            return Err(Error::Recovery);
        }
        journal.verify(readback, cancel)?;
        Ok(response)
    })();
    if let Err(error) = &result {
        let stage = failure_stage(dispatched, error);
        // If fsync fails, preserve the last durable stage and return that failure.
        journal.advance(stage, cancel)?;
    }
    result
}

const fn failure_stage(dispatched: bool, error: &Error) -> Stage {
    if dispatched {
        Stage::Indeterminate
    } else if matches!(error, Error::Cancelled) {
        Stage::CancelledBeforeDispatch
    } else {
        Stage::FailedBeforeDispatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn erased() -> BootReadback {
        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-primary-erase-trial-8.json"
        ))
        .unwrap();
        serde_json::from_value(evidence["response"]["SplTrialVerified"]["readback"].clone())
            .unwrap()
    }

    #[test]
    fn cancellation_before_dispatch_never_sends_execute_and_journal_is_terminal() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("trial.jsonl");
        let cancel = Cancellation::default();
        let context = Context {
            sid: boot_trial::SID,
            session: [1; 16],
            daemon_sha256: [2; 32],
            operation: Operation::ErasePrimary,
        };
        let mut journal = Journal::create(&path, context, &cancel).unwrap();
        let mut calls = 0;
        let result = perform(
            &mut journal,
            Operation::ErasePrimary,
            |request| {
                calls += 1;
                assert!(matches!(request, Request::PrepareSplTrial { .. }));
                let (_, prepared) = Ticket::issue(Operation::ErasePrimary).unwrap();
                cancel.cancel();
                Ok(Response::SplTrialPrepared(prepared))
            },
            &cancel,
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(calls, 1);
        assert_eq!(
            boot_trial::journal::read(&path, &Cancellation::default())
                .unwrap()
                .entries
                .last()
                .unwrap()
                .stage,
            Stage::CancelledBeforeDispatch
        );
    }

    #[test]
    fn dispatch_loss_or_bad_readback_is_indeterminate_without_retry() {
        for outcome in 0..4 {
            let directory = crate::assets::temporary_directory().unwrap();
            let path = directory.path().join("trial.jsonl");
            let cancel = Cancellation::default();
            let context = Context {
                sid: boot_trial::SID,
                session: [1; 16],
                daemon_sha256: [2; 32],
                operation: Operation::ErasePrimary,
            };
            let mut journal = Journal::create(&path, context, &cancel).unwrap();
            let mut calls = 0;
            let result = perform(
                &mut journal,
                Operation::ErasePrimary,
                |request| {
                    calls += 1;
                    if calls == 1 {
                        assert!(matches!(request, Request::PrepareSplTrial { .. }));
                        let (_, prepared) = Ticket::issue(Operation::ErasePrimary).unwrap();
                        return Ok(Response::SplTrialPrepared(prepared));
                    }
                    assert!(matches!(request, Request::ExecuteSplTrial { .. }));
                    match outcome {
                        0 => Err(Error::Timeout),
                        1 => {
                            let mut readback = erased();
                            readback.data_sha256 = "0".repeat(64);
                            Ok(Response::SplTrialVerified {
                                operation: Operation::ErasePrimary,
                                readback: Box::new(readback),
                            })
                        }
                        2 => {
                            cancel.cancel();
                            Ok(Response::SplTrialVerified {
                                operation: Operation::ErasePrimary,
                                readback: Box::new(erased()),
                            })
                        }
                        _ => Ok(Response::SplTrialVerified {
                            operation: Operation::RestorePrimary,
                            readback: Box::new(erased()),
                        }),
                    }
                },
                &cancel,
            );
            assert!(result.is_err());
            assert_eq!(calls, 2);
            assert_eq!(
                boot_trial::journal::read(&path, &Cancellation::default())
                    .unwrap()
                    .entries
                    .last()
                    .unwrap()
                    .stage,
                Stage::Indeterminate
            );
        }
    }

    #[test]
    fn only_exact_checked_readback_records_verified() {
        let directory = crate::assets::temporary_directory().unwrap();
        let path = directory.path().join("trial.jsonl");
        let cancel = Cancellation::default();
        let context = Context {
            sid: boot_trial::SID,
            session: [1; 16],
            daemon_sha256: [2; 32],
            operation: Operation::ErasePrimary,
        };
        let mut journal = Journal::create(&path, context, &cancel).unwrap();
        let result = perform(
            &mut journal,
            Operation::ErasePrimary,
            |request| match request {
                Request::PrepareSplTrial { operation } => Ok(Response::SplTrialPrepared(
                    Ticket::issue(*operation).unwrap().1,
                )),
                Request::ExecuteSplTrial { operation, .. } => Ok(Response::SplTrialVerified {
                    operation: *operation,
                    readback: Box::new(erased()),
                }),
                _ => Err(Error::State),
            },
            &cancel,
        );
        assert!(result.is_ok());
        assert_eq!(
            boot_trial::journal::read(&path, &cancel)
                .unwrap()
                .entries
                .last()
                .unwrap()
                .stage,
            Stage::Verified
        );
    }

    #[test]
    fn tickets_are_fresh_bound_expiring_and_prepared_pins_are_fixed() {
        let (ticket, mut prepared) = Ticket::issue(Operation::ErasePrimary).unwrap();
        let (_, other) = Ticket::issue(Operation::ErasePrimary).unwrap();
        assert_ne!(prepared.token, other.token);
        prepared.validate(Operation::ErasePrimary).unwrap();
        assert!(
            ticket
                .check(Operation::RestorePrimary, &prepared.token, Duration::ZERO)
                .is_err()
        );
        assert!(
            ticket
                .check(Operation::ErasePrimary, &other.token, Duration::ZERO)
                .is_err()
        );
        assert!(
            ticket
                .check(Operation::ErasePrimary, &prepared.token, LIFETIME)
                .is_err()
        );
        ticket
            .consume(Operation::ErasePrimary, &prepared.token)
            .unwrap();
        prepared.restoration_sha256.replace_range(..1, "0");
        assert!(prepared.validate(Operation::ErasePrimary).is_err());
        prepared.restoration_sha256 = boot_trial::RESTORATION_IMAGE.into();
        prepared.token = [0; 32];
        assert!(prepared.validate(Operation::ErasePrimary).is_err());
    }
}
