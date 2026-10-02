//! Read only physical page 255 of the fixed read-only mtd5 alias.
use flasher_core::{
    Cancellation, Error, boot_trial,
    recovery::marker::MarkerReadback,
    tool::{SystemToolRunner, ToolRequest, ToolRunner},
};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
    time::Duration,
};

fn request(output: &Path) -> Result<ToolRequest, Error> {
    Ok(ToolRequest::new(
        "/usr/sbin/nanddump",
        [
            "--noecc".to_owned(),
            "--oob".to_owned(),
            "--bb=dumpbad".to_owned(),
            "--quiet".to_owned(),
            "--start=4177920".to_owned(),
            "--length=16384".to_owned(),
            format!("--file={}", output.to_str().ok_or(Error::UnsafePath)?),
            "/dev/mtd5".to_owned(),
        ],
    )
    .timeout(Duration::from_secs(10)))
}

fn flags() -> Result<u64, Error> {
    let flags = super::text(Path::new("/sys/class/mtd/mtd5/flags"))?;
    u64::from_str_radix(flags.strip_prefix("0x").ok_or(Error::Device)?, 16)
        .map_err(|_| Error::Device)
}

pub fn read(cancel: &Cancellation) -> Result<MarkerReadback, Error> {
    cancel.check()?;
    let mut sid = [0; 16];
    File::open("/sys/bus/nvmem/devices/sunxi-sid0/nvmem")?.read_exact(&mut sid)?;
    if sid != boot_trial::SID {
        return Err(Error::Device);
    }
    let ubi = match fs::read_dir("/sys/class/ubi") {
        Ok(entries) => entries
            .map(|entry| entry?.file_name().into_string().map_err(|_| Error::State))
            .collect::<Result<Vec<_>, Error>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(Error::Io(error)),
    };
    boot_trial::ram_only(&super::text(Path::new("/proc/mounts"))?, &ubi)?;
    let inventory = super::inventory(cancel)?;
    if inventory.mtd.len() != 6 {
        return Err(Error::Device);
    }
    let info = inventory.mtd.into_iter().last().ok_or(Error::Device)?;
    let before_flags = flags()?;
    MarkerReadback::validate_alias(&info, before_flags)?;
    // Rootfs logical block 222 corresponds to the fixed physical block 226.
    // Require a fresh complete map rather than treating the source offset alone
    // as proof that this running kernel reports that block unavailable.
    if !super::rootfs_map(cancel)?
        .validate()?
        .unavailable_blocks()
        .contains(&222)
    {
        return Err(Error::Device);
    }
    let directory = flasher_core::assets::temporary_directory()?;
    let path = directory.path().join("marker-raw.bin");
    let output = SystemToolRunner.run(&request(&path)?, cancel)?;
    if output.exit != Some(0) || !output.stdout.is_empty() || !output.stderr.is_empty() {
        return Err(Error::Output);
    }
    let raw = super::bytes(&path, 18_048)?;
    let report = MarkerReadback::from_raw(info, before_flags, &raw)?;
    let after = super::inventory(cancel)?;
    if after.mtd.len() != 6 || after.mtd.last() != Some(&report.info) || flags()? != before_flags {
        return Err(Error::Device);
    }
    cancel.check()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_request_is_raw_fixed_last_page_and_dumpbad_without_write() {
        let request = request(Path::new("/run/private/marker-raw.bin")).unwrap();
        assert_eq!(request.program, Path::new("/usr/sbin/nanddump"));
        assert_eq!(
            request.args,
            [
                "--noecc",
                "--oob",
                "--bb=dumpbad",
                "--quiet",
                "--start=4177920",
                "--length=16384",
                "--file=/run/private/marker-raw.bin",
                "/dev/mtd5"
            ]
        );
        assert_eq!(request.timeout, Duration::from_secs(10));
        assert_eq!(request.stdin, [] as [u8; 0]);
    }
}
