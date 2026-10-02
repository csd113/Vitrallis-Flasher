//! Fixed raw boot-region diagnostics; no caller-controlled paths or lengths.
use flasher_core::{
    Cancellation, Error,
    recovery::{BootReadback, BootRegion},
    tool::{SystemToolRunner, ToolRequest, ToolRunner},
};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

const PAGE: usize = 16_384;
const OOB: usize = 1_664;
const PAGES: usize = 256;
const BLOCK: u64 = 4_194_304;

fn request(region: BootRegion, output: &Path) -> Result<ToolRequest, Error> {
    let output = output.to_str().ok_or(Error::UnsafePath)?;
    Ok(ToolRequest::new(
        "/usr/sbin/nanddump",
        [
            "--noecc".to_owned(),
            "--oob".to_owned(),
            "--bb=dumpbad".to_owned(),
            "--quiet".to_owned(),
            "--length=4194304".to_owned(),
            format!("--file={output}"),
            format!("/dev/mtd{}", region.index()),
        ],
    )
    .timeout(Duration::from_secs(10)))
}

fn geometry(base: &Path, region: BootRegion) -> Result<(), Error> {
    for (field, expected) in [
        ("size", BLOCK),
        ("erasesize", BLOCK),
        ("writesize", PAGE as u64),
        ("oobsize", OOB as u64),
        ("offset", u64::from(region.index()) * BLOCK),
        ("bad_blocks", 0),
        ("ecc_strength", 56),
        ("ecc_step_size", 1024),
    ] {
        if crate::number(&base.join(field))? != expected {
            return Err(Error::Device);
        }
    }
    Ok(())
}

fn summarize(region: BootRegion, bytes: &[u8]) -> Result<BootReadback, Error> {
    if bytes.len() != PAGES * (PAGE + OOB) {
        return Err(Error::Length);
    }
    let mut data_hash = Sha256::new();
    let mut oob_hash = Sha256::new();
    let mut erased_data_pages = Vec::new();
    let mut erased_oob_pages = Vec::new();
    let mut page_marker_bytes = Vec::with_capacity(PAGES);
    for (index, page) in bytes.as_chunks::<{ PAGE + OOB }>().0.iter().enumerate() {
        let index = u16::try_from(index).map_err(|_| Error::Length)?;
        let (data, oob) = page.split_at(PAGE);
        data_hash.update(data);
        oob_hash.update(oob);
        if data.iter().all(|b| *b == 0xff) {
            erased_data_pages.push(index);
        }
        if oob.iter().all(|b| *b == 0xff) {
            erased_oob_pages.push(index);
        }
        // These are observed bytes, not an assertion of marker semantics.
        page_marker_bytes.push([oob[0], oob[1]]);
    }
    Ok(BootReadback {
        region,
        data_bytes: BLOCK,
        oob_bytes: (PAGES * OOB) as u64,
        data_sha256: format!("{:x}", data_hash.finalize()),
        oob_sha256: format!("{:x}", oob_hash.finalize()),
        interleaved_sha256: format!("{:x}", Sha256::digest(bytes)),
        first_data_bytes: bytes[..32].to_vec(),
        erased_data_pages,
        erased_oob_pages,
        page_marker_bytes,
        corrected_bits_before: 0,
        corrected_bits_after: 0,
        ecc_failures_before: 0,
        ecc_failures_after: 0,
        tool_stderr: String::new(),
    })
}

pub fn read(region: BootRegion, cancel: &Cancellation) -> Result<BootReadback, Error> {
    cancel.check()?;
    let path = format!("/sys/class/mtd/mtd{}", region.index());
    let base = Path::new(&path);
    geometry(base, region)?;
    let corrected = crate::number(&base.join("corrected_bits"))?;
    let failures = crate::number(&base.join("ecc_failures"))?;
    let directory = flasher_core::assets::temporary_directory()?;
    let path = directory.path().join("boot-raw.bin");
    let output = SystemToolRunner.run(&request(region, &path)?, cancel)?;
    let bytes = crate::bytes(&path, (PAGES * (PAGE + OOB)) as u64)?;
    cancel.check()?;
    let mut report = summarize(region, &bytes)?;
    report.corrected_bits_before = corrected;
    report.ecc_failures_before = failures;
    report.corrected_bits_after = crate::number(&base.join("corrected_bits"))?;
    report.ecc_failures_after = crate::number(&base.join("ecc_failures"))?;
    report.tool_stderr = output.stderr;
    if report.ecc_failures_after != failures {
        return Err(Error::Device);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_page_data_and_oob_remain_separate_and_truncation_fails() {
        let mut bytes = vec![0xff; PAGES * (PAGE + OOB)];
        bytes[0] = 0x42;
        bytes[PAGE] = 0x7f;
        let report = summarize(BootRegion::SplPrimary, &bytes).unwrap();
        assert_eq!(report.erased_data_pages.len(), PAGES - 1);
        assert_eq!(report.erased_oob_pages.len(), PAGES - 1);
        assert_eq!(report.page_marker_bytes[0], [0x7f, 0xff]);
        let mut expected = vec![0xff; PAGES * PAGE];
        expected[0] = 0x42;
        assert_eq!(
            report.data_sha256,
            format!("{:x}", Sha256::digest(expected))
        );
        assert!(summarize(BootRegion::SplPrimary, &bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn requests_preserve_physical_offsets_and_never_skip_bad_blocks() {
        for region in [
            BootRegion::SplPrimary,
            BootRegion::SplBackup,
            BootRegion::UBoot,
            BootRegion::FourthBootBlock,
        ] {
            let request = request(region, Path::new("/run/private/boot-raw.bin")).unwrap();
            assert_eq!(request.program, Path::new("/usr/sbin/nanddump"));
            assert!(request.args.contains(&"--noecc".to_owned()));
            assert!(request.args.contains(&"--oob".to_owned()));
            assert!(request.args.contains(&"--bb=dumpbad".to_owned()));
            assert_eq!(
                request.args.last().unwrap(),
                &format!("/dev/mtd{}", region.index())
            );
        }
    }
}
