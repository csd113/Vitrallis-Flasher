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
        [command] if command == "detect" || command == "fel-probe" => {
            native_diagnostic(command, &cancel)?;
        }
        [command, assets, template, daemon, tool] if command == "recovery-boot" => {
            let private = flasher_core::recovery_boot::boot(
                Path::new(assets),
                Path::new(template),
                Path::new(daemon),
                Path::new(tool),
                &cancel,
            )?;
            println!(
                "RAM-only boot dispatched; authenticated inventory still required.\nPrivate session directory: {}",
                private.display()
            );
        }
        [command, config, binary]
            if matches!(
                command.as_str(),
                "recovery-inventory" | "recovery-ping" | "recovery-return-to-fel"
            ) =>
        {
            let diagnostic = match command.as_str() {
                "recovery-ping" => flasher_core::recovery::diagnostic_ping,
                "recovery-return-to-fel" => flasher_core::recovery::diagnostic_return_to_fel,
                _ => flasher_core::recovery::diagnostic_inventory,
            };
            let response = diagnostic(Path::new(config), Path::new(binary), &cancel)?;
            let json = serde_json::to_string_pretty(&response).map_err(|_| Error::Recovery)?;
            println!("{json}");
        }
        [command, tool] if command == "detect" => {
            let backend = RealFel::new(
                SunxiTool::open(Path::new(tool))
                    .inspect_err(|_| eprintln!("{}", usb_guidance()))?,
            );
            let devices = backend
                .discover(&cancel)
                .inspect_err(|_| eprintln!("{}", usb_guidance()))?;
            let selected = select(&devices)?;
            println!(
                "FEL candidate: {} {:03}:{:03} SID {}\nPocketCHIP board and NAND remain unverified. Physical writes are blocked.",
                selected.soc, selected.bus, selected.address, selected.sid
            );
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

fn help() {
    println!(
        "Vitrallis Flasher\n\n  doctor\n  releases\n  detect (native USB, read-only)\n  fel-probe (scratch SRAM diagnostic)\n  recovery-boot /assets /template /daemon /sunxi-fel (pinned read-only diagnostic)\n  recovery-ping /private-session/session.bin /private-session/daemon.bin\n  recovery-inventory /private-session/session.bin /private-session/daemon.bin\n  recovery-return-to-fel /private-session/session.bin /private-session/daemon.bin (keep FEL bridge connected)\n  detect /absolute/path/to/sunxi-fel\n  validate manifest.json\n  fetch manifest.json existing-private-cache\n  offline manifest.json existing-private-cache offline-directory\n  simulate [--profile stock|vitrallis-default]\n  upgrade --profile stock|vitrallis-default (blocked)\n\nStock PocketHome is the default choice. No physical write command is available. Simulation still requires typed ERASE confirmation. Downloads require an explicitly selected manifest; checksums establish integrity, not publisher trust."
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
