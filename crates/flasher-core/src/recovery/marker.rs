//! Fixed last-page observation through a read-only physical Hynix alias.
//! Marker bytes are observations, not proof of factory/runtime bad-block origin.
use super::MtdInfo;
use crate::Error;
use sha2::{Digest, Sha256};

const PAGE: usize = 16_384;
const OOB: usize = 1_664;

/// Authenticated raw last-page evidence. No NAND data or write capability is returned.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerReadback {
    pub info: MtdInfo,
    pub flags: u64,
    pub physical_page: u16,
    pub data_sha256: String,
    pub oob: Vec<u8>,
    pub interleaved_sha256: String,
}
impl MarkerReadback {
    /// Summarize exactly one fixed page, retaining OOB bytes but only data hashes.
    /// # Errors
    /// Rejects an unsupported alias, writable/SLC flags or a partial page.
    pub fn from_raw(info: MtdInfo, flags: u64, raw: &[u8]) -> Result<Self, Error> {
        Self::validate_alias(&info, flags)?;
        if raw.len() != PAGE + OOB {
            return Err(Error::Length);
        }
        Ok(Self {
            info,
            flags,
            physical_page: 255,
            data_sha256: format!("{:x}", Sha256::digest(&raw[..PAGE])),
            oob: raw[PAGE..].to_vec(),
            interleaved_sha256: format!("{:x}", Sha256::digest(raw)),
        })
    }

    /// Check the fixed physical alias before opening any NAND utility operation.
    /// # Errors
    /// Rejects any changed geometry, location, status or access flags.
    pub fn validate_alias(info: &MtdInfo, flags: u64) -> Result<(), Error> {
        // NAND capability flags are writable (0x400) and optionally SLC (0x4000).
        // The reviewed read-only non-SLC alias has neither; reject other flags.
        if flags != 0
            || info.index != 5
            || info.name != "BBM.probe"
            || info.offset != 947_912_704
            || info.size != 4_194_304
            || info.erase_size != 4_194_304
            || info.page_size != PAGE as u64
            || info.oob_size != OOB as u64
            || info.bad_blocks != 1
            || info.bbt_blocks != 0
            || info.ecc_strength != 56
            || info.ecc_step != 1024
            || info.ecc_failures != 0
        {
            return Err(Error::Device);
        }
        Ok(())
    }

    /// Recheck bounded evidence at the receiving boundary.
    /// # Errors
    /// Rejects a changed alias, page, OOB length or malformed digest.
    pub fn validate(&self) -> Result<(), Error> {
        Self::validate_alias(&self.info, self.flags)?;
        if self.physical_page != 255
            || self.oob.len() != OOB
            || [&self.data_sha256, &self.interleaved_sha256]
                .iter()
                .any(|digest| {
                    digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
        {
            return Err(Error::Recovery);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn scripted_fixture() -> MarkerReadback {
    let info = MtdInfo {
        index: 5,
        name: "BBM.probe".into(),
        size: 4_194_304,
        erase_size: 4_194_304,
        page_size: PAGE as u64,
        oob_size: OOB as u64,
        bad_blocks: 1,
        bbt_blocks: 0,
        ecc_strength: 56,
        ecc_step: 1024,
        corrected_bits: 0,
        ecc_failures: 0,
        offset: 947_912_704,
    };
    let mut raw = vec![0xff; PAGE + OOB];
    raw[0] = 0x42;
    raw[PAGE] = 0;
    MarkerReadback::from_raw(info, 0, &raw).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_marker_bytes_do_not_imply_origin_and_data_are_not_returned() {
        let report = scripted_fixture();
        report.validate().unwrap();
        assert_eq!(&report.oob[..2], [0, 0xff]);
        assert_eq!(report.physical_page, 255);
        assert!(!serde_json::to_string(&report).unwrap().contains("factory"));
        let mut raw = vec![0xff; PAGE + OOB];
        raw[0] = 0x42;
        raw[PAGE] = 0;
        assert_eq!(
            report.data_sha256,
            format!("{:x}", Sha256::digest(&raw[..PAGE]))
        );
        assert_eq!(
            report.interleaved_sha256,
            format!("{:x}", Sha256::digest(&raw))
        );
        let info = scripted_fixture().info;
        assert!(MarkerReadback::from_raw(info, 0, &raw[..raw.len() - 1]).is_err());
    }

    #[test]
    fn changed_alias_access_geometry_or_framed_evidence_is_rejected() {
        let changes: [fn(&mut MarkerReadback); 12] = [
            |r| r.flags = 0x400,
            |r| r.flags = 0x4000,
            |r| r.info.index = 4,
            |r| r.info.offset += 4_194_304,
            |r| r.info.erase_size /= 2,
            |r| r.info.name = "rootfs".into(),
            |r| r.info.bad_blocks = 0,
            |r| r.info.bbt_blocks = 1,
            |r| r.info.ecc_failures = 1,
            |r| r.physical_page = 253,
            |r| r.oob.push(0),
            |r| r.data_sha256 = "g".repeat(64),
        ];
        for change in changes {
            let mut report = scripted_fixture();
            change(&mut report);
            assert!(report.validate().is_err());
        }
    }
}
