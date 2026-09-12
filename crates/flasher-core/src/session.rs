//! One state machine for simulated operations and fail-closed physical discovery.
use crate::{
    Cancellation, Error,
    assets::VerifiedAsset,
    device::{Device, Nand, RealFel, select},
    manifest::{Manifest, Role},
};
use std::time::{Duration, Instant};

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
}
#[derive(Debug, Clone)]
pub struct Event {
    pub stage: Stage,
    pub message: String,
    pub done: u64,
    pub total: u64,
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
    fn identify(&mut self, device: &Device, cancel: &Cancellation) -> Result<Nand, Error>;
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
    fn identify(&mut self, _device: &Device, _cancel: &Cancellation) -> Result<Nand, Error> {
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
    pub nand: Option<Nand>,
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
            nand: Some(Nand::Hynix),
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
    fn identify(&mut self, _device: &Device, cancel: &Cancellation) -> Result<Nand, Error> {
        self.operation(Operation::Identify, cancel)?;
        self.nand.ok_or(Error::Device)
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
    nand: Option<Nand>,
    manifest: Option<Manifest>,
    assets: Vec<VerifiedAsset>,
    prepared_at: Option<Instant>,
}
impl<B: Backend> Session<B> {
    #[must_use]
    pub const fn new(backend: B) -> Self {
        Self {
            backend,
            stage: Stage::Welcome,
            device: None,
            nand: None,
            manifest: None,
            assets: Vec::new(),
            prepared_at: None,
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
    pub const fn nand(&self) -> Option<Nand> {
        self.nand
    }
    /// Moves through discovery and checks every verified payload before offering erase.
    /// # Errors
    /// Any invalid inventory, unapproved physical image or unknown device enters recovery.
    pub fn preflight(
        &mut self,
        manifest: Manifest,
        mut assets: Vec<VerifiedAsset>,
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
            let nand = self.backend.identify(&device, cancel)?;
            if assets.len() != manifest.assets().len() {
                return Err(Error::Manifest("incomplete verified inventory"));
            }
            for (asset, spec) in assets.iter_mut().zip(manifest.assets()) {
                if !asset.matches(spec) {
                    return Err(Error::Manifest(
                        "asset inventory differs from selected release",
                    ));
                }
                asset.recheck(cancel)?;
            }
            self.device = Some(device);
            self.nand = Some(nand);
            self.manifest = Some(manifest);
            self.assets = assets;
            self.prepared_at = Some(Instant::now());
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
            self.manifest.as_ref()?.release(),
            self.manifest.as_ref()?.digest()
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
            if self
                .prepared_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(300))
            {
                return Err(Error::Timeout);
            }
            let device = select(&self.backend.discover(cancel)?)?;
            if Some(&device) != self.device.as_ref()
                || Some(self.backend.identify(&device, cancel)?) != self.nand
            {
                return Err(Error::Device);
            }
            self.report(
                Stage::Download,
                "Rechecking verified snapshots immediately before recovery boot",
                &mut emit,
            );
            for asset in &mut self.assets {
                asset.recheck(cancel)?;
            }
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
                .assets
                .iter_mut()
                .find(|a| a.role() == Role::Rootfs)
                .ok_or(Error::State)?;
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
            self.assets.clear();
            Ok(())
        })();
        self.finish_error(result, &mut emit)
    }
    fn report(&mut self, stage: Stage, message: &str, emit: &mut impl FnMut(Event)) {
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
            self.assets.clear();
            self.prepared_at = None;
            self.report(Stage::Recovery, &error.to_string(), emit);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assets::Cache, simulation};
    fn ready(mock: MockFel) -> Result<(Session<MockFel>, tempfile::TempDir), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let c = Cancellation::default();
        let (manifest, assets) = simulation::prepare(&cache, &c)?;
        let mut session = Session::new(mock);
        session.preflight(manifest, assets, &c, |_| {})?;
        Ok((session, dir))
    }
    #[test]
    fn success_verifies_without_automatic_reboot() -> Result<(), Error> {
        for nand in [Nand::Hynix, Nand::Toshiba] {
            let (mut s, _dir) = ready(MockFel {
                nand: Some(nand),
                ..Default::default()
            })?;
            s.install(
                &s.confirmation().ok_or(Error::State)?,
                &Cancellation::default(),
                |_| {},
            )?;
            assert_eq!(s.stage(), Stage::Complete);
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
    fn stock_confirmation_cannot_authorize_vitrallis_default() -> Result<(), Error> {
        let (stock, _stock_dir) = ready(MockFel::default())?;
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let cancel = Cancellation::default();
        let (manifest, assets) = simulation::prepare_profile(
            &cache,
            &cancel,
            crate::profile::Profile::VitrallisDefault,
        )?;
        let mut shell = Session::new(MockFel::default());
        shell.preflight(manifest, assets, &cancel, |_| {})?;
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
        assert!(!s.backend().operations.contains(&Operation::Erase));
        assert!(
            s.install("anything", &Cancellation::default(), |_| {})
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn expired_confirmation_cannot_erase() -> Result<(), Error> {
        let (mut s, _dir) = ready(MockFel::default())?;
        s.prepared_at = Instant::now().checked_sub(Duration::from_secs(301));
        assert!(matches!(
            s.install(
                &s.confirmation().ok_or(Error::State)?,
                &Cancellation::default(),
                |_| {}
            ),
            Err(Error::Timeout)
        ));
        assert!(!s.backend().operations.contains(&Operation::Erase));
        Ok(())
    }
    #[test]
    fn changed_device_or_nand_cannot_erase() -> Result<(), Error> {
        for change in 0..3 {
            let (mut s, _dir) = ready(MockFel::default())?;
            match change {
                0 => s.backend.devices[0].address = 3,
                1 => s.backend.nand = Some(Nand::Toshiba),
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
            assert_eq!(s.backend().operations.last(), Some(&operation));
            assert!(s.confirmation().is_none());
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
    fn unknown_nand_fails_before_confirmation() {
        assert!(
            ready(MockFel {
                nand: None,
                ..Default::default()
            })
            .is_err()
        );
    }
    #[test]
    fn incomplete_assets_cannot_pass_preflight() -> Result<(), Error> {
        let dir = crate::assets::temporary_directory()?;
        let cache = Cache::open(dir.path())?;
        let c = Cancellation::default();
        let (m, mut a) = simulation::prepare(&cache, &c)?;
        a.pop();
        let mut s = Session::new(MockFel::default());
        assert!(s.preflight(m, a, &c, |_| {}).is_err());
        assert_eq!(s.stage(), Stage::Recovery);
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
