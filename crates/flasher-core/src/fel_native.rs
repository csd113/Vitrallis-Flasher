//! Native, bounded Allwinner FEL transport.
//!
//! Framing follows the pinned sunxi-tools protocol, not manifest commands.
//! Until recovery RAM policy is physically established, writes are restricted
//! to the documented A13 scratch area, and execution to a readback-verified
//! ARM `bx lr` diagnostic. FEL cannot establish board/NAND/RAM capacity, so
//! `identify` remains blocked and `device_info` reports unknown RAM as zero.
use crate::{
    Cancellation, Error,
    device::{Device, DeviceInfo, TargetInfo},
    fel::FelTransport,
};
use rusb::{Context, DeviceHandle, Direction, TransferType, UsbContext};
use std::time::{Duration, Instant};

const VENDOR: u16 = 0x1f3a;
const PRODUCT: u16 = 0xefe8;
const SID_ADDRESS: u32 = 0x01c2_3800;
const SCRATCH_ADDRESS: u32 = 0x1000;
const SCRATCH_LENGTH: u32 = 256;
const RETURN_INSTRUCTION: [u8; 4] = 0xe12f_ff1e_u32.to_le_bytes();
// A13 SID MMIO must be read with aligned 32-bit LDR instructions. BROM's
// byte-copy READ returns only each register's low byte. This fixed leaf uses
// caller-saved r0/r1, writes four results at scratch+44, then returns to BROM.
// No stack, branches, variable MMIO address or NAND operation is involved.
const SID_READER: [u32; 15] = [
    0xe59f_0020, // ldr r0, [pc, #32] -- SID address literal at offset 40
    0xe590_1000, // ldr r1, [r0]
    0xe58f_101c, // str r1, [pc, #28] -- result at offset 44
    0xe590_1004, // ldr r1, [r0, #4]
    0xe58f_1018, // str r1, [pc, #24] -- result at offset 48
    0xe590_1008, // ldr r1, [r0, #8]
    0xe58f_1014, // str r1, [pc, #20] -- result at offset 52
    0xe590_100c, // ldr r1, [r0, #12]
    0xe58f_1010, // str r1, [pc, #16] -- result at offset 56
    0xe12f_ff1e, // bx lr
    SID_ADDRESS,
    0,
    0,
    0,
    0,
];
const MAX_TRANSFER: usize = 4096;
const IO_TIMEOUT: Duration = Duration::from_millis(500);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(15);

/// BROM facts only; board identity and NAND geometry require recovery evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BromVersion {
    pub raw: [u8; 32],
    pub soc_id: u16,
    pub protocol: u16,
    pub scratchpad: u32,
}
impl BromVersion {
    fn parse(raw: &[u8]) -> Result<Self, Error> {
        let raw: [u8; 32] = raw
            .try_into()
            .map_err(|_| Error::FelProtocol("version length"))?;
        if &raw[..8] != b"AWUSBFEX" {
            return Err(Error::FelProtocol("version signature"));
        }
        let soc_id = u16::from_le_bytes([raw[9], raw[10]]);
        let protocol = u16::from_le_bytes([raw[16], raw[17]]);
        if soc_id != 0x1625 || protocol != 1 {
            return Err(Error::Device);
        }
        Ok(Self {
            soc_id,
            protocol,
            scratchpad: u32::from_le_bytes([raw[20], raw[21], raw[22], raw[23]]),
            raw,
        })
    }
}

trait BulkIo {
    fn send(&mut self, data: &[u8], cancel: &Cancellation) -> Result<usize, Error>;
    fn receive(&mut self, data: &mut [u8], cancel: &Cancellation) -> Result<usize, Error>;
}

struct UsbIo {
    handle: DeviceHandle<Context>,
    input: u8,
    output: u8,
    started: Instant,
}
impl UsbIo {
    fn timeout(&self, cancel: &Cancellation) -> Result<Duration, Error> {
        cancel.check()?;
        let remaining = OPERATION_TIMEOUT
            .checked_sub(self.started.elapsed())
            .filter(|left| *left >= Duration::from_millis(1))
            .ok_or(Error::Timeout)?;
        Ok(remaining.min(IO_TIMEOUT))
    }
    fn result(result: Result<usize, rusb::Error>, cancel: &Cancellation) -> Result<usize, Error> {
        cancel.check()?;
        result.map_err(|error| match error {
            rusb::Error::Timeout => Error::Timeout,
            rusb::Error::NoDevice => Error::NoDevice,
            _ => Error::Usb(error),
        })
    }
}
impl BulkIo for UsbIo {
    fn send(&mut self, data: &[u8], cancel: &Cancellation) -> Result<usize, Error> {
        let timeout = self.timeout(cancel)?;
        Self::result(self.handle.write_bulk(self.output, data, timeout), cancel)
    }
    fn receive(&mut self, data: &mut [u8], cancel: &Cancellation) -> Result<usize, Error> {
        let timeout = self.timeout(cancel)?;
        Self::result(self.handle.read_bulk(self.input, data, timeout), cancel)
    }
}

struct Protocol<T>(T);
impl<T: BulkIo> Protocol<T> {
    fn send(&mut self, data: &[u8], cancel: &Cancellation) -> Result<(), Error> {
        for chunk in data.chunks(MAX_TRANSFER) {
            cancel.check()?;
            if self.0.send(chunk, cancel)? != chunk.len() {
                return Err(Error::FelProtocol("short outbound transfer"));
            }
        }
        cancel.check()
    }
    fn receive(&mut self, data: &mut [u8], cancel: &Cancellation) -> Result<(), Error> {
        for chunk in data.chunks_mut(MAX_TRANSFER) {
            cancel.check()?;
            let mut received = 0;
            while received < chunk.len() {
                let count = self.0.receive(&mut chunk[received..], cancel)?;
                if count == 0 || count > chunk.len() - received {
                    return Err(Error::FelProtocol("invalid inbound transfer length"));
                }
                received += count;
                cancel.check()?;
            }
        }
        Ok(())
    }
    fn header(
        &mut self,
        operation: u16,
        length: usize,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        let length = u32::try_from(length).map_err(|_| Error::Length)?;
        let mut header = [0; 32];
        header[..4].copy_from_slice(b"AWUC");
        header[8..12].copy_from_slice(&length.to_le_bytes());
        header[12..16].copy_from_slice(&0x0c00_0000_u32.to_le_bytes());
        header[16..18].copy_from_slice(&operation.to_le_bytes());
        header[18..22].copy_from_slice(&length.to_le_bytes());
        self.send(&header, cancel)
    }
    fn acknowledgement(&mut self, cancel: &Cancellation) -> Result<(), Error> {
        let mut response = [0; 13];
        self.receive(&mut response, cancel)?;
        if &response[..5] != b"AWUS\0" {
            return Err(Error::FelProtocol("USB acknowledgement signature"));
        }
        Ok(())
    }
    fn write(&mut self, data: &[u8], cancel: &Cancellation) -> Result<(), Error> {
        self.header(0x12, data.len(), cancel)?;
        self.send(data, cancel)?;
        self.acknowledgement(cancel)
    }
    fn read(&mut self, data: &mut [u8], cancel: &Cancellation) -> Result<(), Error> {
        self.header(0x11, data.len(), cancel)?;
        self.receive(data, cancel)?;
        self.acknowledgement(cancel)
    }
    fn request(
        &mut self,
        operation: u32,
        address: u32,
        length: u32,
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        let mut request = [0; 16];
        request[..4].copy_from_slice(&operation.to_le_bytes());
        request[4..8].copy_from_slice(&address.to_le_bytes());
        request[8..12].copy_from_slice(&length.to_le_bytes());
        self.write(&request, cancel)
    }
    fn status(&mut self, cancel: &Cancellation) -> Result<u32, Error> {
        let mut status = [0; 8];
        self.read(&mut status, cancel)?;
        // Captured from this R8 BROM: ff ff 00 00 00 00 00 00. The
        // first two bytes are a marker, not the result code. Require the
        // measured marker and zero trailing word; never accept an unknown
        // status layout. See docs/evidence/batch3/fel-status-measurement.json.
        if status[..2] != [0xff; 2] || status[4..] != [0; 4] {
            return Err(Error::FelProtocol("status marker or trailing bytes"));
        }
        Ok(u32::from(u16::from_le_bytes([status[2], status[3]])))
    }
    fn successful_status(&mut self, cancel: &Cancellation) -> Result<(), Error> {
        if self.status(cancel)? != 0 {
            return Err(Error::FelProtocol("BROM rejected the request"));
        }
        Ok(())
    }
    fn version(&mut self, cancel: &Cancellation) -> Result<(BromVersion, u32), Error> {
        self.request(1, 0, 0, cancel)?;
        let mut version = [0; 32];
        self.read(&mut version, cancel)?;
        let status = self.status(cancel)?;
        Ok((BromVersion::parse(&version)?, status))
    }
    fn memory(
        &mut self,
        address: u32,
        length: u32,
        cancel: &Cancellation,
    ) -> Result<Vec<u8>, Error> {
        if length == 0 || length > SCRATCH_LENGTH || address.checked_add(length).is_none() {
            return Err(Error::Length);
        }
        self.request(0x103, address, length, cancel)?;
        let mut data = vec![0; usize::try_from(length).map_err(|_| Error::Length)?];
        self.read(&mut data, cancel)?;
        self.successful_status(cancel)?;
        Ok(data)
    }
    fn upload(&mut self, address: u32, data: &[u8], cancel: &Cancellation) -> Result<(), Error> {
        let length = u32::try_from(data.len()).map_err(|_| Error::Length)?;
        scratch_range(address, length)?;
        self.request(0x101, address, length, cancel)?;
        self.write(data, cancel)?;
        self.successful_status(cancel)
    }
    fn sid(&mut self, cancel: &Cancellation) -> Result<Vec<u8>, Error> {
        let reader: Vec<_> = SID_READER
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        let original = self.memory(SCRATCH_ADDRESS, 60, cancel)?;
        self.upload(SCRATCH_ADDRESS, &reader, cancel)?;
        if self.memory(SCRATCH_ADDRESS, 60, cancel)? != reader {
            return Err(Error::Hash);
        }
        self.request(0x102, SCRATCH_ADDRESS, 0, cancel)?;
        self.successful_status(cancel)?;
        let bytes = self.memory(SCRATCH_ADDRESS + 44, 16, cancel)?;
        self.upload(SCRATCH_ADDRESS, &original, cancel)?;
        if self.memory(SCRATCH_ADDRESS, 60, cancel)? != original {
            return Err(Error::Hash);
        }
        Ok(bytes)
    }
}

fn scratch_range(address: u32, length: u32) -> Result<(), Error> {
    if length == 0
        || address < SCRATCH_ADDRESS
        || address
            .checked_add(length)
            .is_none_or(|end| end > SCRATCH_ADDRESS + SCRATCH_LENGTH)
    {
        return Err(Error::Length);
    }
    Ok(())
}

/// Real USB transport with no manifest-controlled operations or memory policy.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFel;
impl NativeFel {
    fn candidates(cancel: &Cancellation) -> Result<Vec<rusb::Device<Context>>, Error> {
        cancel.check()?;
        let context = Context::new()?;
        let mut candidates = Vec::new();
        for device in context.devices()?.iter() {
            cancel.check()?;
            let descriptor = device.device_descriptor()?;
            if descriptor.vendor_id() == VENDOR && descriptor.product_id() == PRODUCT {
                if candidates.len() == 16 {
                    return Err(Error::MultipleDevices);
                }
                candidates.push(device);
            }
        }
        Ok(candidates)
    }
    fn open(
        device: &rusb::Device<Context>,
        cancel: &Cancellation,
    ) -> Result<Protocol<UsbIo>, Error> {
        cancel.check()?;
        let config = device.active_config_descriptor()?;
        let mut endpoints = None;
        for interface in config.interfaces() {
            for descriptor in interface.descriptors() {
                if descriptor.setting_number() != 0 {
                    continue;
                }
                let mut input = None;
                let mut output = None;
                for endpoint in descriptor.endpoint_descriptors() {
                    if endpoint.transfer_type() != TransferType::Bulk {
                        return Err(Error::Device);
                    }
                    let slot = if endpoint.direction() == Direction::In {
                        &mut input
                    } else {
                        &mut output
                    };
                    if slot.replace(endpoint.address()).is_some() {
                        return Err(Error::Device);
                    }
                }
                if let (Some(input), Some(output)) = (input, output)
                    && endpoints
                        .replace((descriptor.interface_number(), input, output))
                        .is_some()
                {
                    return Err(Error::Device);
                }
            }
        }
        let (interface, input, output) = endpoints.ok_or(Error::Device)?;
        let handle = device.open()?;
        handle.claim_interface(interface)?;
        cancel.check()?;
        Ok(Protocol(UsbIo {
            handle,
            input,
            output,
            started: Instant::now(),
        }))
    }
    fn identity(protocol: &mut Protocol<UsbIo>, cancel: &Cancellation) -> Result<String, Error> {
        let (_, status) = protocol.version(cancel)?;
        if status != 0 {
            return Err(Error::FelProtocol("BROM rejected version query"));
        }
        let bytes = protocol.sid(cancel)?;
        let sid = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| {
                format!(
                    "{:08x}",
                    u32::from_le_bytes([word[0], word[1], word[2], word[3]])
                )
            })
            .collect::<Vec<_>>()
            .join(":");
        if sid == "00000000:00000000:00000000:00000000" {
            return Err(Error::Device);
        }
        Ok(sid)
    }
    fn bound(device: &Device, cancel: &Cancellation) -> Result<Protocol<UsbIo>, Error> {
        let candidates = Self::candidates(cancel)?;
        let candidate = match candidates.as_slice() {
            [] => return Err(Error::NoDevice),
            [candidate] => candidate,
            _ => return Err(Error::MultipleDevices),
        };
        let mut protocol = Self::open(candidate, cancel)?;
        if device.soc != "A13" || Self::identity(&mut protocol, cancel)? != device.sid {
            return Err(Error::Device);
        }
        // Bus/address can change after reconnection. SID is reread from this
        // newly opened handle, which is also used for the requested operation.
        Ok(protocol)
    }
    /// Reads the full version packet after freshly verifying the selected SID.
    /// # Errors
    /// Rejects missing/ambiguous devices, changed SID and malformed replies.
    pub fn brom_version(
        &self,
        device: &Device,
        cancel: &Cancellation,
    ) -> Result<BromVersion, Error> {
        let (version, status) = Self::bound(device, cancel)?.version(cancel)?;
        if status != 0 {
            return Err(Error::FelProtocol("BROM rejected version query"));
        }
        Ok(version)
    }

    /// Uploads a 256-byte pattern and executes only an ARM `bx lr` at its start.
    /// Restores and verifies the original 256 bytes on successful completion.
    /// # Errors
    /// Returns transport, cancellation or readback errors. A failed/cancelled
    /// probe may leave the harmless return instruction in scratch SRAM.
    pub fn diagnostic_probe(&self, device: &Device, cancel: &Cancellation) -> Result<(), Error> {
        let original = self.read_memory(device, SCRATCH_ADDRESS, SCRATCH_LENGTH, cancel)?;
        let mut pattern: Vec<_> = (0_u8..=u8::MAX).collect();
        pattern[..4].copy_from_slice(&RETURN_INSTRUCTION);
        self.upload_to_ram(device, SCRATCH_ADDRESS, &pattern, cancel)?;
        self.execute(device, SCRATCH_ADDRESS, cancel)?;
        self.brom_version(device, cancel)?;
        self.upload_to_ram(device, SCRATCH_ADDRESS, &original, cancel)
    }
}
impl FelTransport for NativeFel {
    fn discover(&self, cancel: &Cancellation) -> Result<Vec<Device>, Error> {
        let candidates = Self::candidates(cancel)?;
        if candidates.len() > 1 {
            return Err(Error::MultipleDevices);
        }
        let mut devices = Vec::new();
        for candidate in candidates {
            let mut protocol = Self::open(&candidate, cancel)?;
            devices.push(Device {
                bus: u16::from(candidate.bus_number()),
                address: u16::from(candidate.address()),
                soc: "A13".into(),
                sid: Self::identity(&mut protocol, cancel)?,
            });
        }
        Ok(devices)
    }
    fn identify(&self, device: &Device, cancel: &Cancellation) -> Result<TargetInfo, Error> {
        Self::bound(device, cancel)?;
        // BROM provides no NAND part or board identity. Never invent them.
        Err(Error::PhysicalBlocked)
    }
    fn device_info(&self, device: &Device, cancel: &Cancellation) -> Result<DeviceInfo, Error> {
        Self::bound(device, cancel)?;
        Ok(DeviceInfo {
            soc: device.soc.clone(),
            sid: device.sid.clone(),
            ram_bytes: 0,
        })
    }
    fn upload_to_ram(
        &self,
        device: &Device,
        address: u32,
        data: &[u8],
        cancel: &Cancellation,
    ) -> Result<(), Error> {
        cancel.check()?;
        scratch_range(
            address,
            u32::try_from(data.len()).map_err(|_| Error::Length)?,
        )?;
        let mut protocol = Self::bound(device, cancel)?;
        protocol.upload(address, data, cancel)?;
        if protocol.memory(
            address,
            u32::try_from(data.len()).map_err(|_| Error::Length)?,
            cancel,
        )? != data
        {
            return Err(Error::Hash);
        }
        Ok(())
    }
    fn execute(&self, device: &Device, address: u32, cancel: &Cancellation) -> Result<(), Error> {
        cancel.check()?;
        if address != SCRATCH_ADDRESS {
            return Err(Error::PhysicalBlocked);
        }
        let mut protocol = Self::bound(device, cancel)?;
        if protocol.memory(address, 4, cancel)? != RETURN_INSTRUCTION {
            return Err(Error::PhysicalBlocked);
        }
        protocol.request(0x102, address, 0, cancel)?;
        protocol.successful_status(cancel)
    }
    fn read_memory(
        &self,
        device: &Device,
        address: u32,
        length: u32,
        cancel: &Cancellation,
    ) -> Result<Vec<u8>, Error> {
        cancel.check()?;
        scratch_range(address, length)?;
        Self::bound(device, cancel)?.memory(address, length, cancel)
    }
    fn read_status(&self, device: &Device, cancel: &Cancellation) -> Result<u32, Error> {
        // Status is a response to a command, not an MMIO address. Query version
        // to obtain fresh status rather than consuming an unrelated response.
        Ok(Self::bound(device, cancel)?.version(cancel)?.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(serde::Deserialize)]
    struct HardwareFixture {
        version: Vec<u8>,
        status: Vec<u8>,
        sid_bytes: Vec<u8>,
        incorrect_byte_mmio_sid: Vec<u8>,
    }
    fn hardware() -> HardwareFixture {
        serde_json::from_str(include_str!("../../../fixtures/fel/r8-batch3.json")).unwrap()
    }

    enum Step {
        Send(Vec<u8>, usize),
        Receive(Vec<u8>, usize),
        Fail,
    }
    struct Script {
        steps: VecDeque<Step>,
    }
    impl BulkIo for Script {
        fn send(&mut self, data: &[u8], _cancel: &Cancellation) -> Result<usize, Error> {
            match self.steps.pop_front() {
                Some(Step::Send(expected, count)) => {
                    assert_eq!(data, expected);
                    Ok(count)
                }
                Some(Step::Fail) => Err(Error::Timeout),
                _ => panic!("unexpected outbound transfer"),
            }
        }
        fn receive(&mut self, data: &mut [u8], _cancel: &Cancellation) -> Result<usize, Error> {
            match self.steps.pop_front() {
                Some(Step::Receive(bytes, count)) => {
                    let copied = bytes.len().min(data.len());
                    data[..copied].copy_from_slice(&bytes[..copied]);
                    Ok(count)
                }
                Some(Step::Fail) => Err(Error::Timeout),
                _ => panic!("unexpected inbound transfer"),
            }
        }
    }
    fn scripted(steps: impl IntoIterator<Item = Step>) -> Protocol<Script> {
        Protocol(Script {
            steps: steps.into_iter().collect(),
        })
    }
    // Exact version packet measured through native FEL on the development R8.
    fn version() -> [u8; 32] {
        hardware().version.try_into().unwrap()
    }
    fn ack() -> Step {
        Step::Receive(b"AWUS\0\0\0\0\0\0\0\0\0".to_vec(), 13)
    }
    fn version_steps() -> Vec<Step> {
        vec![
            Step::Send(
                vec![
                    65, 87, 85, 67, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 12, 18, 0, 16, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0,
                ],
                32,
            ),
            Step::Send(vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 16),
            ack(),
            Step::Send(
                vec![
                    65, 87, 85, 67, 0, 0, 0, 0, 32, 0, 0, 0, 0, 0, 0, 12, 17, 0, 32, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0,
                ],
                32,
            ),
            Step::Receive(version().to_vec(), 32),
            ack(),
            Step::Send(
                vec![
                    65, 87, 85, 67, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 12, 17, 0, 8, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0,
                ],
                32,
            ),
            Step::Receive(hardware().status, 8),
            ack(),
        ]
    }
    #[test]
    fn physical_version_framing_is_exact() -> Result<(), Error> {
        let mut protocol = scripted(version_steps());
        let (reply, status) = protocol.version(&Cancellation::default())?;
        assert_eq!(reply.soc_id, 0x1625);
        assert_eq!(reply.protocol, 1);
        assert_eq!(reply.scratchpad, 0x7e00);
        assert_eq!(status, 0);
        assert!(protocol.0.steps.is_empty());
        Ok(())
    }
    fn wire_write(steps: &mut Vec<Step>, bytes: &[u8]) {
        wire_header(steps, 0x12, bytes.len());
        steps.push(Step::Send(bytes.to_vec(), bytes.len()));
        steps.push(ack());
    }
    fn wire_read(steps: &mut Vec<Step>, bytes: &[u8]) {
        wire_header(steps, 0x11, bytes.len());
        steps.push(Step::Receive(bytes.to_vec(), bytes.len()));
        steps.push(ack());
    }
    fn wire_header(steps: &mut Vec<Step>, operation: u16, length: usize) {
        let length = u32::try_from(length).unwrap().to_le_bytes();
        let mut header = b"AWUC\0\0\0\0".to_vec();
        header.extend(length);
        header.extend([0, 0, 0, 12]);
        header.extend(operation.to_le_bytes());
        header.extend(length);
        header.extend([0; 10]);
        steps.push(Step::Send(header, 32));
    }
    fn wire_request(steps: &mut Vec<Step>, operation: u32, address: u32, length: usize) {
        let mut request = operation.to_le_bytes().to_vec();
        request.extend(address.to_le_bytes());
        request.extend(u32::try_from(length).unwrap().to_le_bytes());
        request.extend([0; 4]);
        wire_write(steps, &request);
    }
    fn wire_memory(steps: &mut Vec<Step>, address: u32, bytes: &[u8]) {
        wire_request(steps, 0x103, address, bytes.len());
        wire_read(steps, bytes);
        wire_read(steps, &hardware().status);
    }
    fn wire_upload(steps: &mut Vec<Step>, address: u32, bytes: &[u8]) {
        wire_request(steps, 0x101, address, bytes.len());
        wire_write(steps, bytes);
        wire_read(steps, &hardware().status);
    }
    #[test]
    fn physical_sid_requires_word_reader_and_restores_scratch() -> Result<(), Error> {
        let fixture = hardware();
        assert_ne!(fixture.sid_bytes, fixture.incorrect_byte_mmio_sid);
        let original = vec![0x55; 60];
        let reader: Vec<_> = SID_READER
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        let mut steps = Vec::new();
        wire_memory(&mut steps, 0x1000, &original);
        wire_upload(&mut steps, 0x1000, &reader);
        wire_memory(&mut steps, 0x1000, &reader);
        wire_request(&mut steps, 0x102, 0x1000, 0);
        wire_read(&mut steps, &fixture.status);
        // The SID result must be read from SRAM, never byte-read from MMIO.
        wire_memory(&mut steps, 0x102c, &fixture.sid_bytes);
        wire_upload(&mut steps, 0x1000, &original);
        wire_memory(&mut steps, 0x1000, &original);
        let mut protocol = scripted(steps);
        assert_eq!(protocol.sid(&Cancellation::default())?, fixture.sid_bytes);
        assert!(protocol.0.steps.is_empty());
        Ok(())
    }
    #[test]
    fn malformed_truncated_and_wrong_soc_versions_fail_closed() {
        assert!(BromVersion::parse(&version()[..31]).is_err());
        let mut packet = version();
        packet[0] = 0;
        assert!(BromVersion::parse(&packet).is_err());
        packet = version();
        packet[9] = 0x51;
        assert!(matches!(BromVersion::parse(&packet), Err(Error::Device)));
        packet = version();
        packet[16] = 2;
        assert!(matches!(BromVersion::parse(&packet), Err(Error::Device)));
    }
    #[test]
    fn short_inbound_transfers_are_accumulated() -> Result<(), Error> {
        let mut protocol = scripted([Step::Receive(vec![1, 2], 2), Step::Receive(vec![3, 4], 2)]);
        let mut bytes = [0; 4];
        protocol.receive(&mut bytes, &Cancellation::default())?;
        assert_eq!(bytes, [1, 2, 3, 4]);
        Ok(())
    }
    #[test]
    fn zero_length_oversized_and_short_outbound_transfers_fail() {
        for count in [0, 5] {
            let mut protocol = scripted([Step::Receive(vec![], count)]);
            assert!(
                protocol
                    .receive(&mut [0; 4], &Cancellation::default())
                    .is_err()
            );
        }
        let mut protocol = scripted([Step::Send(vec![1, 2], 1)]);
        assert!(protocol.send(&[1, 2], &Cancellation::default()).is_err());
    }
    #[test]
    fn invalid_acknowledgement_does_not_issue_another_request() {
        let mut protocol = scripted([Step::Receive(vec![0; 13], 13)]);
        assert!(protocol.acknowledgement(&Cancellation::default()).is_err());
        assert!(protocol.0.steps.is_empty());
    }
    #[test]
    fn nonzero_and_malformed_status_reject_success() {
        for offset in [0, 2, 4] {
            let mut bytes = vec![0xff, 0xff, 0, 0, 0, 0, 0, 0];
            bytes[offset] = 1;
            let mut protocol = scripted([
                Step::Send(
                    vec![
                        65, 87, 85, 67, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 12, 17, 0, 8, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0,
                    ],
                    32,
                ),
                Step::Receive(bytes, 8),
                ack(),
            ]);
            assert!(
                protocol
                    .successful_status(&Cancellation::default())
                    .is_err()
            );
            assert!(protocol.0.steps.is_empty());
        }
    }
    #[test]
    fn cancellation_and_timeout_stop_before_later_transfers() {
        let cancel = Cancellation::default();
        cancel.cancel();
        let mut protocol = scripted([]);
        assert!(matches!(protocol.version(&cancel), Err(Error::Cancelled)));
        let mut protocol = scripted([Step::Fail]);
        assert!(matches!(
            protocol.version(&Cancellation::default()),
            Err(Error::Timeout)
        ));
        assert!(protocol.0.steps.is_empty());
    }
    #[test]
    fn unreviewed_ranges_are_rejected_before_usb_access() {
        let device = Device {
            bus: 1,
            address: 1,
            soc: "A13".into(),
            sid: "01234567:89abcdef:01234567:89abcdef".into(),
        };
        for (address, length) in [
            (0, 4),
            (0x1000, 0),
            (0x1000, 257),
            (0x10ff, 2),
            (u32::MAX, 2),
            (0x4000_0000, 4),
        ] {
            assert!(matches!(
                NativeFel.read_memory(&device, address, length, &Cancellation::default()),
                Err(Error::Length)
            ));
        }
        assert!(matches!(
            NativeFel.execute(&device, 0x4000_0000, &Cancellation::default()),
            Err(Error::PhysicalBlocked)
        ));
        let mut protocol = scripted([]);
        assert!(matches!(
            protocol.upload(0x4000_0000, b"x", &Cancellation::default()),
            Err(Error::Length)
        ));
    }
}
