//! Authenticated, bounded recovery transport with a restricted original-SPL trial.
//!
//! A fresh secret is injected into a reviewed RAM-only image over FEL. Both
//! peers prove possession using role-separated HMAC-SHA256. The transcript
//! binds protocol, boot session, hardware SID, implementation hash and fresh
//! nonces. Every subsequent frame binds direction and monotonic sequence.
use crate::{Cancellation, Error};
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

const VERSION: u32 = 4;
const LIMIT: usize = 64 * 1024;
// Bounded device-local preflight, erase/write and checked readback may take longer
// than read-only diagnostics. Polling and cancellation remain at 500 ms.
const TIMEOUT: Duration = Duration::from_secs(60);

pub mod spl_trial;
const POLL: Duration = Duration::from_millis(500);
const HELLO_BYTES: usize = 132;
const MAGIC: &[u8; 8] = b"VTRREC04";

/// Ephemeral boot credentials. Debug output deliberately excludes the key.
pub struct Credentials {
    key: hmac::Key,
    sid: [u8; 16],
    session: [u8; 16],
}
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("sid", &self.sid)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}
impl Credentials {
    /// Creates credentials for exactly the freshly discovered SID.
    /// # Errors
    /// Rejects a zero SID or failure of the operating system random source.
    pub fn generate(sid: [u8; 16]) -> Result<(Self, [u8; 64]), Error> {
        let mut bytes = [0; 64];
        random(&mut bytes[..32])?;
        bytes[32..48].copy_from_slice(&sid);
        random(&mut bytes[48..])?;
        Ok((Self::parse(&bytes)?, bytes))
    }
    /// Parses the fixed-size private RAM-boot configuration.
    /// # Errors
    /// Rejects a wrong length, zero key, SID or session.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 64
            || bytes[..32] == [0; 32]
            || bytes[32..48] == [0; 16]
            || bytes[48..] == [0; 16]
        {
            return Err(Error::Recovery);
        }
        let sid = bytes[32..48].try_into().map_err(|_| Error::Recovery)?;
        let session = bytes[48..].try_into().map_err(|_| Error::Recovery)?;
        Ok(Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, &bytes[..32]),
            sid,
            session,
        })
    }
    /// Public trial-journal binding, excluding the session HMAC key.
    #[must_use]
    pub const fn trial_context(
        &self,
        implementation: [u8; 32],
        operation: crate::boot_trial::Operation,
    ) -> crate::boot_trial::journal::Context {
        crate::boot_trial::journal::Context {
            sid: self.sid,
            session: self.session,
            daemon_sha256: implementation,
            operation,
        }
    }
    /// Requires an independently read Linux nvmem SID before listening.
    /// # Errors
    /// Rejects another device, including byte-order differences.
    pub fn check_sid(&self, measured: &[u8]) -> Result<(), Error> {
        if measured == self.sid {
            Ok(())
        } else {
            Err(Error::Device)
        }
    }
}

/// Fixed reviewed diagnostics and RAM-only restart requests. No paths, commands or addresses exist.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Request {
    Ping,
    Inventory,
    BootReadback {
        region: BootRegion,
        interpretation: ReadInterpretation,
    },
    ReturnToFel,
    PrepareSplTrial {
        operation: crate::boot_trial::Operation,
    },
    ExecuteSplTrial {
        operation: crate::boot_trial::Operation,
        token: [u8; 32],
    },
}

/// Closed boot partitions. Their physical offsets are measured policy, not input.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum BootRegion {
    SplPrimary,
    SplBackup,
    UBoot,
    FourthBootBlock,
}
impl BootRegion {
    #[must_use]
    pub const fn index(self) -> u8 {
        match self {
            Self::SplPrimary => 0,
            Self::SplBackup => 1,
            Self::UBoot => 2,
            Self::FourthBootBlock => 3,
        }
    }
}
/// Raw data/OOB or the kernel's normal ECC interpretation.
/// Kernel ECC is reviewed only for U-Boot, not the special boot0 SPL layout.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum ReadInterpretation {
    Raw,
    KernelCorrected,
    Boot0Corrected,
}
impl ReadInterpretation {
    /// Validates the fixed reviewed ECC interpretation for this partition.
    /// # Errors
    /// Rejects ordinary kernel ECC on SPL or boot0 ECC on other partitions.
    pub const fn validate(self, region: BootRegion) -> Result<(), Error> {
        match (self, region) {
            (Self::Raw, _)
            | (Self::KernelCorrected, BootRegion::UBoot)
            | (Self::Boot0Corrected, BootRegion::SplPrimary | BootRegion::SplBackup) => Ok(()),
            _ => Err(Error::Device),
        }
    }
}
/// Fixed-size raw data/OOB readback evidence. It grants no write capability.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BootReadback {
    pub region: BootRegion,
    pub interpretation: ReadInterpretation,
    pub data_bytes: u64,
    pub oob_bytes: u64,
    pub data_sha256: String,
    pub oob_sha256: String,
    pub interleaved_sha256: String,
    pub first_data_bytes: Vec<u8>,
    pub erased_data_pages: Vec<u16>,
    pub erased_oob_pages: Vec<u16>,
    pub page_marker_bytes: Vec<[u8; 2]>,
    pub corrected_bits_before: u64,
    pub corrected_bits_after: u64,
    pub ecc_failures_before: u64,
    pub ecc_failures_after: u64,
    pub tool_stderr: String,
    pub spl_copies: Vec<crate::boot0::SplCopyReport>,
}

/// NAND facts reported by the running recovery kernel; not a write capability.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MtdInfo {
    pub index: u8,
    pub name: String,
    pub size: u64,
    pub erase_size: u64,
    pub page_size: u64,
    pub oob_size: u64,
    pub bad_blocks: u64,
    pub bbt_blocks: u64,
    pub ecc_strength: u64,
    pub ecc_step: u64,
    pub corrected_bits: u64,
    pub ecc_failures: u64,
    pub offset: u64,
}
/// Read-only evidence from the authenticated implementation.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub model: String,
    pub compatible: Vec<String>,
    pub kernel: String,
    pub command_line: String,
    pub memory_reg: Vec<u8>,
    pub mtd: Vec<MtdInfo>,
    pub boot_log: String,
    pub mtd_report: String,
    pub nanddump_help: String,
}
/// Responses remain typed and bounded before serialization/acceptance.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Response {
    Pong,
    Inventory(Box<Inventory>),
    BootReadback(Box<BootReadback>),
    RestartAccepted,
    SplTrialPrepared(spl_trial::Prepared),
    SplTrialVerified {
        operation: crate::boot_trial::Operation,
        readback: Box<BootReadback>,
    },
}

fn random(bytes: &mut [u8]) -> Result<(), Error> {
    SystemRandom::new().fill(bytes).map_err(|_| Error::Recovery)
}
fn tag(key: &hmac::Key, domain: &[u8], pieces: &[&[u8]]) -> hmac::Tag {
    let mut context = hmac::Context::with_key(key);
    context.update(domain);
    for piece in pieces {
        context.update(piece);
    }
    context.sign()
}
fn verify(key: &hmac::Key, domain: &[u8], pieces: &[&[u8]], received: &[u8]) -> Result<(), Error> {
    // ring's verification is constant-time. Reuse the same unambiguous byte
    // transcript as signing; all pre-frame components have fixed lengths.
    let mut bytes =
        Vec::with_capacity(domain.len() + pieces.iter().map(|p| p.len()).sum::<usize>());
    bytes.extend_from_slice(domain);
    for piece in pieces {
        bytes.extend_from_slice(piece);
    }
    hmac::verify(key, &bytes, received).map_err(|_| Error::Recovery)
}
fn configure(stream: &TcpStream) -> Result<(), Error> {
    stream.set_read_timeout(Some(POLL))?;
    stream.set_write_timeout(Some(POLL))?;
    stream.set_nodelay(true)?;
    Ok(())
}
fn transfer(
    mut bytes: &mut [u8],
    stream: &mut TcpStream,
    receive: bool,
    cancel: &Cancellation,
) -> Result<(), Error> {
    let started = Instant::now();
    while !bytes.is_empty() {
        cancel.check()?;
        if started.elapsed() >= TIMEOUT {
            return Err(Error::Timeout);
        }
        let result = if receive {
            stream.read(bytes)
        } else {
            stream.write(bytes)
        };
        match result {
            Ok(0) => return Err(Error::Recovery),
            Ok(count) => {
                bytes = &mut bytes[count..];
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    cancel.check()
}
fn send(stream: &mut TcpStream, data: &[u8], cancel: &Cancellation) -> Result<(), Error> {
    // Keep one bounded owned buffer so both transfer directions share the
    // same cancellation/deadline implementation without unsafe casts.
    transfer(&mut data.to_vec(), stream, false, cancel)
}
fn hello(
    credentials: &Credentials,
    implementation: &[u8; 32],
    nonce: &[u8; 32],
) -> [u8; HELLO_BYTES] {
    let mut bytes = [0; HELLO_BYTES];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..12].copy_from_slice(&VERSION.to_be_bytes());
    bytes[12..28].copy_from_slice(&credentials.session);
    bytes[28..44].copy_from_slice(&credentials.sid);
    bytes[44..76].copy_from_slice(implementation);
    bytes[76..108].copy_from_slice(nonce);
    // Reserved fixed bytes must remain zero; future versions cannot be
    // accepted by silently interpreting them as this protocol.
    bytes
}
fn check_hello(bytes: &[u8; HELLO_BYTES], expected: &[u8; HELLO_BYTES]) -> Result<(), Error> {
    if bytes[..76] != expected[..76] || bytes[108..] != [0; 24] || bytes[76..108] == [0; 32] {
        return Err(Error::Recovery);
    }
    Ok(())
}

/// One mutually authenticated connection. A reconnect requires fresh nonces;
/// no operation or sequence is replayed automatically.
pub struct Channel {
    stream: TcpStream,
    key: hmac::Key,
    transcript: [u8; 64],
    sequence: u64,
    client: bool,
}
impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("sequence", &self.sequence)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}
impl Channel {
    /// Connects to an explicit USB-network address, never DNS or a proxy.
    /// # Errors
    /// Rejects connection failures, cancellation and any authentication mismatch.
    pub fn connect(
        address: SocketAddr,
        credentials: &Credentials,
        implementation: &[u8; 32],
        cancel: &Cancellation,
    ) -> Result<Self, Error> {
        cancel.check()?;
        let stream = TcpStream::connect_timeout(&address, POLL)?;
        Self::handshake(stream, credentials, implementation, true, cancel)
    }
    /// Authenticates an accepted connection after independent SID validation.
    /// # Errors
    /// Rejects stale credentials, wrong implementation and malformed handshakes.
    pub fn accept(
        stream: TcpStream,
        credentials: &Credentials,
        implementation: &[u8; 32],
        cancel: &Cancellation,
    ) -> Result<Self, Error> {
        Self::handshake(stream, credentials, implementation, false, cancel)
    }
    fn handshake(
        mut stream: TcpStream,
        credentials: &Credentials,
        implementation: &[u8; 32],
        client: bool,
        cancel: &Cancellation,
    ) -> Result<Self, Error> {
        configure(&stream)?;
        let mut nonce = [0; 32];
        random(&mut nonce)?;
        let own = hello(credentials, implementation, &nonce);
        let mut peer = [0; HELLO_BYTES];
        let mut received_tag = [0; 32];
        let (client_hello, server_hello) = if client {
            send(&mut stream, &own, cancel)?;
            send(
                &mut stream,
                tag(&credentials.key, b"client-hello", &[&own]).as_ref(),
                cancel,
            )?;
            transfer(&mut peer, &mut stream, true, cancel)?;
            transfer(&mut received_tag, &mut stream, true, cancel)?;
            check_hello(&peer, &own)?;
            verify(
                &credentials.key,
                b"server-hello",
                &[&own, &peer],
                &received_tag,
            )?;
            send(
                &mut stream,
                tag(&credentials.key, b"client-confirm", &[&own, &peer]).as_ref(),
                cancel,
            )?;
            (own, peer)
        } else {
            transfer(&mut peer, &mut stream, true, cancel)?;
            transfer(&mut received_tag, &mut stream, true, cancel)?;
            check_hello(&peer, &own)?;
            verify(&credentials.key, b"client-hello", &[&peer], &received_tag)?;
            send(&mut stream, &own, cancel)?;
            send(
                &mut stream,
                tag(&credentials.key, b"server-hello", &[&peer, &own]).as_ref(),
                cancel,
            )?;
            transfer(&mut received_tag, &mut stream, true, cancel)?;
            verify(
                &credentials.key,
                b"client-confirm",
                &[&peer, &own],
                &received_tag,
            )?;
            (peer, own)
        };
        let mut transcript = [0; 64];
        transcript[..32].copy_from_slice(&client_hello[76..108]);
        transcript[32..].copy_from_slice(&server_hello[76..108]);
        Ok(Self {
            stream,
            key: credentials.key.clone(),
            transcript,
            sequence: 0,
            client,
        })
    }
    const fn domain(&self, outbound: bool) -> &'static [u8] {
        if self.client == outbound {
            b"client-frame"
        } else {
            b"server-frame"
        }
    }
    fn write<T: Serialize>(&mut self, value: &T, cancel: &Cancellation) -> Result<(), Error> {
        let payload = serde_json::to_vec(value).map_err(|_| Error::Recovery)?;
        if payload.is_empty() || payload.len() > LIMIT {
            return Err(Error::Recovery);
        }
        let length = u32::try_from(payload.len())
            .map_err(|_| Error::Length)?
            .to_be_bytes();
        let sequence = self.sequence.to_be_bytes();
        let signature = tag(
            &self.key,
            self.domain(true),
            &[&self.transcript, &sequence, &length, &payload],
        );
        send(&mut self.stream, &length, cancel)?;
        send(&mut self.stream, &sequence, cancel)?;
        send(&mut self.stream, &payload, cancel)?;
        send(&mut self.stream, signature.as_ref(), cancel)
    }
    fn read<T: for<'a> Deserialize<'a>>(&mut self, cancel: &Cancellation) -> Result<T, Error> {
        let mut length = [0; 4];
        let mut sequence = [0; 8];
        transfer(&mut length, &mut self.stream, true, cancel)?;
        transfer(&mut sequence, &mut self.stream, true, cancel)?;
        let count = usize::try_from(u32::from_be_bytes(length)).map_err(|_| Error::Length)?;
        if count == 0 || count > LIMIT || sequence != self.sequence.to_be_bytes() {
            return Err(Error::Recovery);
        }
        let mut payload = vec![0; count];
        let mut signature = [0; 32];
        transfer(&mut payload, &mut self.stream, true, cancel)?;
        transfer(&mut signature, &mut self.stream, true, cancel)?;
        verify(
            &self.key,
            self.domain(false),
            &[&self.transcript, &sequence, &length, &payload],
            &signature,
        )?;
        serde_json::from_slice(&payload).map_err(|_| Error::Recovery)
    }
    fn advance(&mut self) -> Result<(), Error> {
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Recovery)?;
        Ok(())
    }
    /// Runs one reviewed operation. Any failure consumes the channel; no retry occurs.
    /// # Errors
    /// Rejects cancellation, framing, authentication and response-type mismatch.
    pub fn request(&mut self, request: &Request, cancel: &Cancellation) -> Result<Response, Error> {
        if !self.client {
            return Err(Error::State);
        }
        if let Request::BootReadback {
            region,
            interpretation,
        } = request
        {
            interpretation.validate(*region)?;
        }
        let result = (|| {
            self.write(request, cancel)?;
            let response = self.read(cancel)?;
            match (&request, &response) {
                (Request::Ping, Response::Pong)
                | (Request::Inventory, Response::Inventory(_))
                | (Request::ReturnToFel, Response::RestartAccepted) => {}
                (Request::PrepareSplTrial { operation }, Response::SplTrialPrepared(prepared)) => {
                    prepared.validate(*operation)?;
                }
                (
                    Request::ExecuteSplTrial { operation, .. },
                    Response::SplTrialVerified {
                        operation: reported,
                        readback,
                    },
                ) if operation == reported => {
                    spl_trial::verify(*operation, readback)?;
                }
                (
                    Request::BootReadback {
                        region,
                        interpretation,
                    },
                    Response::BootReadback(report),
                ) if *region == report.region && *interpretation == report.interpretation => {
                    if *interpretation == ReadInterpretation::Boot0Corrected {
                        if report.spl_copies.len() != 4
                            || report.spl_copies.iter().enumerate().any(|(index, copy)| {
                                usize::from(copy.copy) != index
                                    || copy.header_bytes.len() != 32
                                    || copy.corrected_bits.iter().any(|bits| *bits > 64)
                            })
                        {
                            return Err(Error::Recovery);
                        }
                    } else if !report.spl_copies.is_empty() {
                        return Err(Error::Recovery);
                    }
                }
                _ => return Err(Error::Recovery),
            }
            self.advance()?;
            Ok(response)
        })();
        if result.is_err() {
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
        result
    }
    /// Serves NAND read-only diagnostics. A successful return requests a RAM-only
    /// restart; the device must check that NAND/UBI is unmounted before reboot.
    /// # Errors
    /// Returns a protocol, authentication, I/O or inventory error.
    pub fn serve(
        &mut self,
        mut inventory: impl FnMut() -> Result<Inventory, Error>,
        mut readback: impl FnMut(BootRegion, ReadInterpretation) -> Result<BootReadback, Error>,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        self.serve_reviewed(
            |request| match request {
                Request::Ping => Ok(Response::Pong),
                Request::Inventory => Ok(Response::Inventory(Box::new(inventory()?))),
                Request::ReturnToFel => Ok(Response::RestartAccepted),
                Request::BootReadback {
                    region,
                    interpretation,
                } => {
                    interpretation.validate(region)?;
                    Ok(Response::BootReadback(Box::new(readback(
                        region,
                        interpretation,
                    )?)))
                }
                Request::PrepareSplTrial { .. } | Request::ExecuteSplTrial { .. } => {
                    Err(Error::State)
                }
            },
            cancel,
        )
    }

    /// Dispatches only authenticated closed requests to reviewed device policy.
    /// A verified response must follow device-local readback, never tool acceptance.
    /// # Errors
    /// Rejects client-side use, cancellation, framing and policy failures.
    pub fn serve_reviewed(
        &mut self,
        mut handler: impl FnMut(Request) -> Result<Response, Error>,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        if self.client {
            return Err(Error::State);
        }
        loop {
            let request = self.read::<Request>(cancel)?;
            let response = handler(request)?;
            self.write(&response, cancel)?;
            self.advance()?;
            if response == Response::RestartAccepted {
                return Ok(());
            }
        }
    }
}

/// Read-only diagnostic entry point. Inputs are explicit private regular files;
/// the expected implementation is hashed locally before authenticating a peer.
/// # Errors
/// Rejects unsafe paths, credentials, bounds and recovery authentication failures.
pub fn diagnostic_inventory(
    config: &std::path::Path,
    binary: &std::path::Path,
    cancel: &Cancellation,
) -> Result<Response, Error> {
    diagnostic_request(config, binary, &Request::Inventory, cancel)
}

/// Reads one reviewed boot block without accepting paths or addresses.
/// # Errors
/// Rejects unsafe credentials, authentication, bounds and readback failures.
pub fn diagnostic_boot_readback(
    config: &std::path::Path,
    binary: &std::path::Path,
    region: BootRegion,
    interpretation: ReadInterpretation,
    cancel: &Cancellation,
) -> Result<Response, Error> {
    interpretation.validate(region)?;
    diagnostic_request(
        config,
        binary,
        &Request::BootReadback {
            region,
            interpretation,
        },
        cancel,
    )
}

/// Authenticates the live endpoint without depending on inventory collection.
/// # Errors
/// Rejects unsafe files, cancellation and any authentication failure.
pub fn diagnostic_ping(
    config: &std::path::Path,
    binary: &std::path::Path,
    cancel: &Cancellation,
) -> Result<Response, Error> {
    diagnostic_request(config, binary, &Request::Ping, cancel)
}

/// Requests a controlled RAM-only reboot. Discover FEL again to prove it ran.
/// # Errors
/// Rejects invalid credentials, cancellation and authentication failures.
pub fn diagnostic_return_to_fel(
    config: &std::path::Path,
    binary: &std::path::Path,
    cancel: &Cancellation,
) -> Result<Response, Error> {
    diagnostic_request(config, binary, &Request::ReturnToFel, cancel)
}

fn diagnostic_request(
    config: &std::path::Path,
    binary: &std::path::Path,
    request: &Request,
    cancel: &Cancellation,
) -> Result<Response, Error> {
    let (credentials, implementation) = diagnostic_identity(config, binary)?;
    let address = SocketAddr::from(([192, 168, 81, 1], 3333));
    let mut channel = Channel::connect(address, &credentials, &implementation, cancel)?;
    channel.request(request, cancel)
}

fn diagnostic_identity(
    config: &std::path::Path,
    binary: &std::path::Path,
) -> Result<(Credentials, [u8; 32]), Error> {
    use sha2::{Digest, Sha256};
    for path in [config, binary] {
        if !path.is_absolute() {
            return Err(Error::UnsafePath);
        }
        crate::assets::regular_components(path)?;
        if !std::fs::metadata(path)?.is_file() {
            return Err(Error::UnsafePath);
        }
    }
    let file = std::fs::File::open(config)?;
    let metadata = file.metadata()?;
    if metadata.len() != 64 {
        return Err(Error::Length);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
            return Err(Error::UnsafePath);
        }
    }
    let mut bytes = Vec::new();
    file.take(65).read_to_end(&mut bytes)?;
    let credentials = Credentials::parse(&bytes)?;
    bytes.fill(0);
    let mut binary_bytes = Vec::new();
    std::fs::File::open(binary)?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut binary_bytes)?;
    if binary_bytes.is_empty() || binary_bytes.len() > 32 * 1024 * 1024 {
        return Err(Error::Length);
    }
    let implementation: [u8; 32] = Sha256::digest(binary_bytes).into();
    Ok((credentials, implementation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};
    fn credentials(marker: u8) -> Credentials {
        Credentials::parse(&[marker; 64]).unwrap()
    }
    fn pair() -> (Channel, Channel) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            Channel::accept(
                listener.accept().unwrap().0,
                &credentials(1),
                &[2; 32],
                &Cancellation::default(),
            )
            .unwrap()
        });
        let client =
            Channel::connect(address, &credentials(1), &[2; 32], &Cancellation::default()).unwrap();
        (client, server.join().unwrap())
    }
    #[test]
    fn fresh_nonces_and_authenticated_round_trip() {
        let (mut client, mut server) = pair();
        let (second, _) = pair();
        assert_ne!(client.transcript, second.transcript);
        let task = thread::spawn(move || {
            for _ in 0..2 {
                assert_eq!(
                    server.read::<Request>(&Cancellation::default()).unwrap(),
                    Request::Ping
                );
                server
                    .write(&Response::Pong, &Cancellation::default())
                    .unwrap();
                server.advance().unwrap();
            }
        });
        for _ in 0..2 {
            assert_eq!(
                client
                    .request(&Request::Ping, &Cancellation::default())
                    .unwrap(),
                Response::Pong
            );
        }
        task.join().unwrap();
        assert_eq!(client.sequence, 2);
    }

    #[test]
    fn controlled_restart_is_authenticated_and_returns_after_response() {
        let (mut client, mut server) = pair();
        let task = thread::spawn(move || {
            server.serve(
                || Err(Error::State),
                |_, _| Err(Error::State),
                &Cancellation::default(),
            )
        });
        assert_eq!(
            client
                .request(&Request::ReturnToFel, &Cancellation::default())
                .unwrap(),
            Response::RestartAccepted
        );
        assert!(task.join().unwrap().is_ok());
    }
    #[test]
    fn boot_readback_cannot_substitute_a_different_partition() {
        for (reported, interpretation, requested) in [
            (
                BootRegion::SplPrimary,
                ReadInterpretation::Raw,
                ReadInterpretation::Raw,
            ),
            (
                BootRegion::UBoot,
                ReadInterpretation::Raw,
                ReadInterpretation::Raw,
            ),
            (
                BootRegion::SplPrimary,
                ReadInterpretation::KernelCorrected,
                ReadInterpretation::Raw,
            ),
            (
                BootRegion::SplPrimary,
                ReadInterpretation::Boot0Corrected,
                ReadInterpretation::Boot0Corrected,
            ),
        ] {
            let (mut client, mut server) = pair();
            let task = thread::spawn(move || {
                assert_eq!(
                    server.read::<Request>(&Cancellation::default()).unwrap(),
                    Request::BootReadback {
                        region: BootRegion::SplPrimary,
                        interpretation: requested
                    }
                );
                let report = BootReadback {
                    region: reported,
                    interpretation,
                    data_bytes: 4_194_304,
                    oob_bytes: 425_984,
                    data_sha256: "a".repeat(64),
                    oob_sha256: "b".repeat(64),
                    interleaved_sha256: "c".repeat(64),
                    first_data_bytes: vec![0xff; 32],
                    erased_data_pages: Vec::new(),
                    erased_oob_pages: Vec::new(),
                    page_marker_bytes: vec![[0xff; 2]; 256],
                    corrected_bits_before: 0,
                    corrected_bits_after: 0,
                    ecc_failures_before: 0,
                    ecc_failures_after: 0,
                    tool_stderr: String::new(),
                    spl_copies: Vec::new(),
                };
                server
                    .write(
                        &Response::BootReadback(Box::new(report)),
                        &Cancellation::default(),
                    )
                    .unwrap();
            });
            let response = client.request(
                &Request::BootReadback {
                    region: BootRegion::SplPrimary,
                    interpretation: requested,
                },
                &Cancellation::default(),
            );
            assert_eq!(
                response.is_ok(),
                reported == BootRegion::SplPrimary
                    && interpretation == ReadInterpretation::Raw
                    && requested == ReadInterpretation::Raw
            );
            task.join().unwrap();
        }
    }
    #[test]
    fn wrong_key_session_sid_or_implementation_rejected() {
        for field in [0, 32, 48, 64] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                Channel::accept(
                    listener.accept().unwrap().0,
                    &credentials(1),
                    &[2; 32],
                    &Cancellation::default(),
                )
            });
            let mut bytes = [1; 64];
            let mut implementation = [2; 32];
            if field < 64 {
                bytes[field] = 3;
            } else {
                implementation[0] = 3;
            }
            assert!(
                Channel::connect(
                    address,
                    &Credentials::parse(&bytes).unwrap(),
                    &implementation,
                    &Cancellation::default()
                )
                .is_err()
            );
            assert!(server.join().unwrap().is_err());
        }
    }
    #[test]
    fn bounds_and_replay_sequence_checked_before_payload() {
        for (length, sequence) in [(0_u32, 0_u64), (65_537, 0), (4, 1)] {
            let (mut client, mut server) = pair();
            client.stream.write_all(&length.to_be_bytes()).unwrap();
            client.stream.write_all(&sequence.to_be_bytes()).unwrap();
            assert!(matches!(
                server.read::<Request>(&Cancellation::default()),
                Err(Error::Recovery)
            ));
        }
    }
    #[test]
    fn tampered_payload_and_cross_session_frame_rejected() {
        for cross_session in [false, true] {
            let (mut client, mut server) = pair();
            if cross_session {
                client.transcript[0] ^= 1;
            } else {
                client.key = hmac::Key::new(hmac::HMAC_SHA256, &[3; 32]);
            }
            client
                .write(&Request::Ping, &Cancellation::default())
                .unwrap();
            assert!(matches!(
                server.read::<Request>(&Cancellation::default()),
                Err(Error::Recovery)
            ));
        }
    }
    #[test]
    fn malformed_json_and_unreviewed_operations_rejected() {
        for payload in [
            b"\"Erase\"".as_slice(),
            b"{\"Inventory\":{\"command\":\"sh\"}}",
            b"\"Ping\" trailing",
        ] {
            assert!(serde_json::from_slice::<Request>(payload).is_err());
        }
    }
    #[test]
    fn truncated_frame_and_cancellation_fail_closed() {
        let (mut client, mut server) = pair();
        client.stream.write_all(&16_u32.to_be_bytes()).unwrap();
        drop(client);
        assert!(server.read::<Request>(&Cancellation::default()).is_err());
        let (mut client, _) = pair();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            client.request(&Request::Ping, &cancel),
            Err(Error::Cancelled)
        ));
        assert!(
            client
                .request(&Request::Ping, &Cancellation::default())
                .is_err()
        );
    }
    #[test]
    fn sid_and_private_configuration_validation() {
        let (credential, bytes) = Credentials::generate([7; 16]).unwrap();
        credential.check_sid(&[7; 16]).unwrap();
        assert!(credential.check_sid(&[8; 16]).is_err());
        assert!(Credentials::generate([0; 16]).is_err());
        assert!(Credentials::parse(&[0; 64]).is_err());
        assert!(Credentials::parse(&bytes[..63]).is_err());
        assert!(!format!("{credential:?}").contains("key"));
    }
}
