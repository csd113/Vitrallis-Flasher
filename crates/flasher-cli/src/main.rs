//! Minimal CLI; USB discovery is opt-in and installation is simulated only.
use flasher_core::{
    Cancellation, Error,
    assets::{Cache, acquire},
    device::{RealFel, select},
    fel::FelTransport,
    fel_native::NativeFel,
    manifest::Manifest,
    platform::usb_guidance,
    process::SunxiTool,
    profile::Profile,
    session::{MockFel, Session},
    simulation,
};
use std::{
    env,
    io::{self, BufRead, Read, Write},
    path::Path,
    process::ExitCode,
};
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
fn run() -> Result<(), Error> {
    let args: Vec<_> = env::args().skip(1).collect();
    let cancel = Cancellation::default();
    let interrupt = cancel.clone();
    ctrlc::set_handler(move || interrupt.cancel()).map_err(|_| Error::State)?;
    match args.as_slice() {
        [] => help(),
        [command] if command == "help" || command == "--help" => help(),
        [command] if command == "releases" => {
            for release in
                flasher_core::releases::catalog(flasher_core::releases::Channel::Simulation)
            {
                println!("{}: {}", release.id, release.label);
            }
            println!("Approved physical releases: none");
        }
        [command] if command == "doctor" => {
            println!(
                "{}\nPhysical flashing: blocked. Approved release catalog: empty.",
                usb_guidance()
            );
        }
        [command] if command == "rootfs-audit-tar" => audit_rootfs(&cancel, false)?,
        [command] if command == "rootfs-audit-gzip" => audit_rootfs(&cancel, true)?,
        [command] if command == "detect" || command == "fel-probe" => {
            native_diagnostic(command, &cancel)?;
        }
        [command, path] if is_saved_boot_diagnostic(command) => {
            saved_boot_diagnostic(command, path, &cancel)?;
        }
        [command, primary, backup, digest, output] if command == "boot0-recover-source" => {
            recover_boot0_source(primary, backup, digest, output, &cancel)?;
        }
        [command, path] if command == "trial-journal-read" => {
            read_trial_journal(path, &cancel)?;
        }
        [command, assets, template, daemon, tool] if command == "recovery-boot" => {
            boot_recovery_diagnostic(assets, template, daemon, tool, &cancel)?;
        }
        [command, assets, template, daemon, tool, dtb] if command == "recovery-boot-marker" => {
            boot_marker_diagnostic(assets, template, daemon, tool, dtb, &cancel)?;
        }
        [command, config, binary, operation] if command == "recovery-spl-preflight" => {
            let response = flasher_core::recovery::spl_trial::prepare(
                Path::new(config),
                Path::new(binary),
                trial_operation(operation)?,
                &cancel,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&response).map_err(|_| Error::Recovery)?
            );
        }
        [command, config, binary, operation, journal] if command == "recovery-spl-trial" => {
            spl_trial_diagnostic(config, binary, operation, journal, &cancel)?;
        }
        [command, config, binary, region] if command == "recovery-boot-readback" => {
            boot_readback_diagnostic(config, binary, region, &cancel)?;
        }
        [command, config, binary] if is_recovery_diagnostic(command) => {
            read_recovery_diagnostic(command, config, binary, &cancel)?;
        }
        [command, tool] if command == "detect" => {
            external_detect(tool, &cancel)?;
        }
        [command, path] if command == "validate" => {
            let manifest = Manifest::open(Path::new(path))?;
            println!(
                "Valid contract: {} SHA-256 {} (not approval to flash)",
                manifest.release(),
                manifest.digest()
            );
        }
        [command, manifest, cache] if command == "fetch" => {
            fetch(manifest, cache, None, &cancel)?;
        }
        [command, manifest, cache, offline] if command == "offline" => {
            fetch(manifest, cache, Some(Path::new(offline)), &cancel)?;
        }
        [command] if command == "simulate" => {
            simulate(&cancel, Profile::default())?;
        }
        [command, flag, profile] if command == "simulate" && flag == "--profile" => {
            simulate(&cancel, Profile::parse(profile)?)?;
        }
        [command, flag, profile] if command == "upgrade" && flag == "--profile" => {
            println!("Requested: {}", Profile::parse(profile)?.label());
            return Err(Error::PhysicalBlocked);
        }
        _ => {
            help();
            return Err(Error::State);
        }
    }
    Ok(())
}
fn audit_rootfs(cancel: &Cancellation, compressed: bool) -> Result<(), Error> {
    let inspection = if compressed {
        flasher_core::rootfs::inspect_gzip(io::stdin().lock(), cancel)?
    } else {
        flasher_core::rootfs::inspect(io::stdin().lock(), cancel)?
    };
    println!(
        "{}",
        serde_json::json!({
            "members": inspection.entries().len(),
            "regular_file_bytes": inspection.file_bytes,
            "semantic_sha256": inspection.semantic_sha256,
            "filesystem_mutated": false,
            "production_reflash": false,
        })
    );
    Ok(())
}

fn is_saved_boot_diagnostic(command: &str) -> bool {
    matches!(
        command,
        "boot0-readback"
            | "boot0-check-restoration"
            | "boot0-check-release-hynix"
            | "uboot-check-pair"
    )
}

fn is_recovery_diagnostic(command: &str) -> bool {
    matches!(
        command,
        "recovery-inventory"
            | "recovery-ping"
            | "recovery-return-to-fel"
            | "recovery-rootfs-map"
            | "recovery-physical-marker"
    )
}

fn read_recovery_diagnostic(
    command: &str,
    config: &str,
    binary: &str,
    cancel: &Cancellation,
) -> Result<(), Error> {
    let diagnostic = match command {
        "recovery-ping" => flasher_core::recovery::diagnostic_ping,
        "recovery-rootfs-map" => flasher_core::recovery::diagnostic_rootfs_map,
        "recovery-physical-marker" => flasher_core::recovery::diagnostic_physical_marker,
        "recovery-return-to-fel" => flasher_core::recovery::diagnostic_return_to_fel,
        _ => flasher_core::recovery::diagnostic_inventory,
    };
    let response = diagnostic(Path::new(config), Path::new(binary), cancel)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|_| Error::Recovery)?
    );
    Ok(())
}

fn spl_trial_diagnostic(
    config: &str,
    binary: &str,
    operation: &str,
    journal: &str,
    cancel: &Cancellation,
) -> Result<(), Error> {
    let operation = trial_operation(operation)?;
    let response = flasher_core::recovery::spl_trial::run(
        Path::new(config),
        Path::new(binary),
        operation,
        Path::new(journal),
        cancel,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|_| Error::Recovery)?
    );
    Ok(())
}

fn trial_operation(operation: &str) -> Result<flasher_core::boot_trial::Operation, Error> {
    use flasher_core::boot_trial::Operation;
    match operation {
        "erase-primary" => Ok(Operation::ErasePrimary),
        "restore-primary" => Ok(Operation::RestorePrimary),
        "erase-backup" => Ok(Operation::EraseBackup),
        "restore-backup" => Ok(Operation::RestoreBackup),
        "program-release-primary" => Ok(Operation::ProgramReleasePrimary),
        "erase-backup-for-release-primary" => Ok(Operation::EraseBackupForReleasePrimary),
        "restore-backup-for-release-primary" => Ok(Operation::RestoreBackupForReleasePrimary),
        "program-release-uboot-backup" => Ok(Operation::ProgramReleaseUbootBackup),
        "erase-original-uboot-primary" => Ok(Operation::EraseOriginalUbootPrimary),
        "restore-original-uboot-primary" => Ok(Operation::RestoreOriginalUbootPrimary),
        "program-release-uboot-primary" => Ok(Operation::ProgramReleaseUbootPrimary),
        "erase-release-uboot-backup" => Ok(Operation::EraseReleaseUbootBackup),
        "restore-release-uboot-backup" => Ok(Operation::RestoreReleaseUbootBackup),
        _ => Err(Error::State),
    }
}

fn external_detect(tool: &str, cancel: &Cancellation) -> Result<(), Error> {
    let backend = RealFel::new(
        SunxiTool::open(Path::new(tool)).inspect_err(|_| eprintln!("{}", usb_guidance()))?,
    );
    let devices = backend
        .discover(cancel)
        .inspect_err(|_| eprintln!("{}", usb_guidance()))?;
    let selected = select(&devices)?;
    println!(
        "FEL candidate: {} {:03}:{:03} SID {}\nPocketCHIP board and NAND remain unverified. Physical writes are blocked.",
        selected.soc, selected.bus, selected.address, selected.sid
    );
    Ok(())
}

fn read_trial_journal(path: &str, cancel: &Cancellation) -> Result<(), Error> {
    let report = flasher_core::boot_trial::journal::read(Path::new(path), cancel)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|_| Error::Recovery)?
    );
    Ok(())
}

fn saved_boot_diagnostic(command: &str, path: &str, cancel: &Cancellation) -> Result<(), Error> {
    if command == "uboot-check-pair" {
        check_uboot_pair(path, cancel)?;
    } else if command == "boot0-check-release-hynix" {
        let _snapshot =
            flasher_core::boot_trial::release::LockedHynixSplFile::open(Path::new(path), cancel)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pins": flasher_core::boot_trial::release::Pins::expected(),
                "image_bytes": flasher_core::boot_trial::IMAGE_BYTES,
                "all_four_copies_decoded_without_errors": true,
                "nand_mutation_performed": false,
            }))
            .map_err(|_| Error::Recovery)?
        );
    } else if command == "boot0-check-restoration" {
        let _snapshot = flasher_core::boot_trial::OriginalSplFile::open(Path::new(path), cancel)?;
        let report = serde_json::json!({
            "restoration_image_sha256": flasher_core::boot_trial::RESTORATION_IMAGE,
            "original_spl_sha256": flasher_core::boot_trial::ORIGINAL_SPL,
            "image_bytes": flasher_core::boot_trial::IMAGE_BYTES,
            "all_four_copies_decoded_without_errors": true,
            "nand_mutation_performed": false,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|_| Error::Recovery)?
        );
    } else {
        let report = flasher_core::boot0::read_saved_region(Path::new(path), cancel)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|_| Error::Recovery)?
        );
    }
    Ok(())
}

fn check_uboot_pair(path: &str, cancel: &Cancellation) -> Result<(), Error> {
    use flasher_core::boot_trial::uboot;
    let mut images = uboot::Images::open(Path::new(path), cancel)?;
    images.revalidate(cancel)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "bundle_sha256": uboot::BUNDLE,
            "original_program_sha256": flasher_core::boot_trial::ORIGINAL_UBOOT,
            "release_program_sha256": uboot::RELEASE_PROGRAM,
            "program_bytes": uboot::PROGRAM_BYTES,
            "nand_mutation_performed": false,
            "physical_manifest_approved": false,
        }))
        .map_err(|_| Error::Recovery)?
    );
    Ok(())
}

fn recover_boot0_source(
    primary: &str,
    backup: &str,
    digest: &str,
    output: &str,
    cancel: &Cancellation,
) -> Result<(), Error> {
    flasher_core::boot0::recover_saved_source(
        Path::new(primary),
        Path::new(backup),
        digest,
        Path::new(output),
        cancel,
    )?;
    println!("Verified all eight SPL copies; published private 16 KiB source: {output}");
    Ok(())
}

fn boot_recovery_diagnostic(
    assets: &str,
    template: &str,
    daemon: &str,
    tool: &str,
    cancel: &Cancellation,
) -> Result<(), Error> {
    let private = flasher_core::recovery_boot::boot(
        Path::new(assets),
        Path::new(template),
        Path::new(daemon),
        Path::new(tool),
        cancel,
    )?;
    println!(
        "RAM-only boot dispatched; authenticated inventory still required.\nPrivate session directory: {}",
        private.display()
    );
    Ok(())
}

fn boot_marker_diagnostic(
    assets: &str,
    template: &str,
    daemon: &str,
    tool: &str,
    dtb: &str,
    cancel: &Cancellation,
) -> Result<(), Error> {
    let private = flasher_core::recovery_boot::boot_marker(
        Path::new(assets),
        Path::new(template),
        Path::new(daemon),
        Path::new(tool),
        Path::new(dtb),
        cancel,
    )?;
    println!(
        "Read-only marker RAM boot dispatched; authenticated inventory still required.\nPrivate session directory: {}",
        private.display()
    );
    Ok(())
}

fn native_diagnostic(command: &str, cancel: &Cancellation) -> Result<(), Error> {
    let transport = NativeFel;
    let device = select(&transport.discover(cancel)?)?;
    let version = transport.brom_version(&device, cancel)?;
    println!(
        "Native FEL candidate: {} {:03}:{:03} SID {}\nBROM SoC {:#06x}, protocol {}, scratchpad {:#010x}\nBROM packet: {:02x?}",
        device.soc,
        device.bus,
        device.address,
        device.sid,
        version.soc_id,
        version.protocol,
        version.scratchpad,
        version.raw
    );
    if command == "fel-probe" {
        transport.diagnostic_probe(&device, cancel)?;
        println!(
            "256-byte scratch SRAM upload/readback, bx lr execution and restoration verified."
        );
    }
    println!("Board, NAND and recovery policy remain unverified; physical flashing is blocked.");
    Ok(())
}

fn boot_readback_diagnostic(
    config: &str,
    binary: &str,
    region: &str,
    cancel: &Cancellation,
) -> Result<(), Error> {
    use flasher_core::recovery::{BootRegion, ReadInterpretation};
    let interpretation = match region {
        "uboot-corrected" | "fourth-boot-block-corrected" => ReadInterpretation::KernelCorrected,
        "spl-primary-corrected" | "spl-backup-corrected" => ReadInterpretation::Boot0Corrected,
        _ => ReadInterpretation::Raw,
    };
    let region = match region {
        "spl-primary" | "spl-primary-corrected" => BootRegion::SplPrimary,
        "spl-backup" | "spl-backup-corrected" => BootRegion::SplBackup,
        "uboot" | "uboot-corrected" => BootRegion::UBoot,
        "fourth-boot-block" | "fourth-boot-block-corrected" => BootRegion::FourthBootBlock,
        _ => return Err(Error::Device),
    };
    let response = flasher_core::recovery::diagnostic_boot_readback(
        Path::new(config),
        Path::new(binary),
        region,
        interpretation,
        cancel,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|_| Error::Recovery)?
    );
    Ok(())
}

fn help() {
    println!(
        "Vitrallis Flasher\n\n  doctor\n  rootfs-audit-tar (read decompressed tar from stdin; no extraction)\n  rootfs-audit-gzip (bounded native gzip inspection from stdin; no extraction)\n  releases\n  detect (native USB, read-only)\n  fel-probe (scratch SRAM diagnostic)\n  boot0-readback /absolute/path/to/mtd-raw-data.bin (saved 4 MiB SPL diagnostic)\n  boot0-check-restoration /absolute/path/to/original-spl.nand (fixed pinned restoration candidate)\n  boot0-check-release-hynix /absolute/path/to/spl-hynix.nand (fixed locked candidate)\n  uboot-check-pair /absolute/path/to/pinned-pair.bin (read-only original/release snapshots)\n  boot0-recover-source /primary /backup expected-sha256 /private-output (verify eight saved copies)\n  recovery-spl-preflight /private-session/session.bin /private-session/daemon.bin erase-primary|restore-primary|erase-backup|restore-backup (prepare then disconnect; no write)\n  recovery-spl-trial /private-session/session.bin /private-session/daemon.bin erase-primary|restore-primary|erase-backup|restore-backup /new-private-journal.jsonl (fixed sacrificial-unit diagnostic)\n  trial-journal-read /absolute/path/to/private-journal.jsonl (read-only inspection)\n  recovery-boot /assets /template /daemon /sunxi-fel (pinned RAM recovery diagnostic)\n  recovery-boot-marker /assets /template /daemon /sunxi-fel /pinned-marker.dtb (fixed read-only alias; denies SPL trials)\n  recovery-physical-marker /private-session/session.bin /private-session/daemon.bin (fixed raw last-page observation)\n  recovery-boot-readback /private-session/session.bin /private-session/daemon.bin spl-primary|spl-backup|spl-primary-corrected|spl-backup-corrected|uboot|uboot-corrected|fourth-boot-block|fourth-boot-block-corrected\n  recovery-ping /private-session/session.bin /private-session/daemon.bin\n  recovery-inventory /private-session/session.bin /private-session/daemon.bin\n  recovery-rootfs-map /private-session/session.bin /private-session/daemon.bin (read-only logical eraseblock map)\n  recovery-return-to-fel /private-session/session.bin /private-session/daemon.bin (keep FEL bridge connected)\n  detect /absolute/path/to/sunxi-fel\n  validate manifest.json\n  fetch manifest.json existing-private-cache\n  offline manifest.json existing-private-cache offline-directory\n  simulate [--profile stock|vitrallis-default]\n  upgrade --profile stock|vitrallis-default (blocked)\n\nStock PocketHome is the default choice. Production flashing remains blocked. SPL diagnostics are restricted to the measured sacrificial unit and exact original/locked Hynix artifacts; no release approval is granted. Release trial operations: program-release-primary, erase-backup-for-release-primary, restore-backup-for-release-primary. U-Boot trial operations: program-release-uboot-backup, erase-original-uboot-primary, restore-original-uboot-primary, program-release-uboot-primary, erase-release-uboot-backup, restore-release-uboot-backup. Simulation still requires typed ERASE confirmation. Downloads require an explicitly selected manifest; checksums establish integrity, not publisher trust."
    );
}
fn fetch(
    path: &str,
    directory: &str,
    offline: Option<&Path>,
    cancel: &Cancellation,
) -> Result<(), Error> {
    let manifest = Manifest::open(Path::new(path))?;
    let cache = Cache::open(Path::new(directory))?;
    let verified = acquire(&manifest, &cache, offline, cancel, |role, done, total| {
        if done == total {
            println!("Verified {role:?}: {total} bytes");
        }
    })?;
    println!(
        "{} verified assets; physical flashing remains blocked.",
        verified.len()
    );
    Ok(())
}
fn simulate(cancel: &Cancellation, profile: Profile) -> Result<(), Error> {
    let directory = flasher_core::assets::temporary_directory()?;
    simulate_in(directory.path(), cancel, profile)
}
fn simulate_in(directory: &Path, cancel: &Cancellation, profile: Profile) -> Result<(), Error> {
    let cache = Cache::open(directory)?;
    let verified = simulation::prepare_profile(&cache, cancel, profile)?;
    println!(
        "Requested: {}\n{}\nThis fixture does not install an OS or change the startup desktop.",
        verified.manifest().profile().label(),
        verified.manifest().profile().description()
    );
    let mut session = Session::new(MockFel::default());
    session.preflight(verified, cancel, |e| {
        println!("{}: {}", e.stage.label(), e.message);
    })?;
    if let Some(plan) = session.plan() {
        println!("\nReview-only plan for this fixture (no executor exists):");
        for step in plan.steps() {
            println!(
                "  {:?} source={:?} placement={:?} length={:?}",
                step.kind, step.source, step.placement, step.length
            );
        }
        println!(
            "  {} unresolved physical gates; execution is blocked in this build.",
            plan.gates().len()
        );
    }
    let confirmation = session.confirmation().ok_or(Error::State)?;
    println!("\nSIMULATION ONLY. Type this exact phrase:\n{confirmation}");
    io::stdout().flush()?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut input = String::new();
        let result = io::stdin()
            .lock()
            .take(512)
            .read_line(&mut input)
            .map(|_| input);
        let _ = tx.send(result);
    });
    let input = loop {
        cancel.check()?;
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(result) => break result?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(Error::State),
        }
    };
    session.install(input.trim_end_matches(['\r', '\n']), cancel, |e| {
        println!("{}: {}", e.stage.label(), e.message);
    })
}
