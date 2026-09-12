//! Host-independent, fail-closed `PocketCHIP` recovery services.
pub mod assets;
pub mod device;
pub mod manifest;
pub mod platform;
pub mod process;
pub mod profile;
pub mod releases;
pub mod session;
pub mod simulation;
mod transport;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Errors contain diagnostics, never manifest-controlled command text.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid manifest: {0}")]
    Manifest(&'static str),
    #[error("asset length differs from the manifest")]
    Length,
    #[error("asset SHA-256 differs from the manifest")]
    Hash,
    #[error("unsafe path: use a regular file in a private directory")]
    UnsafePath,
    #[error("operation cancelled; start a fresh preflight before retrying")]
    Cancelled,
    #[error("operation timed out")]
    Timeout,
    #[error("no accessible FEL device; check cable, jumper and USB permissions")]
    NoDevice,
    #[error("multiple FEL devices; disconnect every device except the intended PocketCHIP")]
    MultipleDevices,
    #[error("device identity or NAND is unknown, unsupported, or changed")]
    Device,
    #[error("physical recovery is blocked: no approved image and authenticated recovery protocol")]
    PhysicalBlocked,
    #[error("invalid operation for the current recovery state")]
    State,
    #[error("confirmation does not match the device and image")]
    Confirmation,
    #[error("external tool failed (exit {0:?}); see platform USB guidance")]
    Process(Option<i32>),
    #[error("external tool output exceeded its safety limit or was malformed")]
    Output,
    #[error("HTTPS transfer failed")]
    Network,
    #[error("filesystem or process I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Cloneable cooperative cancellation; each retry uses a fresh token.
#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    /// # Errors
    /// Returns `Cancelled` once cancellation is requested.
    pub fn check(&self) -> Result<(), Error> {
        if self.0.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
