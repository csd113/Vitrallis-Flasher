//! Mockable FEL transport boundary for `PocketCHIP` recovery work.
//!
//! Batch 2 supplied [`UnavailableFel`], which returns `FelUnavailable` for every
//! operation. Batch 3 adds bounded [`crate::fel_native::NativeFel`] diagnostics;
//! recovery boot and NAND authorization remain gated. Tests use
//! [`ScriptedFel`], which records exact calls and returns deterministic
//! responses or injected errors without touching USB.
use crate::{
    Cancellation, Error,
    device::{Device, DeviceInfo, TargetInfo},
    script::Scripted,
};
use sha2::{Digest, Sha256};
use std::{
    fmt::{Debug, Formatter},
    sync::{Mutex, MutexGuard, PoisonError},
};

/// One FEL operation with the exact target and arguments it was invoked with.
#[derive(Clone, PartialEq, Eq)]
pub enum FelCall {
    Discover,
    Identify {
        device: Device,
    },
    DeviceInfo {
        device: Device,
    },
    UploadRam {
        device: Device,
        address: u32,
        data: Vec<u8>,
    },
    Execute {
        device: Device,
        address: u32,
    },
    ReadMemory {
        device: Device,
        address: u32,
        length: u32,
    },
    ReadStatus {
        device: Device,
    },
}
impl FelCall {
    #[must_use]
    pub fn identify(device: &Device) -> Self {
        Self::Identify {
            device: device.clone(),
        }
    }
    #[must_use]
    pub fn device_info(device: &Device) -> Self {
        Self::DeviceInfo {
            device: device.clone(),
        }
    }
    #[must_use]
    pub fn upload(device: &Device, address: u32, data: &[u8]) -> Self {
        Self::UploadRam {
            device: device.clone(),
            address,
            data: data.to_vec(),
        }
    }
    #[must_use]
    pub fn execute(device: &Device, address: u32) -> Self {
        Self::Execute {
            device: device.clone(),
            address,
        }
    }
    #[must_use]
    pub fn read_memory(device: &Device, address: u32, length: u32) -> Self {
        Self::ReadMemory {
            device: device.clone(),
            address,
            length,
        }
    }
    #[must_use]
    pub fn read_status(device: &Device) -> Self {
        Self::ReadStatus {
            device: device.clone(),
        }
    }
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Identify { .. } => "identify",
            Self::DeviceInfo { .. } => "device-info",
            Self::UploadRam { .. } => "upload-ram",
            Self::Execute { .. } => "execute",
            Self::ReadMemory { .. } => "read-memory",
            Self::ReadStatus { .. } => "read-status",
        }
    }
}
impl Debug for FelCall {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discover => formatter.write_str("Discover"),
            Self::Identify { device } => formatter
                .debug_struct("Identify")
                .field("device", device)
                .finish(),
            Self::DeviceInfo { device } => formatter
                .debug_struct("DeviceInfo")
                .field("device", device)
                .finish(),
            Self::UploadRam {
                device,
                address,
                data,
            } => formatter
                .debug_struct("UploadRam")
                .field("device", device)
                .field("address", &format!("{address:#010x}"))
                .field("length", &data.len())
                .field("sha256", &format!("{:x}", Sha256::digest(data)))
                .finish(),
            Self::Execute { device, address } => formatter
                .debug_struct("Execute")
                .field("device", device)
                .field("address", &format!("{address:#010x}"))
                .finish(),
            Self::ReadMemory {
                device,
                address,
                length,
            } => formatter
                .debug_struct("ReadMemory")
                .field("device", device)
                .field("address", &format!("{address:#010x}"))
                .field("length", length)
                .finish(),
            Self::ReadStatus { device } => formatter
                .debug_struct("ReadStatus")
                .field("device", device)
                .finish(),
        }
    }
}

/// A scripted reply for one FEL call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FelReply {
    Devices(Vec<Device>),
    Target(TargetInfo),
    Info(DeviceInfo),
    Data(Vec<u8>),
    Status(u32),
    Ack,
}

/// The operations a future `PocketCHIP` FEL implementation must provide.
pub trait FelTransport: Send + Sync + Debug {
    /// # Errors
    /// Returns discovery failures; never filters a malformed candidate list.
    fn discover(&self, cancel: &Cancellation) -> Result<Vec<Device>, Error>;
    /// # Errors
    /// Returns identity failures; A13/R8 alone must not prove `PocketCHIP`.
    fn identify(&self, device: &Device, cancel: &Cancellation) -> Result<TargetInfo, Error>;
    /// # Errors
    /// Returns read-only device query failures.
    fn device_info(&self, device: &Device, cancel: &Cancellation) -> Result<DeviceInfo, Error>;
    /// # Errors
    /// Returns upload failures; Batch 2 never calls this against hardware.
    fn upload_to_ram(
        &self,
        device: &Device,
        address: u32,
        data: &[u8],
        cancel: &Cancellation,
    ) -> Result<(), Error>;
    /// # Errors
    /// Returns execution failures; Batch 2 never calls this against hardware.
    fn execute(&self, device: &Device, address: u32, cancel: &Cancellation) -> Result<(), Error>;
    /// # Errors
    /// Returns read failures or malformed/truncated replies.
    fn read_memory(
        &self,
        device: &Device,
        address: u32,
        length: u32,
        cancel: &Cancellation,
    ) -> Result<Vec<u8>, Error>;
    /// # Errors
    /// Returns status-read failures.
    fn read_status(&self, device: &Device, cancel: &Cancellation) -> Result<u32, Error>;
}

/// Production default: an explicit refusal, not a hidden USB path.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableFel;
impl FelTransport for UnavailableFel {
    fn discover(&self, _cancel: &Cancellation) -> Result<Vec<Device>, Error> {
        Err(Error::FelUnavailable)
    }
    fn identify(&self, _device: &Device, _cancel: &Cancellation) -> Result<TargetInfo, Error> {
        Err(Error::FelUnavailable)
    }
    fn device_info(&self, _device: &Device, _cancel: &Cancellation) -> Result<DeviceInfo, Error> {
        Err(Error::FelUnavailable)
    }
    fn upload_to_ram(
        &self,
        _device: &Device,
        _address: u32,
        _data: &[u8],
        _cancel: &Cancellation,
    ) -> Result<(), Error> {
        Err(Error::FelUnavailable)
    }
    fn execute(
        &self,
        _device: &Device,
        _address: u32,
        _cancel: &Cancellation,
    ) -> Result<(), Error> {
        Err(Error::FelUnavailable)
    }
    fn read_memory(
        &self,
        _device: &Device,
        _address: u32,
        _length: u32,
        _cancel: &Cancellation,
    ) -> Result<Vec<u8>, Error> {
        Err(Error::FelUnavailable)
    }
    fn read_status(&self, _device: &Device, _cancel: &Cancellation) -> Result<u32, Error> {
        Err(Error::FelUnavailable)
    }
}

/// Deterministic transport for tests: ordered expectations and terminal errors.
#[derive(Debug)]
pub struct ScriptedFel {
    script: Mutex<Scripted<FelCall, FelReply>>,
}
impl ScriptedFel {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            script: Mutex::new(Scripted::new()),
        }
    }
    pub fn expect_discover(&self, devices: Vec<Device>) {
        self.lock()
            .expect_ok(FelCall::Discover, FelReply::Devices(devices));
    }
    pub fn expect_identify(&self, device: &Device, info: TargetInfo) {
        self.lock()
            .expect_ok(FelCall::identify(device), FelReply::Target(info));
    }
    pub fn expect_device_info(&self, device: &Device, info: DeviceInfo) {
        self.lock()
            .expect_ok(FelCall::device_info(device), FelReply::Info(info));
    }
    pub fn expect_upload(&self, device: &Device, address: u32, data: &[u8]) {
        self.lock()
            .expect_ok(FelCall::upload(device, address, data), FelReply::Ack);
    }
    pub fn expect_execute(&self, device: &Device, address: u32) {
        self.lock()
            .expect_ok(FelCall::execute(device, address), FelReply::Ack);
    }
    pub fn expect_read_memory(&self, device: &Device, address: u32, data: Vec<u8>) {
        let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
        self.lock().expect_ok(
            FelCall::read_memory(device, address, length),
            FelReply::Data(data),
        );
    }
    pub fn expect_read_status(&self, device: &Device, status: u32) {
        self.lock()
            .expect_ok(FelCall::read_status(device), FelReply::Status(status));
    }
    pub fn inject_error(&self, call: FelCall, error: Error) {
        self.lock().expect_err(call, error);
    }
    #[must_use]
    pub fn calls(&self) -> Vec<FelCall> {
        self.lock().calls().to_vec()
    }
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.lock().remaining()
    }
    #[must_use]
    pub fn failed(&self) -> bool {
        self.lock().failed()
    }
    fn lock(&self) -> MutexGuard<'_, Scripted<FelCall, FelReply>> {
        self.script.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn check(&self, cancel: &Cancellation) -> Result<(), Error> {
        if let Err(cancelled) = cancel.check() {
            self.lock().poison();
            return Err(cancelled);
        }
        Ok(())
    }
}

fn reply<T>(
    result: Result<FelReply, Error>,
    extract: impl FnOnce(FelReply) -> Option<T>,
) -> Result<T, Error> {
    let response = result?;
    extract(response)
        .ok_or_else(|| Error::ScriptUnexpected("scripted reply has the wrong type".into()))
}
impl Default for ScriptedFel {
    fn default() -> Self {
        Self::new()
    }
}
impl FelTransport for ScriptedFel {
    fn discover(&self, cancel: &Cancellation) -> Result<Vec<Device>, Error> {
        self.check(cancel)?;
        reply(self.lock().call(FelCall::Discover), |reply| match reply {
            FelReply::Devices(devices) => Some(devices),
            _ => None,
        })
    }
    fn identify(&self, device: &Device, cancel: &Cancellation) -> Result<TargetInfo, Error> {
        self.check(cancel)?;
        reply(
            self.lock().call(FelCall::identify(device)),
            |reply| match reply {
                FelReply::Target(info) => Some(info),
                _ => None,
            },
        )
    }
    fn device_info(&self, device: &Device, cancel: &Cancellation) -> Result<DeviceInfo, Error> {
        self.check(cancel)?;
        reply(
            self.lock().call(FelCall::device_info(device)),
            |reply| match reply {
                FelReply::Info(info) => Some(info),
                _ => None,
            },
        )
    }
    fn upload_to_ram(
        &self,
        device: &Device,
        address: u32,
        data: &[u8],
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        self.check(cancel)?;
        reply(
            self.lock().call(FelCall::upload(device, address, data)),
            |reply| match reply {
                FelReply::Ack => Some(()),
                _ => None,
            },
        )
    }
    fn execute(&self, device: &Device, address: u32, cancel: &Cancellation) -> Result<(), Error> {
        self.check(cancel)?;
        reply(
            self.lock().call(FelCall::execute(device, address)),
            |reply| match reply {
                FelReply::Ack => Some(()),
                _ => None,
            },
        )
    }
    fn read_memory(
        &self,
        device: &Device,
        address: u32,
        length: u32,
        cancel: &Cancellation,
    ) -> Result<Vec<u8>, Error> {
        self.check(cancel)?;
        let data = reply(
            self.lock()
                .call(FelCall::read_memory(device, address, length)),
            |reply| match reply {
                FelReply::Data(data) => Some(data),
                _ => None,
            },
        )?;
        if data.len() != usize::try_from(length).unwrap_or(usize::MAX) {
            return Err(Error::Output);
        }
        Ok(data)
    }
    fn read_status(&self, device: &Device, cancel: &Cancellation) -> Result<u32, Error> {
        self.check(cancel)?;
        reply(
            self.lock().call(FelCall::read_status(device)),
            |reply| match reply {
                FelReply::Status(status) => Some(status),
                _ => None,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device() -> Device {
        Device {
            bus: 1,
            address: 2,
            soc: "A13".into(),
            sid: "01234567:89abcdef:01234567:89abcdef".into(),
        }
    }
    #[test]
    fn unavailable_transport_refuses_every_operation() {
        let transport = UnavailableFel;
        let cancel = Cancellation::default();
        assert!(matches!(
            transport.discover(&cancel),
            Err(Error::FelUnavailable)
        ));
        assert!(matches!(
            transport.identify(&device(), &cancel),
            Err(Error::FelUnavailable)
        ));
        assert!(matches!(
            transport.device_info(&device(), &cancel),
            Err(Error::FelUnavailable)
        ));
        assert!(matches!(
            transport.upload_to_ram(&device(), 0x4000_0000, b"x", &cancel),
            Err(Error::FelUnavailable)
        ));
        assert!(matches!(
            transport.execute(&device(), 0x4000_0000, &cancel),
            Err(Error::FelUnavailable)
        ));
        assert!(matches!(
            transport.read_memory(&device(), 0x4000_0000, 4, &cancel),
            Err(Error::FelUnavailable)
        ));
        assert!(matches!(
            transport.read_status(&device(), &cancel),
            Err(Error::FelUnavailable)
        ));
    }
    #[test]
    fn scripted_transport_records_exact_calls() -> Result<(), Error> {
        let transport = ScriptedFel::new();
        let device = device();
        transport.expect_discover(vec![device.clone()]);
        transport.expect_identify(&device, TargetInfo::fixture(crate::device::Nand::Hynix));
        transport.expect_device_info(
            &device,
            DeviceInfo {
                soc: "A13".into(),
                sid: device.sid.clone(),
                ram_bytes: 512 * 1024 * 1024,
            },
        );
        transport.expect_upload(&device, 0x4000_0000, b"payload");
        transport.expect_execute(&device, 0x4000_0000);
        transport.expect_read_memory(&device, 0x4000_0000, b"read".to_vec());
        transport.expect_read_status(&device, 0x1234);
        assert_eq!(
            transport.discover(&Cancellation::default())?,
            vec![device.clone()]
        );
        assert_eq!(
            transport
                .identify(&device, &Cancellation::default())?
                .nand_part,
            "H27UCG8T2ETR"
        );
        assert_eq!(
            transport
                .device_info(&device, &Cancellation::default())?
                .ram_bytes,
            512 * 1024 * 1024
        );
        transport.upload_to_ram(&device, 0x4000_0000, b"payload", &Cancellation::default())?;
        transport.execute(&device, 0x4000_0000, &Cancellation::default())?;
        assert_eq!(
            transport.read_memory(&device, 0x4000_0000, 4, &Cancellation::default())?,
            b"read"
        );
        assert_eq!(
            transport.read_status(&device, &Cancellation::default())?,
            0x1234
        );
        assert_eq!(transport.calls().len(), 7);
        assert_eq!(transport.remaining(), 0);
        Ok(())
    }
    #[test]
    fn scripted_error_is_terminal_and_calls_stop() {
        let transport = ScriptedFel::new();
        let device = device();
        transport.inject_error(FelCall::Discover, Error::NoDevice);
        transport.expect_identify(&device, TargetInfo::fixture(crate::device::Nand::Hynix));
        assert!(matches!(
            transport.discover(&Cancellation::default()),
            Err(Error::NoDevice)
        ));
        assert!(matches!(
            transport.identify(&device, &Cancellation::default()),
            Err(Error::ScriptUnexpected(_))
        ));
        assert_eq!(transport.calls().len(), 2);
        assert_eq!(transport.remaining(), 1);
    }
    #[test]
    fn truncated_read_memory_reply_is_rejected() {
        let transport = ScriptedFel::new();
        let device = device();
        transport.lock().expect_ok(
            FelCall::read_memory(&device, 0x1000, 8),
            FelReply::Data(vec![0; 4]),
        );
        assert!(matches!(
            transport.read_memory(&device, 0x1000, 8, &Cancellation::default()),
            Err(Error::Output)
        ));
    }
    #[test]
    fn wrong_arguments_are_reported() {
        let transport = ScriptedFel::new();
        let device = device();
        transport.expect_upload(&device, 0x4000_0000, b"one");
        assert!(matches!(
            transport.upload_to_ram(&device, 0x4000_1000, b"one", &Cancellation::default()),
            Err(Error::ScriptMismatch { .. })
        ));
    }
    #[test]
    fn cancellation_poisons_the_script() {
        let transport = ScriptedFel::new();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(transport.discover(&cancel), Err(Error::Cancelled)));
        assert!(transport.failed());
        assert!(matches!(
            transport.discover(&Cancellation::default()),
            Err(Error::ScriptUnexpected(_))
        ));
    }
}
