//! FEL identity is only a candidate: A13/R8 alone does not prove `PocketCHIP`.
use crate::{Cancellation, Error, process::SunxiTool};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub bus: u16,
    pub address: u16,
    pub soc: String,
    pub sid: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nand {
    Hynix,
    Toshiba,
}
impl Nand {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hynix => "SK Hynix H27UCG8T2ETR",
            Self::Toshiba => "Toshiba TC58TEG5DCLTA00",
        }
    }
    #[must_use]
    pub const fn page_bytes(self) -> u32 {
        16384
    }
    #[must_use]
    pub const fn erase_block_bytes(self) -> u32 {
        4_194_304
    }
    #[must_use]
    pub const fn oob_bytes(self) -> u32 {
        match self {
            Self::Hynix => 1664,
            Self::Toshiba => 1280,
        }
    }
    /// Full approved part identifiers only; never default an unknown NAND to Hynix.
    /// # Errors
    /// Rejects unknown or partial identification.
    pub fn identify(part: &str) -> Result<Self, Error> {
        match part {
            "H27UCG8T2ETR" => Ok(Self::Hynix),
            "TC58TEG5DCLTA00" => Ok(Self::Toshiba),
            _ => Err(Error::Device),
        }
    }
}
/// Parses the pinned sunxi-fel `--list` format, preserving unsupported candidates.
/// # Errors
/// Malformed or excessive output is rejected in full, never filtered into a safe-looking subset.
pub fn parse_list(text: &str) -> Result<Vec<Device>, Error> {
    if text.len() > 64 * 1024 {
        return Err(Error::Output);
    }
    let mut devices = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 6
            || fields[0] != "USB"
            || fields[1] != "device"
            || fields[3] != "Allwinner"
        {
            return Err(Error::Device);
        }
        let (bus, address) = fields[2].split_once(':').ok_or(Error::Device)?;
        let bus = bus.parse::<u16>().map_err(|_| Error::Device)?;
        let address = address.parse::<u16>().map_err(|_| Error::Device)?;
        let sid = fields[5];
        if bus == 0
            || address == 0
            || sid.len() != 35
            || !sid.split(':').all(|p| crate::manifest::hex(p, 8))
            || sid == "00000000:00000000:00000000:00000000"
        {
            return Err(Error::Device);
        }
        if devices.len() == 16 {
            return Err(Error::MultipleDevices);
        }
        devices.push(Device {
            bus,
            address,
            soc: fields[4].to_owned(),
            sid: sid.to_owned(),
        });
    }
    Ok(devices)
}
/// Requires exactly one A13/R8 candidate with stable identity.
/// # Errors
/// Rejects zero, multiple and incompatible devices.
pub fn select(devices: &[Device]) -> Result<Device, Error> {
    match devices {
        [] => Err(Error::NoDevice),
        [d] if d.soc == "A13"
            && d.sid.len() == 35
            && d.sid != "00000000:00000000:00000000:00000000" =>
        {
            Ok(d.clone())
        }
        [_] => Err(Error::Device),
        _ => Err(Error::MultipleDevices),
    }
}
#[derive(Debug)]
pub struct RealFel {
    tool: SunxiTool,
}
impl RealFel {
    #[must_use]
    pub const fn new(tool: SunxiTool) -> Self {
        Self { tool }
    }
    /// Read-only FEL enumeration. Does not boot recovery or identify NAND by erasing.
    /// # Errors
    /// Returns external-tool and device-format errors.
    pub fn discover(&self, cancel: &Cancellation) -> Result<Vec<Device>, Error> {
        parse_list(&self.tool.list(cancel)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const LINE: &str =
        "USB device 001:002   Allwinner A13     01234567:89abcdef:01234567:89abcdef\n";
    #[test]
    fn parses_pinned_output() -> Result<(), Error> {
        assert_eq!(select(&parse_list(LINE)?)?.address, 2);
        Ok(())
    }
    #[test]
    fn rejects_wrong_multiple_and_absent_devices() -> Result<(), Error> {
        assert!(matches!(select(&[]), Err(Error::NoDevice)));
        assert!(matches!(
            select(&parse_list(&LINE.repeat(2))?),
            Err(Error::MultipleDevices)
        ));
        assert!(matches!(
            select(&parse_list(&LINE.replace("A13", "H3"))?),
            Err(Error::Device)
        ));
        Ok(())
    }
    #[test]
    fn malformed_entries_never_disappear() {
        for bad in [
            "garbage",
            "USB device 001:002 Allwinner A13",
            "USB device 001:002 Allwinner A13 00000000:00000000:00000000:00000000",
        ] {
            assert!(parse_list(&format!("{LINE}{bad}")).is_err());
        }
    }
    #[test]
    fn nand_never_defaults_unknown_parts() -> Result<(), Error> {
        assert_eq!(Nand::identify("H27UCG8T2ETR")?.oob_bytes(), 1664);
        assert_eq!(Nand::identify("TC58TEG5DCLTA00")?.oob_bytes(), 1280);
        assert!(Nand::identify("unknown").is_err());
        assert!(Nand::identify("Hynix").is_err());
        Ok(())
    }
}
