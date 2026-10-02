//! One state machine for simulated operations and fail-closed physical discovery.
use crate::{
    Cancellation, Error,
    assets::VerifiedAssets,
    clock::{Clock, SystemClock},
    device::{Device, IdentifiedTarget, Nand, RealFel, TargetInfo, select},
    manifest::Role,
    nand::{self, NandPlan},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Default lifetime of a prepared confirmation.
pub const DEFAULT_CONFIRMATION_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Welcome,
    Fel,
    Detect,
    Preflight,
    ConfirmErase,
    Download,
    Flash,
    Verify,
    Complete,
    Recovery,
}
impl Stage {
    pub const ALL: [Self; 10] = [
        Self::Welcome,
        Self::Fel,
        Self::Detect,
        Self::Preflight,
        Self::ConfirmErase,
        Self::Download,
        Self::Flash,
        Self::Verify,
        Self::Complete,
        Self::Recovery,
    ];
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Fel => "Connect in FEL",
            Self::Detect => "Detect device",
            Self::Preflight => "Preflight",
            Self::ConfirmErase => "Confirm erase",
            Self::Download => "Verify assets",
            Self::Flash => "Install",
            Self::Verify => "Verify NAND",
            Self::Complete => "Complete",
            Self::Recovery => "Recovery",
        }
    }
    /// Monotonic milestone position; `Recovery` is terminal and never decreases
    /// relative to a failed operation's stage.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Welcome => 0,
            Self::Fel => 1,
            Self::Detect => 2,
            Self::Preflight => 3,
            Self::ConfirmErase => 4,
            Self::Download => 5,
            Self::Flash => 6,
            Self::Verify => 7,
            Self::Complete => 8,
            Self::Recovery => 9,
        }
    }
}
#[derive(Debug, Clone)]
pub struct Event {
    pub stage: Stage,
    pub message: String,
    pub done: u64,
    pub total: u64,
}
impl Event {
    /// Byte progress within the current stage-item, never an overall claim.
    #[must_use]
    pub fn percent(&self) -> Option<u32> {
        if self.total == 0 {
            return None;
        }
        let value = self.done.min(self.total).saturating_mul(100) / self.total;
        Some(u32::try_from(value).unwrap_or(100))
    }
}
/// Terminal state of one session attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Identify,
    BootRecovery,
    Erase,
    WriteBootloader,
    StreamRootfs,
    Verify,
    Reboot,
}

/// Explicit, testable session policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionConfig {
    pub confirmation_ttl: Duration,
}
impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            confirmation_ttl: DEFAULT_CONFIRMATION_TTL,
        }
    }
}
impl SessionConfig {
    #[must_use]
    pub const fn new(confirmation_ttl: Duration) -> Self {
        Self { confirmation_ttl }
    }
}

mod private {
    pub trait Sealed {}
}
/// Sealed boundary: only audited backends can participate in erase authorization.
pub trait Backend: private::Sealed {
    /// # Errors
    /// Returns discovery/USB errors.
    fn discover(&mut self, cancel: &Cancellation) -> Result<Vec<Device>, Error>;
    /// # Errors
    /// Requires board proof and known NAND before any destructive operation.
    fn identify(
        &mut self,
        device: &Device,
        cancel: &Cancellation,
    ) -> Result<IdentifiedTarget, Error>;
    fn simulated(&self) -> bool;
    /// # Errors
    /// Returns cancellation, transport or verification failure.
    fn operation(&mut self, operation: Operation, cancel: &Cancellation) -> Result<(), Error>;
}
impl private::Sealed for RealFel {}
impl Backend for RealFel {
    fn discover(&mut self, cancel: &Cancellation) -> Result<Vec<Device>, Error> {
        Self::discover(self, cancel)
    }
    fn identify(
        &mut self,
        _device: &Device,
        _cancel: &Cancellation,
    ) -> Result<IdentifiedTarget, Error> {
        Err(Error::PhysicalBlocked)
    }
    fn simulated(&self) -> bool {
        false
    }
    fn operation(&mut self, _operation: Operation, _cancel: &Cancellation) -> Result<(), Error> {
        Err(Error::PhysicalBlocked)
    }
}
/// Deterministic mock. Never holds a USB handle or invokes a command.
#[derive(Debug, Clone)]
pub struct MockFel {
    pub devices: Vec<Device>,
    pub target: Option<TargetInfo>,
    pub fail_at: Option<Operation>,
    pub operations: Vec<Operation>,
    pub delay: Duration,
}
impl Default for MockFel {
    fn default() -> Self {
        Self {
            devices: vec![Device {
                bus: 1,
                address: 1,
                soc: "A13".into(),
                sid: "01234567:89abcdef:01234567:89abcdef".into(),
            }],
            target: Some(TargetInfo::fixture(Nand::Hynix)),
            fail_at: None,
            operations: Vec::new(),
            delay: Duration::ZERO,
        }
    }
}
impl private::Sealed for MockFel {}
impl Backend for MockFel {
    fn discover(&mut self, cancel: &Cancellation) -> Result<Vec<Device>, Error> {
        cancel.check()?;
        Ok(self.devices.clone())
    }
    fn identify(
        &mut self,
        _device: &Device,
        cancel: &Cancellation,
    ) -> Result<IdentifiedTarget, Error> {
        self.operation(Operation::Identify, cancel)?;
        IdentifiedTarget::identify(self.target.clone().ok_or(Error::Device)?)
    }
    fn simulated(&self) -> bool {
        true
    }
    fn operation(&mut self, operation: Operation, cancel: &Cancellation) -> Result<(), Error> {
        let started = Instant::now();
        while started.elapsed() < self.delay {
            cancel.check()?;
            std::thread::sleep(Duration::from_millis(10));
        }
        cancel.check()?;
        self.operations.push(operation);
        if self.fail_at == Some(operation) {
            return Err(Error::Process(Some(1)));
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct Session<B> {
    backend: B,
    stage: Stage,
    device: Option<Device>,
    target: Option<IdentifiedTarget>,
    verified: Option<VerifiedAssets>,
    plan: Option<NandPlan>,
    prepared_at: Option<Instant>,
    config: SessionConfig,
    clock: Arc<dyn Clock>,
    outcome: Option<Outcome>,
}
impl<B: Backend> Session<B> {
    #[must_use]
    pub fn new(backend: B) -> Self {
        Self::with_config(backend, SessionConfig::default(), Arc::new(SystemClock))
    }
    #[must_use]
    pub fn with_config(backend: B, config: SessionConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            backend,
            stage: Stage::Welcome,
            device: None,
            target: None,
            verified: None,
            plan: None,
            prepared_at: None,
            config,
            clock,
            outcome: None,
        }
    }
    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.stage
    }
    #[must_use]
    pub const fn backend(&self) -> &B {
        &self.backend
    }
    #[must_use]
    pub fn nand(&self) -> Option<Nand> {
        self.target
            .as_ref()
            .map(crate::device::IdentifiedTarget::nand)
    }
    /// The review-only install plan built during preflight.
    #[must_use]
    pub const fn plan(&self) -> Option<&NandPlan> {
        self.plan.as_ref()
    }
    #[must_use]
    pub const fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }
    /// Moves through discovery and checks every verified payload before offering erase.
    /// # Errors
    /// Any invalid inventory, unapproved physical image or unknown device enters recovery.
    pub fn preflight(
        &mut self,
        verified: VerifiedAssets,
        cancel: &Cancellation,
        mut emit: impl FnMut(Event),
    ) -> Result<(), Error> {
        if self.stage != Stage::Welcome {
            return Err(Error::State);
        }
        let result = (|| {
            self.report(
                Stage::Fel,
                "FEL pin to GND, then connect a USB data cable",
                &mut emit,
            );
            cancel.check()?;
            self.report(Stage::Detect, "Checking all FEL candidates", &mut emit);
            let device = select(&self.backend.discover(cancel)?)?;
            self.report(
                Stage::Preflight,
                "Checking board, NAND and complete asset inventory",
                &mut emit,
            );
            // No manifest field, CLI option or mock device can enable real writes.
            if !self.backend.simulated() {
                return Err(Error::PhysicalBlocked);
            }
            let target = self.backend.identify(&device, cancel)?;
            let mut verified = verified;
            verified.recheck_all(cancel)?;
            let plan = nand::plan(&verified, &target)?;
            self.device = Some(device);
            self.target = Some(target);
            self.verified = Some(verified);
            self.plan = Some(plan);
            self.prepared_at = Some(self.clock.now());
            self.report(
                Stage::ConfirmErase,
                "All preflight checks passed in simulation; explicit confirmation required",
                &mut emit,
            );
            Ok(())
        })();
        self.finish_error(result, &mut emit)
    }
    /// Confirmation is bound to SID, release and the entire manifest digest.
    #[must_use]
    pub fn confirmation(&self) -> Option<String> {
        if self.stage != Stage::ConfirmErase {
            return None;
        }
        Some(format!(
            "ERASE {} {} {}",
            self.device.as_ref()?.sid,
            self.verified.as_ref()?.manifest().release(),
            self.verified.as_ref()?.manifest().digest()
        ))
    }
    /// Consumes the prepared session; never automatically retries or reboots a failed write.
    /// # Errors
    /// Wrong/stale confirmation, changed device/NAND, corrupt bytes, cancellation or backend failure enters recovery.
    pub fn install(
        &mut self,
        confirmation: &str,
        cancel: &Cancellation,
        mut emit: impl FnMut(Event),
    ) -> Result<(), Error> {
        if self.stage != Stage::ConfirmErase {
            return Err(Error::State);
        }
        let result = (|| {
            cancel.check()?;
            if self.confirmation().as_deref() != Some(confirmation) {
                return Err(Error::Confirmation);
            }
            let prepared = self.prepared_at.ok_or(Error::State)?;
            let elapsed = self
                .clock
                .now()
                .checked_duration_since(prepared)
                .ok_or(Error::Timeout)?;
            if elapsed > self.config.confirmation_ttl {
                return Err(Error::Timeout);
            }
            let device = select(&self.backend.discover(cancel)?)?;
            if Some(&device) != self.device.as_ref() {
                return Err(Error::Device);
            }
            let target = self.backend.identify(&device, cancel)?;
            if Some(&target) != self.target.as_ref() {
                return Err(Error::Device);
            }
            self.report(
                Stage::Download,
                "Rechecking verified snapshots immediately before recovery boot",
                &mut emit,
            );
            self.verified
                .as_mut()
                .ok_or(Error::State)?
                .recheck_all(cancel)?;
            self.report(
                Stage::Flash,
                "Booting LIVE recovery in simulated RAM",
                &mut emit,
            );
            self.backend.operation(Operation::BootRecovery, cancel)?;
            cancel.check()?;
            self.backend.operation(Operation::Erase, cancel)?;
            self.backend.operation(Operation::WriteBootloader, cancel)?;
            self.backend.operation(Operation::StreamRootfs, cancel)?;
            let rootfs = self
                .verified
                .as_mut()
                .ok_or(Error::State)?
                .asset_mut(Role::Rootfs)?;
            rootfs.stream(cancel, |done, total| {
                emit(Event {
                    stage: Stage::Flash,
                    message: "Streaming rootfs to simulated on-device UBIFS installer".into(),
                    done,
                    total,
                });
            })?;
            self.report(
                Stage::Verify,
                "Checking simulated persistent readback and clean unmount",
                &mut emit,
            );
            self.backend.operation(Operation::Verify, cancel)?;
            cancel.check()?;
            self.report(
                Stage::Complete,
                "Simulation complete. No hardware was written or rebooted.",
                &mut emit,
            );
            self.outcome = Some(Outcome::Success);
            self.verified = None;
            self.plan = None;
            self.prepared_at = None;
            Ok(())
        })();
        self.finish_error(result, &mut emit)
    }
    fn report(&mut self, stage: Stage, message: &str, emit: &mut impl FnMut(Event)) {
        debug_assert!(
            stage == Stage::Recovery || stage.index() >= self.stage.index(),
            "progress must not move backwards"
        );
        self.stage = stage;
        emit(Event {
            stage,
            message: message.into(),
            done: 0,
            total: 0,
        });
    }
    fn finish_error(
        &mut self,
        result: Result<(), Error>,
        emit: &mut impl FnMut(Event),
    ) -> Result<(), Error> {
        if let Err(error) = &result {
            self.verified = None;
            self.plan = None;
            self.prepared_at = None;
            self.outcome = Some(if matches!(error, Error::Cancelled) {
                Outcome::Cancelled
            } else {
                Outcome::Failure
            });
            self.report(Stage::Recovery, &error.to_string(), emit);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assets::Cache, clock::TestClock, profile::Profile, releases, simulation};

    fn ready(mock: MockFel) -> Result<(Session<MockFel>, tempfile::TempDir), Error> {
        let (session, dir, _clock) = ready_with_clock(mock, SessionConfig::default())?;
        Ok((session, dir))
    }
    fn ready_with_clock(
        mock: MockFel,
        config: SessionConfig,
    ) -> Result<(Session<MockFel>, tempfile::TempDir, Arc<TestClock>), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let cancel = Cancellation::default();
        let verified = simulation::prepare(&cache, &cancel)?;
        let clock = Arc::new(TestClock::new());
        let mut session = Session::with_config(mock, config, clock.clone());
        session.preflight(verified, &cancel, |_| {})?;
        Ok((session, dir, clock))
    }
    #[test]
    fn success_verifies_without_automatic_reboot() -> Result<(), Error> {
        for nand in [Nand::Hynix, Nand::Toshiba] {
            let (mut s, _dir) = ready(MockFel {
                target: Some(TargetInfo::fixture(nand)),
                ..Default::default()
            })?;
            s.install(
                &s.confirmation().ok_or(Error::State)?,
                &Cancellation::default(),
                |_| {},
            )?;
            assert_eq!(s.stage(), Stage::Complete);
            assert_eq!(s.outcome(), Some(Outcome::Success));
            assert_eq!(
                s.backend().operations,
                [
                    Operation::Identify,
                    Operation::Identify,
                    Operation::BootRecovery,
                    Operation::Erase,
                    Operation::WriteBootloader,
                    Operation::StreamRootfs,
                    Operation::Verify
                ]
            );
        }
        Ok(())
    }
    #[test]
    fn plan_is_review_only_and_uses_the_identified_spl_variant() -> Result<(), Error> {
        let (s, _dir) = ready(MockFel {
            target: Some(TargetInfo::fixture(Nand::Toshiba)),
            ..Default::default()
        })?;
        let plan = s.plan().ok_or(Error::State)?;
        assert!(!plan.is_executable());
        assert!(matches!(
            plan.authorize_execution(),
            Err(Error::PhysicalBlocked)
        ));
        let sources: Vec<_> = plan.steps().iter().filter_map(|step| step.source).collect();
        assert!(sources.contains(&Role::SplToshiba));
        assert!(!sources.contains(&Role::SplHynix));
        Ok(())
    }
    #[test]
    fn stock_confirmation_cannot_authorize_vitrallis_default() -> Result<(), Error> {
        let (stock, _stock_dir) = ready(MockFel::default())?;
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let cancel = Cancellation::default();
        let verified = simulation::prepare_profile(&cache, &cancel, Profile::VitrallisDefault)?;
        let mut shell = Session::new(MockFel::default());
        shell.preflight(verified, &cancel, |_| {})?;
        assert!(matches!(
            shell.install(&stock.confirmation().ok_or(Error::State)?, &cancel, |_| {}),
            Err(Error::Confirmation)
        ));
        assert!(!shell.backend().operations.contains(&Operation::Erase));
        assert!(shell.confirmation().is_none());
        Ok(())
    }
    #[test]
    fn confirmation_required_and_one_shot() -> Result<(), Error> {
        let (mut s, _dir) = ready(MockFel::default())?;
        assert!(matches!(
            s.install("ERASE", &Cancellation::default(), |_| {}),
            Err(Error::Confirmation)
        ));
        assert_eq!(s.stage(), Stage::Recovery);
        assert_eq!(s.outcome(), Some(Outcome::Failure));
        assert!(!s.backend().operations.contains(&Operation::Erase));
        assert!(
            s.install("anything", &Cancellation::default(), |_| {})
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn expiry_boundary_is_exact_and_expired_confirmation_cannot_erase() -> Result<(), Error> {
        let config = SessionConfig::new(Duration::from_secs(100));
        let (mut midway, _dir, clock) = ready_with_clock(MockFel::default(), config)?;
        clock.advance(Duration::from_secs(50));
        midway.install(
            &midway.confirmation().ok_or(Error::State)?,
            &Cancellation::default(),
            |_| {},
        )?;
        assert_eq!(midway.stage(), Stage::Complete);
        let (mut at_boundary, _dir, clock) = ready_with_clock(MockFel::default(), config)?;
        clock.advance(Duration::from_secs(100));
        at_boundary.install(
            &at_boundary.confirmation().ok_or(Error::State)?,
            &Cancellation::default(),
            |_| {},
        )?;
        assert_eq!(at_boundary.stage(), Stage::Complete);
        let (mut expired, _dir, clock) = ready_with_clock(MockFel::default(), config)?;
        clock.advance(Duration::from_secs(101));
        assert!(matches!(
            expired.install(
                &expired.confirmation().ok_or(Error::State)?,
                &Cancellation::default(),
                |_| {}
            ),
            Err(Error::Timeout)
        ));
        assert!(!expired.backend().operations.contains(&Operation::Erase));
        assert_eq!(expired.outcome(), Some(Outcome::Failure));
        Ok(())
    }
    #[test]
    fn changed_device_or_nand_cannot_erase() -> Result<(), Error> {
        for change in 0..3 {
            let (mut s, _dir) = ready(MockFel::default())?;
            match change {
                0 => s.backend.devices[0].address = 3,
                1 => s.backend.target = Some(TargetInfo::fixture(Nand::Toshiba)),
                _ => s.backend.devices.push(s.backend.devices[0].clone()),
            }
            assert!(
                s.install(
                    &s.confirmation().ok_or(Error::State)?,
                    &Cancellation::default(),
                    |_| {}
                )
                .is_err()
            );
            assert!(!s.backend().operations.contains(&Operation::Erase));
            assert!(s.plan().is_none());
        }
        Ok(())
    }
    #[test]
    fn cancellation_before_erase_invalidates_session() -> Result<(), Error> {
        let (mut s, _dir) = ready(MockFel::default())?;
        let c = Cancellation::default();
        assert!(matches!(
            s.install(&s.confirmation().ok_or(Error::State)?, &c, |event| {
                if event.stage == Stage::Flash {
                    c.cancel();
                }
            }),
            Err(Error::Cancelled)
        ));
        assert!(!s.backend().operations.contains(&Operation::Erase));
        assert_eq!(s.stage(), Stage::Recovery);
        assert_eq!(s.outcome(), Some(Outcome::Cancelled));
        Ok(())
    }
    #[test]
    fn cancellation_during_rootfs_streaming_stops_before_verify() -> Result<(), Error> {
        let (mut s, _dir) = ready(MockFel::default())?;
        let c = Cancellation::default();
        assert!(matches!(
            s.install(&s.confirmation().ok_or(Error::State)?, &c, |event| {
                if event.message.contains("Streaming") {
                    c.cancel();
                }
            }),
            Err(Error::Cancelled)
        ));
        assert!(
            s.backend().operations.contains(&Operation::StreamRootfs),
            "the stream was already underway"
        );
        assert!(!s.backend().operations.contains(&Operation::Verify));
        assert_eq!(s.stage(), Stage::Recovery);
        assert_eq!(s.outcome(), Some(Outcome::Cancelled));
        assert!(s.confirmation().is_none());
        Ok(())
    }
    #[test]
    fn cancellation_before_recovery_boot_runs_no_device_operation() -> Result<(), Error> {
        let (mut s, _dir) = ready(MockFel::default())?;
        let c = Cancellation::default();
        assert!(matches!(
            s.install(&s.confirmation().ok_or(Error::State)?, &c, |event| {
                if event.stage == Stage::Download {
                    c.cancel();
                }
            }),
            Err(Error::Cancelled)
        ));
        assert!(!s.backend().operations.contains(&Operation::BootRecovery));
        assert_eq!(s.outcome(), Some(Outcome::Cancelled));
        Ok(())
    }
    #[test]
    fn every_operation_failure_stops_and_requires_fresh_preflight() -> Result<(), Error> {
        for operation in [
            Operation::BootRecovery,
            Operation::Erase,
            Operation::WriteBootloader,
            Operation::StreamRootfs,
            Operation::Verify,
        ] {
            let (mut s, _dir) = ready(MockFel {
                fail_at: Some(operation),
                ..Default::default()
            })?;
            assert!(
                s.install(
                    &s.confirmation().ok_or(Error::State)?,
                    &Cancellation::default(),
                    |_| {}
                )
                .is_err()
            );
            assert_eq!(s.stage(), Stage::Recovery);
            assert_eq!(s.outcome(), Some(Outcome::Failure));
            assert_eq!(s.backend().operations.last(), Some(&operation));
            assert!(s.confirmation().is_none());
            assert!(s.plan().is_none());
        }
        let (mut retry, _dir) = ready(MockFel::default())?;
        retry.install(
            &retry.confirmation().ok_or(Error::State)?,
            &Cancellation::default(),
            |_| {},
        )?;
        Ok(())
    }
    #[test]
    fn progress_events_are_stage_monotonic_and_never_premature() -> Result<(), Error> {
        let (mut s, _dir) = ready(MockFel::default())?;
        let mut events = Vec::new();
        s.install(
            &s.confirmation().ok_or(Error::State)?,
            &Cancellation::default(),
            |event| events.push(event),
        )?;
        let mut last_index = 0;
        let mut last_done = 0;
        let mut stream_seen = 0;
        let mut completed = false;
        for event in &events {
            assert!(event.stage.index() >= last_index, "stage moved backwards");
            last_index = event.stage.index();
            assert!(
                event.done <= event.total,
                "byte progress exceeded its total"
            );
            if event.total > 0 {
                assert!(event.done >= last_done, "byte progress moved backwards");
                last_done = event.done;
            }
            if event.message.contains("Streaming") {
                stream_seen += 1;
                assert_eq!(event.percent(), Some(100), "item progress must terminate");
            }
            if event.stage == Stage::Complete {
                assert!(!completed, "complete was reported twice");
                completed = true;
            }
            if completed {
                assert_eq!(event.stage, Stage::Complete);
            }
        }
        assert!(completed);
        assert_eq!(
            events.last().map(|event| event.stage),
            Some(Stage::Complete)
        );
        assert!(events.iter().all(|event| event.stage != Stage::Recovery));
        assert_eq!(
            stream_seen, 1,
            "rootfs milestone must be reported exactly once"
        );
        Ok(())
    }
    #[test]
    fn failure_progress_never_reaches_a_success_stage() -> Result<(), Error> {
        let (mut s, _dir) = ready(MockFel {
            fail_at: Some(Operation::WriteBootloader),
            ..Default::default()
        })?;
        let mut events = Vec::new();
        assert!(
            s.install(
                &s.confirmation().ok_or(Error::State)?,
                &Cancellation::default(),
                |event| events.push(event),
            )
            .is_err()
        );
        assert!(
            events
                .iter()
                .all(|event| event.stage != Stage::Complete && event.stage != Stage::Verify),
            "a failed operation must not emit later success stages"
        );
        assert_eq!(
            events.last().map(|event| event.stage),
            Some(Stage::Recovery)
        );
        Ok(())
    }
    #[test]
    fn unknown_nand_fails_before_confirmation() {
        let mut info = TargetInfo::fixture(Nand::Hynix);
        info.nand_part = "unknown".into();
        assert!(
            ready(MockFel {
                target: Some(info),
                ..Default::default()
            })
            .is_err()
        );
    }
    #[test]
    fn incomplete_assets_cannot_pass_preflight() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let cancel = Cancellation::default();
        let manifest = releases::select(releases::Channel::Simulation, "simulation-debian13")?;
        let mut raw = manifest
            .assets()
            .iter()
            .map(|asset| cache.import(asset, simulation::PAYLOAD, &cancel, |_, _| {}))
            .collect::<Result<Vec<_>, Error>>()?;
        raw.pop();
        assert!(VerifiedAssets::verify(manifest, raw, &cancel).is_err());
        let session = Session::new(MockFel::default());
        assert_eq!(session.stage(), Stage::Welcome);
        Ok(())
    }
    #[test]
    fn preflight_cannot_run_twice() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let cancel = Cancellation::default();
        let verified = simulation::prepare(&cache, &cancel)?;
        let mut session = Session::new(MockFel::default());
        session.preflight(verified, &cancel, |_| {})?;
        let again = simulation::prepare(&cache, &cancel)?;
        assert!(matches!(
            session.preflight(again, &cancel, |_| {}),
            Err(Error::State)
        ));
        Ok(())
    }
    #[test]
    fn physical_backend_is_unconditionally_blocked() -> Result<(), Error> {
        use crate::process::SunxiTool;
        let executable = std::fs::canonicalize(std::env::current_exe()?)?;
        let mut real = RealFel::new(SunxiTool::open(&executable)?);
        assert!(!real.simulated());
        for operation in [
            Operation::BootRecovery,
            Operation::Erase,
            Operation::WriteBootloader,
            Operation::StreamRootfs,
            Operation::Verify,
            Operation::Reboot,
        ] {
            assert!(matches!(
                real.operation(operation, &Cancellation::default()),
                Err(Error::PhysicalBlocked)
            ));
        }
        assert!(matches!(
            real.identify(&MockFel::default().devices[0], &Cancellation::default()),
            Err(Error::PhysicalBlocked)
        ));
        Ok(())
    }
}
