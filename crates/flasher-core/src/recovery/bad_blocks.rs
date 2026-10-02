//! Read-only rootfs eraseblock enumeration through the existing MTD utility.
//!
//! Entries are partition-relative logical eraseblocks, including unavailable BBT
//! blocks. They are not physical byte offsets or permission to write NAND.
use super::MtdInfo;
use crate::{
    Cancellation, Error,
    tool::{ToolRequest, ToolRunner},
};

const BLOCKS: u64 = 2044;
const ERASE: u64 = 2_097_152;
const LIMIT: usize = 64 * 1024;

/// Bounded utility output and the device-local geometry used to validate it.
/// Deserialized reports must pass [`Self::validate`] before they are accepted.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootfsMapReadback {
    pub info: MtdInfo,
    pub tool_stdout: String,
}
impl RootfsMapReadback {
    /// Recheck the complete map at the receiving boundary.
    /// # Errors
    /// Rejects unsupported geometry, malformed maps and incomplete enumeration.
    pub fn validate(&self) -> Result<RootfsBadBlockMap, Error> {
        RootfsBadBlockMap::parse(&self.tool_stdout, "", &self.info)
    }
}

/// A complete map checked against the freshly observed Hynix SLC partition.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct RootfsBadBlockMap {
    logical_eraseblock_bytes: u64,
    logical_eraseblocks: u64,
    unavailable_blocks: Vec<u16>,
}
impl RootfsBadBlockMap {
    /// Logical partition eraseblock indices; BAD also includes BBT reservations.
    #[must_use]
    pub fn unavailable_blocks(&self) -> &[u16] {
        &self.unavailable_blocks
    }

    /// Parse the reviewed utility format and require every expected entry.
    /// # Errors
    /// Rejects unsupported geometry, diagnostics, incomplete/reordered maps,
    /// unexpected flags, offsets or unavailable counts. This format does not
    /// establish page pairing or distinguish factory bad blocks from BBT blocks.
    pub fn parse(stdout: &str, stderr: &str, info: &MtdInfo) -> Result<Self, Error> {
        let expected_bad = geometry(info)?;
        if stdout.len() > LIMIT || !stderr.is_empty() || !stdout.starts_with("mtd4\n") {
            return Err(Error::Output);
        }
        let (header, map) = stdout
            .split_once("Eraseblock map:\n")
            .ok_or(Error::Output)?;
        for (label, expected) in [
            ("Name:", "rootfs"),
            ("Type:", "mlc-nand"),
            ("Bad blocks are allowed:", "true"),
            ("Device is writable:", "true"),
        ] {
            if field(header, label)? != expected {
                return Err(Error::Device);
            }
        }
        for (label, expected) in [
            ("Eraseblock size:", ERASE),
            ("Amount of eraseblocks:", BLOCKS),
            ("Minimum input/output unit size:", 16_384),
            ("OOB size:", 1664),
        ] {
            if field(header, label)?
                .split_whitespace()
                .next()
                .ok_or(Error::Output)?
                .parse::<u64>()
                .map_err(|_| Error::Output)?
                != expected
            {
                return Err(Error::Device);
            }
        }
        // Multi-region output has a different address contract and is unsupported.
        if header.contains("Eraseblock region") {
            return Err(Error::Device);
        }
        let mut tokens = map.split_whitespace().peekable();
        let mut unavailable_blocks = Vec::new();
        for index in 0..BLOCKS {
            let label = tokens.next().ok_or(Error::Output)?;
            let actual = label
                .strip_suffix(':')
                .ok_or(Error::Output)?
                .parse::<u64>()
                .map_err(|_| Error::Output)?;
            let offset = u64::from_str_radix(tokens.next().ok_or(Error::Output)?, 16)
                .map_err(|_| Error::Output)?;
            if actual != index || offset != index * ERASE {
                return Err(Error::Device);
            }
            if tokens.peek() == Some(&"BAD") {
                tokens.next();
                unavailable_blocks.push(u16::try_from(index).map_err(|_| Error::Length)?);
            }
        }
        // mtdinfo stops bad-block queries after the first ioctl failure and may
        // suppress EOPNOTSUPP diagnostics. The measured last block is BBT-reserved:
        // its BAD result proves queries did not stop before the end of this map.
        if tokens.next().is_some()
            || unavailable_blocks.len() as u64 != expected_bad
            || unavailable_blocks.last() != Some(&2043)
        {
            return Err(Error::Device);
        }
        Ok(Self {
            logical_eraseblock_bytes: ERASE,
            logical_eraseblocks: BLOCKS,
            unavailable_blocks,
        })
    }
}

fn geometry(info: &MtdInfo) -> Result<u64, Error> {
    let count = info
        .bad_blocks
        .checked_add(info.bbt_blocks)
        .ok_or(Error::Device)?;
    if info.index != 4
        || info.name != "rootfs"
        || info.offset != 16_777_216
        || info.size != BLOCKS * ERASE
        || info.erase_size != ERASE
        || info.page_size != 16_384
        || info.oob_size != 1664
        || info.ecc_strength != 56
        || info.ecc_step != 1024
        || info.ecc_failures != 0
        || info.bbt_blocks != 4
        || count >= BLOCKS
    {
        return Err(Error::Device);
    }
    Ok(count)
}

fn field<'a>(header: &'a str, label: &str) -> Result<&'a str, Error> {
    let mut values = header.lines().filter_map(|line| line.strip_prefix(label));
    let value = values.next().ok_or(Error::Output)?.trim();
    if values.next().is_some() {
        return Err(Error::Output);
    }
    Ok(value)
}

/// Obtain a new map using a fixed path and read-only utility operation.
/// # Errors
/// Propagates cancellation, tool failures/timeouts and map validation failures.
/// No map is returned from partial utility output or unsupported ioctl results.
pub fn capture(
    runner: &dyn ToolRunner,
    info: MtdInfo,
    cancel: &Cancellation,
) -> Result<RootfsMapReadback, Error> {
    cancel.check()?;
    geometry(&info)?;
    let output = runner.run(
        &ToolRequest::new("/usr/sbin/mtdinfo", ["--map", "/dev/mtd4"]),
        cancel,
    )?;
    if output.exit != Some(0) {
        return Err(Error::Output);
    }
    let _map = RootfsBadBlockMap::parse(&output.stdout, &output.stderr, &info)?;
    cancel.check()?;
    Ok(RootfsMapReadback {
        info,
        tool_stdout: output.stdout,
    })
}

#[cfg(test)]
pub(super) fn scripted_fixture() -> RootfsMapReadback {
    let (info, blocks) = tests::measured();
    RootfsMapReadback {
        info,
        tool_stdout: tests::render(&blocks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ScriptedToolRunner, ToolOutput};
    use std::fmt::Write;

    pub(super) fn measured() -> (MtdInfo, Vec<u16>) {
        let recovery: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-protected-chain-after-backup-erase-10.json"
        ))
        .unwrap();
        let info = serde_json::from_value(
            recovery["records"][2]["response"]["Inventory"]["mtd"][4].clone(),
        )
        .unwrap();
        let ioctl: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/mtd-ioctl-inventory.json"
        ))
        .unwrap();
        let blocks = ioctl["records"][4]["bad_blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                u16::try_from(entry["partition_offset"].as_u64().unwrap() / ERASE).unwrap()
            })
            .collect();
        (info, blocks)
    }

    // Scripted utility formatting with measured indices; not a live --map capture.
    pub(super) fn render(blocks: &[u16]) -> String {
        let mut output = "mtd4\nName: rootfs\nType: mlc-nand\nEraseblock size: 2097152 bytes, 2.0 MiB\nAmount of eraseblocks: 2044 (4286578688 bytes, 3.9 GiB)\nMinimum input/output unit size: 16384 bytes\nOOB size: 1664 bytes\nBad blocks are allowed: true\nDevice is writable: true\nEraseblock map:\n".to_owned();
        for index in 0..BLOCKS {
            let status = if blocks.contains(&u16::try_from(index).unwrap()) {
                "BAD"
            } else {
                ""
            };
            writeln!(output, " {index}: {:08x} {status}", index * ERASE).unwrap();
        }
        output
    }

    #[test]
    fn actual_authenticated_v6_map_matches_original_ioctl_enumeration() {
        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/evidence/batch3/recovery-rootfs-map-11.json"
        ))
        .unwrap();
        let report: RootfsMapReadback =
            serde_json::from_value(evidence["response"]["RootfsMap"].clone()).unwrap();
        let (_, original_blocks) = measured();
        assert_eq!(
            report.validate().unwrap().unavailable_blocks(),
            original_blocks
        );
        assert_eq!(report.tool_stdout.len(), 47_987);
    }

    #[test]
    fn measured_logical_map_includes_all_65_unavailable_blocks() {
        let (info, blocks) = measured();
        let map = RootfsBadBlockMap::parse(&render(&blocks), "", &info).unwrap();
        assert_eq!(map.unavailable_blocks(), blocks);
        assert_eq!(blocks.len(), 65);
    }

    #[test]
    fn partial_error_and_unsupported_bad_queries_cannot_return_a_map() {
        let (info, blocks) = measured();
        let output = render(&blocks);
        for invalid in [
            output.replace(" BAD", ""),
            output.replace(" 1: 00200000", " 0: 00200000"),
            output.replace(" 1: 00200000", " 1: 00400000"),
            output.replace(" 0: 00000000", " 0: 00000000 RO"),
            output.replace(" 0: 00000000", " 0: 00000000 BAD BAD"),
            output.replace("mtd4\n", "mtd3\n"),
            output.replace("Device is writable: true", "Device is writable: false"),
            output + "could not read bad block info\n",
        ] {
            assert!(RootfsBadBlockMap::parse(&invalid, "", &info).is_err());
        }
        let output = render(&blocks);
        assert!(RootfsBadBlockMap::parse(&output, "could not read bad block info", &info).is_err());
        let truncated = output
            .rsplit_once('\n')
            .unwrap()
            .0
            .rsplit_once('\n')
            .unwrap()
            .0;
        assert!(RootfsBadBlockMap::parse(truncated, "", &info).is_err());
    }

    #[test]
    fn same_count_without_final_bbt_result_cannot_mask_query_failure() {
        let (info, mut blocks) = measured();
        assert_eq!(blocks.pop(), Some(2043));
        blocks.insert(0, 0);
        assert_eq!(blocks.len(), 65);
        assert!(RootfsBadBlockMap::parse(&render(&blocks), "", &info).is_err());
    }

    #[test]
    fn oversized_duplicate_headers_and_count_drift_fail_closed() {
        let (mut info, blocks) = measured();
        let output = render(&blocks);
        let oversized = output.clone() + &" ".repeat(LIMIT);
        let duplicate = output.replace("Name: rootfs", "Name: rootfs\nName: rootfs");
        assert!(RootfsBadBlockMap::parse(&oversized, "", &info).is_err());
        assert!(RootfsBadBlockMap::parse(&duplicate, "", &info).is_err());
        info.bad_blocks += 1;
        assert!(RootfsBadBlockMap::parse(&output, "", &info).is_err());
        info.bad_blocks = u64::MAX;
        assert!(RootfsBadBlockMap::parse(&output, "", &info).is_err());
    }

    #[test]
    fn tool_timeout_never_returns_a_partial_map() {
        let (info, _) = measured();
        let runner = ScriptedToolRunner::new();
        runner.expect_error(
            ToolRequest::new("/usr/sbin/mtdinfo", ["--map", "/dev/mtd4"]),
            Error::Timeout,
        );
        assert!(matches!(
            capture(&runner, info, &Cancellation::default()),
            Err(Error::Timeout)
        ));
        assert_eq!(runner.remaining(), 0);
    }

    #[test]
    fn geometry_failure_prevents_even_read_only_tool_dispatch() {
        let (mut info, _) = measured();
        info.erase_size = 4_194_304;
        let runner = ScriptedToolRunner::new();
        assert!(capture(&runner, info, &Cancellation::default()).is_err());
        assert_eq!(runner.remaining(), 0);
    }

    #[test]
    fn fixed_tool_request_and_cancellation_are_preserved() {
        let (info, blocks) = measured();
        let runner = ScriptedToolRunner::new();
        runner.expect_ok(
            ToolRequest::new("/usr/sbin/mtdinfo", ["--map", "/dev/mtd4"]),
            ToolOutput {
                exit: Some(0),
                stdout: render(&blocks),
                stderr: String::new(),
            },
        );
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            capture(&runner, info, &cancel),
            Err(Error::Cancelled)
        ));
        assert_eq!(runner.remaining(), 1);
        let (info, _) = measured();
        assert_eq!(
            capture(&runner, info, &Cancellation::default())
                .unwrap()
                .validate()
                .unwrap()
                .unavailable_blocks(),
            blocks
        );
        assert_eq!(runner.remaining(), 0);
    }
}
