//! Minimal CLI; USB discovery is opt-in and installation is simulated only.
use flasher_core::{
    Cancellation, Error,
    assets::{Cache, acquire},
    device::{RealFel, select},
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
fn help() {
    println!(
        "Vitrallis Flasher\n\n  doctor\n  releases\n  detect /absolute/path/to/sunxi-fel\n  validate manifest.json\n  fetch manifest.json existing-private-cache\n  offline manifest.json existing-private-cache offline-directory\n  simulate [--profile stock|vitrallis-default]\n  upgrade --profile stock|vitrallis-default (blocked)\n\nStock PocketHome is the default choice. No physical write command is available. Simulation still requires typed ERASE confirmation. Downloads require an explicitly selected manifest; checksums establish integrity, not publisher trust."
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
    let assets = acquire(&manifest, &cache, offline, cancel, |role, done, total| {
        if done == total {
            println!("Verified {role:?}: {total} bytes");
        }
    })?;
    println!(
        "{} verified assets; physical flashing remains blocked.",
        assets.len()
    );
    Ok(())
}
fn simulate(cancel: &Cancellation, profile: Profile) -> Result<(), Error> {
    let directory = flasher_core::assets::temporary_directory()?;
    simulate_in(directory.path(), cancel, profile)
}
fn simulate_in(directory: &Path, cancel: &Cancellation, profile: Profile) -> Result<(), Error> {
    let cache = Cache::open(directory)?;
    let (manifest, assets) = simulation::prepare_profile(&cache, cancel, profile)?;
    println!(
        "Requested: {}\n{}\nThis fixture does not install an OS or change the startup desktop.",
        manifest.profile().label(),
        manifest.profile().description()
    );
    let mut session = Session::new(MockFel::default());
    session.preflight(manifest, assets, cancel, |e| {
        println!("{}: {}", e.stage.label(), e.message);
    })?;
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
