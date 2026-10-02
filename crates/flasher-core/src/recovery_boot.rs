//! Reviewed RAM-only recovery bootstrap; no NAND command exists in this path.
//!
//! The current diagnostic policy pins the measured macOS utility build and
//! immutable recovery template. It grants no physical manifest approval.
use crate::{
    Cancellation, Error,
    assets::{regular_components, temporary_directory},
    device::{Device, select},
    fel::FelTransport,
    fel_native::NativeFel,
    recovery::Credentials,
    tool::{SystemToolRunner, ToolRequest, ToolRunner},
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const TOOL_HASH: &str = "1bd55a8b40b629cd5a374ffe9698eb21a894f14e0710d38e07e10fd9e7d2d059";
const TEMPLATE_HASH: &str = "b9e128e6c2b37dac13fe0f5354749a760bd0a2ad8eaea5d3441bd0a02bbe5315";
const TEMPLATE_BYTES: usize = 41_687_740;
const DAEMON_HASH: &str = "3247bc2c94786016532912f9825b616d09bfa2252ee4400cbdf1564f383d2dc0";
const MARKER_DTB_HASH: &str = "55b8346c340692bb22f06dc020bdf36445341237b752e27fe0486f3e751450fd";
const MARKER_DTB_BYTES: usize = 25_639;
const BOOT_SCRIPT: &[u8] = b"echo == Vitrallis RAM-only recovery ==\nsetenv bootargs console=ttyS0,115200 panic=0 rdinit=/init\nfdt addr 0x43000000\nfdt resize 65536\nfdt apply 0x43200000\nbootz 0x42000000 0x43300000 0x43000000\n";
const INPUTS: [(&str, &str, &str, usize); 4] = [
    (
        "artifacts/u-boot-sunxi-with-spl.bin",
        "uboot.bin",
        "23d2730799a109946753147413153687877e41265981833c4466d034d2279d05",
        721_776,
    ),
    (
        "artifacts/vmlinuz-6.12.107+deb13-chip",
        "kernel.bin",
        "d3c044d80034bb10513c5e987afe208b489c8f36c76a222f223ff63d1f8e1540",
        6_697_472,
    ),
    (
        "artifacts/sun5i-r8-chip.dtb",
        "dtb.bin",
        "0132f7fa312542c374c7e5dfec2a1a12893b9cbe490d22e165fd7a89de51762e",
        25537,
    ),
    (
        "inputs/x-chip-pocketchip.dtbo",
        "overlay.bin",
        "3230ea7ffb660bb559e8b938a3e4d9ea5e3a2f484a512f345564735f85a23e24",
        0,
    ),
];

fn verified(path: &Path, digest: &str, bound: usize, length: usize) -> Result<Vec<u8>, Error> {
    if !path.is_absolute() {
        return Err(Error::UnsafePath);
    }
    regular_components(path)?;
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::UnsafePath);
    }
    let mut data = Vec::new();
    file.take(u64::try_from(bound).map_err(|_| Error::Length)? + 1)
        .read_to_end(&mut data)?;
    if data.is_empty() || data.len() > bound || (length != 0 && data.len() != length) {
        return Err(Error::Length);
    }
    if format!("{:x}", Sha256::digest(&data)) != digest {
        return Err(Error::Hash);
    }
    Ok(data)
}
fn sid(device: &Device) -> Result<[u8; 16], Error> {
    let mut sid = [0; 16];
    let words: Vec<_> = device.sid.split(':').collect();
    if words.len() != 4 {
        return Err(Error::Device);
    }
    for (index, word) in words.into_iter().enumerate() {
        let value = u32::from_str_radix(word, 16).map_err(|_| Error::Device)?;
        sid[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    Ok(sid)
}
fn verified_marker_dtb(path: &Path, device: &Device) -> Result<Vec<u8>, Error> {
    if sid(device)? != crate::boot_trial::SID {
        return Err(Error::Device);
    }
    verified(path, MARKER_DTB_HASH, MARKER_DTB_BYTES, MARKER_DTB_BYTES)
}
fn crc32(data: &[u8]) -> u32 {
    // IEEE CRC-32 used by legacy U-Boot uImage, not an authentication primitive.
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn uimage(payload: &[u8], image_type: u8, compression: u8) -> Result<Vec<u8>, Error> {
    let mut header = [0; 64];
    header[..4].copy_from_slice(&0x2705_1956_u32.to_be_bytes());
    header[12..16].copy_from_slice(
        &u32::try_from(payload.len())
            .map_err(|_| Error::Length)?
            .to_be_bytes(),
    );
    header[24..28].copy_from_slice(&crc32(payload).to_be_bytes());
    header[28..32].copy_from_slice(&[5, 2, image_type, compression]);
    header[32..49].copy_from_slice(b"Vitrallis RAM v1\0");
    let checksum = crc32(&header);
    header[4..8].copy_from_slice(&checksum.to_be_bytes());
    Ok([header.as_slice(), payload].concat())
}
fn cpio(name: &str, payload: &[u8], mode: u32) -> Result<Vec<u8>, Error> {
    let size = u32::try_from(payload.len()).map_err(|_| Error::Length)?;
    let namesize = u32::try_from(name.len() + 1).map_err(|_| Error::Length)?;
    let mut bytes = b"070701".to_vec();
    for field in [1, mode, 0, 0, 1, 0, size, 0, 0, 0, 0, namesize, 0] {
        write!(&mut bytes, "{field:08x}")?;
    }
    bytes.extend_from_slice(name.as_bytes());
    bytes.push(0);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes.extend_from_slice(payload);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    Ok(bytes)
}
fn session_image(template: &[u8], secret: &[u8; 64]) -> Result<Vec<u8>, Error> {
    // Linux's documented initramfs grammar accepts a compressed archive
    // followed by an aligned uncompressed newc archive. Only one fixed
    // root-owned mode-0600 configuration file is appended to the pinned image.
    let mut payload = template.get(64..).ok_or(Error::Length)?.to_vec();
    payload.resize(payload.len().next_multiple_of(4), 0);
    payload.extend_from_slice(&cpio("run/vitrallis-session", secret, 0o100_600)?);
    payload.extend_from_slice(&cpio("TRAILER!!!", &[], 0)?);
    uimage(&payload, 3, 1)
}

/// Boots only the pinned restricted diagnostic implementation on the freshly selected SID.
/// Returned private diagnostics retain session credentials for reconnection.
/// # Errors
/// Rejects changed inputs, ambiguous identity, cancellation and tool failures.
/// This does not authorize NAND execution or approve a physical manifest.
pub fn boot(
    assets: &Path,
    template: &Path,
    daemon: &Path,
    tool: &Path,
    cancel: &Cancellation,
) -> Result<PathBuf, Error> {
    boot_with_dtb(assets, template, daemon, tool, None, cancel)
}

/// Boots the fixed read-only marker alias DTB on the measured sacrificial SID.
/// # Errors
/// Rejects any changed diagnostic DTB, inputs, identity or failed RAM operation.
/// The extra alias denies original-SPL trial writes; no manifest is approved.
pub fn boot_marker(
    assets: &Path,
    template: &Path,
    daemon: &Path,
    tool: &Path,
    marker_dtb: &Path,
    cancel: &Cancellation,
) -> Result<PathBuf, Error> {
    boot_with_dtb(assets, template, daemon, tool, Some(marker_dtb), cancel)
}

fn boot_with_dtb(
    assets: &Path,
    template: &Path,
    daemon: &Path,
    tool: &Path,
    marker_dtb: Option<&Path>,
    cancel: &Cancellation,
) -> Result<PathBuf, Error> {
    cancel.check()?;
    let selected = select(&NativeFel.discover(cancel)?)?;
    let marker_dtb = marker_dtb
        .map(|path| verified_marker_dtb(path, &selected))
        .transpose()?;
    let template = verified(template, TEMPLATE_HASH, 40 * 1024 * 1024, TEMPLATE_BYTES)?;
    let daemon = verified(daemon, DAEMON_HASH, 32 * 1024 * 1024, 0)?;
    let _tool_bytes = verified(tool, TOOL_HASH, 4 * 1024 * 1024, 0)?;
    let staging = temporary_directory()?;
    for (relative, staged, digest, length) in INPUTS {
        cancel.check()?;
        let bytes = verified(&assets.join(relative), digest, 16 * 1024 * 1024, length)?;
        std::fs::write(staging.path().join(staged), bytes)?;
    }
    if let Some(bytes) = &marker_dtb {
        std::fs::write(staging.path().join("dtb.bin"), bytes)?;
    }
    let (_, mut secret) = Credentials::generate(sid(&selected)?)?;
    let image = session_image(&template, &secret)?;
    for (name, data) in [
        ("session.bin", secret.as_slice()),
        ("initrd.uimage", image.as_slice()),
        ("daemon.bin", daemon.as_slice()),
    ] {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.path().join(name))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(data)?;
        file.sync_all()?;
    }
    secret.fill(0);
    let mut script = u32::try_from(BOOT_SCRIPT.len())
        .map_err(|_| Error::Length)?
        .to_be_bytes()
        .to_vec();
    script.extend_from_slice(&[0; 4]);
    script.extend_from_slice(BOOT_SCRIPT);
    std::fs::write(staging.path().join("boot.scr"), uimage(&script, 6, 0)?)?;
    let fresh = select(&NativeFel.discover(cancel)?)?;
    if fresh.sid != selected.sid {
        return Err(Error::Device);
    }
    let path = |name: &str| staging.path().join(name).to_string_lossy().into_owned();
    let request = ToolRequest::new(
        tool,
        [
            "--sid".to_owned(),
            fresh.sid,
            "uboot".to_owned(),
            path("uboot.bin"),
            "write".to_owned(),
            "0x42000000".to_owned(),
            path("kernel.bin"),
            "write".to_owned(),
            "0x43000000".to_owned(),
            path("dtb.bin"),
            "write".to_owned(),
            "0x43100000".to_owned(),
            path("boot.scr"),
            "write".to_owned(),
            "0x43200000".to_owned(),
            path("overlay.bin"),
            "write".to_owned(),
            "0x43300000".to_owned(),
            path("initrd.uimage"),
        ],
    )
    .timeout(Duration::from_secs(120));
    let output = SystemToolRunner.run(&request, cancel)?;
    let log = serde_json::to_vec_pretty(&serde_json::json!({
        "exit": output.exit, "stdout": output.stdout, "stderr": output.stderr,
        "sid": selected.sid, "template_sha256": TEMPLATE_HASH,
        "daemon_sha256": DAEMON_HASH,
        "marker_alias": marker_dtb.is_some(),
    }))
    .map_err(|_| Error::Output)?;
    std::fs::write(staging.path().join("boot-tool-log.json"), log)?;
    Ok(staging.keep())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_dtb_pins_match_preparation_and_reject_changed_input_or_sid() {
        let metadata: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/hynix-marker-probe-preparation.json"
        ))
        .unwrap();
        let probe = &metadata["diagnostic_preparation"];
        assert_eq!(probe["derived_sha256"], MARKER_DTB_HASH);
        assert_eq!(
            probe["derived_bytes"].as_u64().unwrap(),
            MARKER_DTB_BYTES as u64
        );
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("probe.dtb");
        let mut device = Device {
            bus: 0,
            address: 1,
            soc: "A13".into(),
            sid: "16254217:50303858:31333030:0e0288c0".into(),
        };
        std::fs::write(&path, vec![0; MARKER_DTB_BYTES]).unwrap();
        assert!(matches!(
            verified_marker_dtb(&path, &device),
            Err(Error::Hash)
        ));
        std::fs::write(&path, [0; 1]).unwrap();
        assert!(matches!(
            verified_marker_dtb(&path, &device),
            Err(Error::Length)
        ));
        device.sid = "00000001:00000001:00000001:00000001".into();
        assert!(matches!(
            verified_marker_dtb(&path, &device),
            Err(Error::Device)
        ));
    }
    #[test]
    fn bootstrap_pins_match_the_recorded_template_bytes_and_implementation() {
        let metadata: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/evidence/batch3/recovery-template-v9.json"
        ))
        .unwrap();
        assert_eq!(
            metadata["image_bytes"].as_u64().unwrap(),
            TEMPLATE_BYTES as u64
        );
        assert_eq!(metadata["protocol"], 9);
        assert_eq!(
            metadata["release_artifact_sha256"],
            crate::boot_trial::release::ARTIFACT
        );
        assert_eq!(
            metadata["release_program_sha256"],
            crate::boot_trial::release::PROGRAM
        );
        assert_eq!(
            metadata["release_manifest_sha256"],
            crate::boot_trial::release::MANIFEST
        );
        assert_eq!(
            metadata["uboot_pair_sha256"],
            crate::boot_trial::uboot::BUNDLE
        );
        assert_eq!(
            metadata["uboot_release_program_sha256"],
            crate::boot_trial::uboot::RELEASE_PROGRAM
        );
        assert_eq!(
            metadata["uboot_original_program_sha256"],
            crate::boot_trial::ORIGINAL_UBOOT
        );
        assert_eq!(metadata["image_sha256"].as_str().unwrap(), TEMPLATE_HASH);
        assert_eq!(metadata["daemon_sha256"].as_str().unwrap(), DAEMON_HASH);
    }

    #[test]
    fn legacy_crc_known_answer_and_session_archive_has_fixed_permissions() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let entry = cpio("run/vitrallis-session", &[3; 64], 0o100_600).unwrap();
        assert_eq!(&entry[14..22], b"00008180");
        assert_eq!(&entry[54..62], b"00000040");
        assert!(entry.ends_with(&[3; 64]));
        let image = uimage(b"abc", 3, 1).unwrap();
        assert_eq!(&image[64..], b"abc");
        assert_eq!(&image[12..16], &3_u32.to_be_bytes());
    }
    #[test]
    fn reviewed_script_cannot_touch_nand_or_boot_existing_rootfs() {
        let script = std::str::from_utf8(BOOT_SCRIPT).unwrap();
        for prohibited in ["nand", "ubi", "root=", "run bootcmd", "saveenv"] {
            assert!(!script.contains(prohibited));
        }
        assert!(script.contains("rdinit=/init"));
    }
}
