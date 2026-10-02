//! SID-bound RAM recovery with a restricted original-SPL fallback trial.
mod readback;
mod trial;

use flasher_core::{
    Cancellation, Error,
    recovery::{Channel, Credentials, Inventory, MtdInfo},
    tool::{SystemToolRunner, ToolRequest, ToolRunner},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    net::TcpListener,
    path::Path,
    process::ExitCode,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Recovery stopped: {error}");
            ExitCode::FAILURE
        }
    }
}
pub(crate) fn bytes(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Output);
    }
    Ok(bytes)
}
fn text(path: &Path) -> Result<String, Error> {
    String::from_utf8(bytes(path, 4096)?)
        .map(|s| s.trim_end_matches(['\0', '\n']).to_owned())
        .map_err(|_| Error::Output)
}
pub(crate) fn number(path: &Path) -> Result<u64, Error> {
    text(path)?.parse().map_err(|_| Error::Device)
}
fn inventory(cancel: &Cancellation) -> Result<Inventory, Error> {
    let mut mtd = Vec::new();
    // Do not silently hide a diagnostic alias or changed partition topology.
    // The original-SPL preflight requires exactly five entries and therefore
    // rejects a sixth device, including the read-only physical marker alias.
    for index in mtd_indices(Path::new("/sys/class/mtd"))? {
        cancel.check()?;
        let base = format!("/sys/class/mtd/mtd{index}");
        let base = Path::new(&base);
        mtd.push(MtdInfo {
            index,
            name: text(&base.join("name"))?,
            size: number(&base.join("size"))?,
            erase_size: number(&base.join("erasesize"))?,
            page_size: number(&base.join("writesize"))?,
            oob_size: number(&base.join("oobsize"))?,
            bad_blocks: number(&base.join("bad_blocks"))?,
            bbt_blocks: number(&base.join("bbt_blocks"))?,
            ecc_strength: number(&base.join("ecc_strength"))?,
            ecc_step: number(&base.join("ecc_step_size"))?,
            corrected_bits: number(&base.join("corrected_bits"))?,
            ecc_failures: number(&base.join("ecc_failures"))?,
            offset: number(&base.join("offset"))?,
        });
    }
    let compatible = text(Path::new("/proc/device-tree/compatible"))?
        .split('\0')
        .map(str::to_owned)
        .collect();
    let output = SystemToolRunner.run(
        &ToolRequest::new("/usr/bin/dmesg", ["--color=never"]),
        cancel,
    )?;
    let mtd_report = SystemToolRunner
        .run(&ToolRequest::new("/usr/sbin/mtdinfo", ["--all"]), cancel)?
        .stdout;
    let nanddump_help = SystemToolRunner
        .run(&ToolRequest::new("/usr/sbin/nanddump", ["--help"]), cancel)?
        .stdout;
    Ok(Inventory {
        model: text(Path::new("/proc/device-tree/model"))?,
        compatible,
        kernel: text(Path::new("/proc/sys/kernel/osrelease"))?,
        command_line: text(Path::new("/proc/cmdline"))?,
        memory_reg: memory_reg(Path::new("/proc/device-tree"))?,
        mtd,
        boot_log: output.stdout,
        mtd_report,
        nanddump_help,
    })
}

fn mtd_indices(root: &Path) -> Result<Vec<u8>, Error> {
    let mut devices = [false; 6];
    let mut read_only = [false; 6];
    for (count, entry) in fs::read_dir(root)?.enumerate() {
        if count >= 12 {
            return Err(Error::Length);
        }
        let name = entry?.file_name();
        let name = name.to_str().ok_or(Error::Device)?;
        let suffix = name.strip_prefix("mtd").ok_or(Error::Device)?;
        let (digits, is_read_only) = suffix
            .strip_suffix("ro")
            .map_or((suffix, false), |digits| (digits, true));
        let index: u8 = digits.parse().map_err(|_| Error::Device)?;
        if digits != index.to_string() || usize::from(index) >= devices.len() {
            return Err(Error::Device);
        }
        let seen = if is_read_only {
            &mut read_only
        } else {
            &mut devices
        };
        seen[usize::from(index)] = true;
    }
    if devices[..5].iter().any(|present| !present)
        || read_only
            .iter()
            .zip(devices)
            .any(|(ro, device)| *ro && !device)
    {
        return Err(Error::Device);
    }
    Ok((0_u8..6)
        .filter(|index| devices[usize::from(*index)])
        .collect())
}
fn rootfs_info(cancel: &Cancellation) -> Result<MtdInfo, Error> {
    inventory(cancel)?
        .mtd
        .into_iter()
        .find(|info| info.index == 4)
        .ok_or(Error::Device)
}
fn rootfs_map(
    cancel: &Cancellation,
) -> Result<flasher_core::recovery::bad_blocks::RootfsMapReadback, Error> {
    let info = rootfs_info(cancel)?;
    let report = flasher_core::recovery::bad_blocks::capture(&SystemToolRunner, info, cancel)?;
    // A changed geometry, counter or unavailable count invalidates this snapshot.
    if report.info != rootfs_info(cancel)? {
        return Err(Error::Device);
    }
    Ok(report)
}

fn memory_reg(root: &Path) -> Result<Vec<u8>, Error> {
    // This board's U-Boot creates `memory`, as captured in starting-inventory.
    // Require the exact memory type and a bounded R8 address/capacity pair.
    if text(&root.join("memory/device_type"))? != "memory" {
        return Err(Error::Device);
    }
    let bytes = bytes(&root.join("memory/reg"), 8)?;
    let reg: [u8; 8] = bytes.as_slice().try_into().map_err(|_| Error::Device)?;
    let start = u32::from_be_bytes(reg[..4].try_into().map_err(|_| Error::Device)?);
    let length = u32::from_be_bytes(reg[4..].try_into().map_err(|_| Error::Device)?);
    if start != 0x4000_0000 || !(128 * 1024 * 1024..=512 * 1024 * 1024).contains(&length) {
        return Err(Error::Device);
    }
    Ok(bytes)
}
fn ram_only_mounts(mounts: &str) -> Result<(), Error> {
    if mounts.is_empty() {
        return Err(Error::State);
    }
    for line in mounts.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 6
            || !matches!(
                fields[2],
                "rootfs"
                    | "ramfs"
                    | "tmpfs"
                    | "proc"
                    | "sysfs"
                    | "devtmpfs"
                    | "devpts"
                    | "configfs"
                    | "debugfs"
                    | "securityfs"
                    | "tracefs"
                    | "cgroup"
                    | "cgroup2"
                    | "bpf"
            )
            || fields[0].starts_with("/dev/mtd")
            || fields[0].starts_with("/dev/ubi")
        {
            return Err(Error::State);
        }
    }
    Ok(())
}
fn reboot_to_fel() -> Result<(), Error> {
    ram_only_mounts(&text(Path::new("/proc/mounts"))?)?;
    match fs::read_dir("/sys/class/ubi") {
        Ok(entries) => {
            for entry in entries {
                let name = entry?.file_name();
                let name = name.to_str().ok_or(Error::State)?;
                if name
                    .strip_prefix("ubi")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                {
                    return Err(Error::State);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::Io(error)),
    }
    // All mounts are RAM/kernel pseudo filesystems and no UBI is attached.
    // This fixed SysRq operation has no caller-provided path or contents.
    // The physical FEL bridge must remain connected for BROM re-entry.
    fs::write("/proc/sysrq-trigger", b"b")?;
    Err(Error::State) // A returning kernel did not reboot; never claim success.
}

fn sid_module_request() -> ToolRequest {
    // kmod selects compatibility applets by argv[0], not `kmod insmod`.
    ToolRequest::new(
        "/usr/sbin/insmod",
        ["/usr/lib/modules/6.12.107+deb13-chip/kernel/drivers/nvmem/nvmem_sunxi_sid.ko"],
    )
}

fn watchdog_module_request() -> ToolRequest {
    // The pinned kernel builds its R8 reset handler as a module. Without
    // registering this driver, emergency_restart can hang after stopping USB.
    ToolRequest::new(
        "/usr/sbin/insmod",
        ["/usr/lib/modules/6.12.107+deb13-chip/kernel/drivers/watchdog/sunxi_wdt.ko"],
    )
}

fn run() -> Result<(), Error> {
    let cancel = Cancellation::default();
    let path = Path::new("/run/vitrallis-session");
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() != 64 {
        return Err(Error::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 || metadata.uid() != 0 || metadata.nlink() != 1 {
            return Err(Error::UnsafePath);
        }
    }
    let credentials = Credentials::parse(&bytes(path, 64)?)?;
    // The selected kernel builds SID as a module. No manifest-controlled
    // program, module path or arguments can reach this process boundary.
    SystemToolRunner.run(&sid_module_request(), &cancel)?;
    SystemToolRunner.run(&watchdog_module_request(), &cancel)?;
    if text(Path::new("/sys/class/watchdog/watchdog0/identity"))? != "sunxi-wdt" {
        return Err(Error::Device);
    }
    let mut sid = [0; 16];
    File::open("/sys/bus/nvmem/devices/sunxi-sid0/nvmem")?.read_exact(&mut sid)?;
    credentials.check_sid(&sid)?;
    let binary = bytes(Path::new("/usr/sbin/flasher-recovery"), 32 * 1024 * 1024)?;
    let implementation: [u8; 32] = Sha256::digest(binary).into();
    let listener = TcpListener::bind(("192.168.81.1", 3333))?;
    let mut trials = trial::Service::default();
    eprintln!("Authenticated recovery ready; only fixed original-SPL trial mutations available");
    for incoming in listener.incoming() {
        let stream = incoming?;
        match Channel::accept(stream, &credentials, &implementation, &cancel) {
            Ok(mut channel) => {
                match trials.serve(&mut channel, &credentials, implementation, &cancel) {
                    Ok(()) => reboot_to_fel()?,
                    Err(error) => eprintln!("Recovery connection closed: {error}"),
                }
            }
            Err(error) => eprintln!("Recovery peer rejected: {error}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_includes_extra_alias_and_rejects_unknown_or_missing_devices() {
        let root = tempfile::tempdir().unwrap();
        for index in (0..5).rev() {
            fs::create_dir(root.path().join(format!("mtd{index}"))).unwrap();
            fs::create_dir(root.path().join(format!("mtd{index}ro"))).unwrap();
        }
        assert_eq!(mtd_indices(root.path()).unwrap(), [0, 1, 2, 3, 4]);
        fs::create_dir(root.path().join("mtd5")).unwrap();
        assert_eq!(mtd_indices(root.path()).unwrap(), [0, 1, 2, 3, 4, 5]);
        fs::create_dir(root.path().join("mtd5ro")).unwrap();
        assert_eq!(mtd_indices(root.path()).unwrap(), [0, 1, 2, 3, 4, 5]);
        fs::remove_dir(root.path().join("mtd2")).unwrap();
        assert!(mtd_indices(root.path()).is_err());
        fs::create_dir(root.path().join("mtd2")).unwrap();
        fs::remove_dir(root.path().join("mtd5ro")).unwrap();
        for invalid in ["mtd6", "mtd05", "mtd-1", "mtd", "mtd0extra", "other"] {
            let path = root.path().join(invalid);
            fs::create_dir(&path).unwrap();
            assert!(mtd_indices(root.path()).is_err());
            fs::remove_dir(path).unwrap();
        }
    }

    #[test]
    fn measured_unaddressed_memory_node_is_validated() {
        let root = tempfile::tempdir().unwrap();
        let memory = root.path().join("memory");
        fs::create_dir(&memory).unwrap();
        fs::write(memory.join("device_type"), b"memory\0").unwrap();
        let measured = [0x40, 0, 0, 0, 0x1e, 0, 0xf0, 0];
        fs::write(memory.join("reg"), measured).unwrap();
        assert_eq!(memory_reg(root.path()).unwrap(), measured);
        for invalid in [vec![0; 8], vec![0; 9], vec![0; 7]] {
            fs::write(memory.join("reg"), invalid).unwrap();
            assert!(memory_reg(root.path()).is_err());
        }
        fs::write(memory.join("reg"), measured).unwrap();
        fs::write(memory.join("device_type"), b"other\0").unwrap();
        assert!(memory_reg(root.path()).is_err());
    }
    #[test]
    fn nand_mount_or_malformed_inventory_blocks_ram_restart() {
        ram_only_mounts("rootfs / rootfs rw 0 0\nproc /proc proc rw 0 0\n").unwrap();
        for value in [
            "",
            "invalid",
            "ubi0:rootfs / ubifs rw 0 0",
            "/dev/mtd0 /mnt other rw 0 0",
            "/dev/sda1 /mnt ext4 rw 0 0",
            "unknown /mnt unknown rw 0 0",
        ] {
            assert!(ram_only_mounts(value).is_err());
        }
    }
    #[test]
    fn restart_loader_uses_pinned_reset_module() {
        let request = watchdog_module_request();
        assert_eq!(request.program, Path::new("/usr/sbin/insmod"));
        assert_eq!(
            request.args,
            ["/usr/lib/modules/6.12.107+deb13-chip/kernel/drivers/watchdog/sunxi_wdt.ko"]
        );
        assert_eq!(request.stdin, [] as [u8; 0]);
    }
    #[test]
    fn sid_loader_uses_insmod_applet_name_and_one_fixed_module() {
        let request = sid_module_request();
        assert_eq!(request.program, Path::new("/usr/sbin/insmod"));
        assert_eq!(
            request.args,
            ["/usr/lib/modules/6.12.107+deb13-chip/kernel/drivers/nvmem/nvmem_sunxi_sid.ko"]
        );
        assert_eq!(request.stdin, [] as [u8; 0]);
    }
}
