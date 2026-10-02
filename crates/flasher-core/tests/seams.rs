//! Host-only Batch 2 seam coverage through the public API.
//!
//! These tests drive a scripted transport with deterministic fixtures. They do
//! not access USB, FEL, NAND or any hardware, and they never authorize writes.
use flasher_core::{
    Cancellation, Error,
    assets::Cache,
    device::{Device, IdentifiedTarget, Nand, TargetInfo, select},
    fel::{FelCall, FelTransport, ScriptedFel},
    manifest::Role,
    nand, simulation,
};

fn device() -> Device {
    Device {
        bus: 1,
        address: 2,
        soc: "A13".into(),
        sid: "01234567:89abcdef:01234567:89abcdef".into(),
    }
}

fn verified() -> Result<(flasher_core::assets::VerifiedAssets, tempfile::TempDir), Error> {
    let dir = flasher_core::assets::temporary_directory()?;
    let cache = Cache::open(dir.path())?;
    let verified = simulation::prepare(&cache, &Cancellation::default())?;
    Ok((verified, dir))
}

#[test]
fn identification_success_never_triggers_upload_or_execute() -> Result<(), Error> {
    let transport = ScriptedFel::new();
    let device = device();
    transport.expect_discover(vec![device.clone()]);
    transport.expect_identify(&device, TargetInfo::fixture(Nand::Hynix));
    let devices = transport.discover(&Cancellation::default())?;
    let selected = select(&devices)?;
    let info = transport.identify(&selected, &Cancellation::default())?;
    let target = IdentifiedTarget::identify(info)?;
    let (assets, _dir) = verified()?;
    let plan = nand::plan(&assets, &target)?;
    assert!(!plan.is_executable());
    assert!(matches!(
        plan.authorize_execution(),
        Err(Error::PhysicalBlocked)
    ));
    assert!(
        transport
            .calls()
            .iter()
            .all(|call| matches!(call, FelCall::Discover | FelCall::Identify { .. })),
        "identification alone must never upload, execute or read memory"
    );
    assert_eq!(transport.remaining(), 0);
    Ok(())
}

#[test]
fn recognized_nand_fixture_selects_only_its_spl_variant() -> Result<(), Error> {
    for (nand, expected) in [
        (Nand::Hynix, Role::SplHynix),
        (Nand::Toshiba, Role::SplToshiba),
    ] {
        let transport = ScriptedFel::new();
        let device = device();
        transport.expect_identify(&device, TargetInfo::fixture(nand));
        let info = transport.identify(&device, &Cancellation::default())?;
        let target = IdentifiedTarget::identify(info)?;
        assert_eq!(target.spl_role(), expected);
        let (assets, _dir) = verified()?;
        let plan = nand::plan(&assets, &target)?;
        let sources: Vec<_> = plan.steps().iter().filter_map(|step| step.source).collect();
        assert!(sources.contains(&expected));
        let other = match nand {
            Nand::Hynix => Role::SplToshiba,
            Nand::Toshiba => Role::SplHynix,
        };
        assert!(!sources.contains(&other));
        assert!(
            plan.gates()
                .iter()
                .any(|gate| { matches!(gate.id, flasher_core::nand::GateId::NandGeometry) })
        );
    }
    Ok(())
}

#[test]
fn mismatched_ambiguous_or_unknown_fixtures_reject() -> Result<(), Error> {
    for (soc, board, part) in [
        ("H3", "pocketchip", "H27UCG8T2ETR"),
        ("A13", "unknown", "H27UCG8T2ETR"),
        ("A13", "", "H27UCG8T2ETR"),
        ("", "pocketchip", "H27UCG8T2ETR"),
        ("A13", "pocketchip", "W25Q128"),
        ("A13", "pocketchip", ""),
    ] {
        let transport = ScriptedFel::new();
        let device = device();
        transport.expect_identify(
            &device,
            TargetInfo {
                soc: soc.into(),
                board: board.into(),
                nand_part: part.into(),
                ram_bytes: 0,
            },
        );
        let info = transport.identify(&device, &Cancellation::default())?;
        assert!(
            matches!(IdentifiedTarget::identify(info), Err(Error::Device)),
            "{soc}/{board}/{part} must reject"
        );
    }
    Ok(())
}

#[test]
fn every_scripted_transport_call_can_fail_and_stops_the_sequence() {
    type Invoke = Box<dyn Fn(&ScriptedFel) -> Result<(), Error>>;
    let device = device();
    let cases: Vec<(FelCall, Invoke)> = vec![
        (
            FelCall::Discover,
            Box::new(|transport| transport.discover(&Cancellation::default()).map(|_| ())),
        ),
        (
            FelCall::identify(&device),
            Box::new({
                let device = device.clone();
                move |transport| {
                    transport
                        .identify(&device, &Cancellation::default())
                        .map(|_| ())
                }
            }),
        ),
        (
            FelCall::device_info(&device),
            Box::new({
                let device = device.clone();
                move |transport| {
                    transport
                        .device_info(&device, &Cancellation::default())
                        .map(|_| ())
                }
            }),
        ),
        (
            FelCall::upload(&device, 0x4000_0000, b"payload"),
            Box::new({
                let device = device.clone();
                move |transport| {
                    transport.upload_to_ram(
                        &device,
                        0x4000_0000,
                        b"payload",
                        &Cancellation::default(),
                    )
                }
            }),
        ),
        (
            FelCall::execute(&device, 0x4000_0000),
            Box::new({
                let device = device.clone();
                move |transport| transport.execute(&device, 0x4000_0000, &Cancellation::default())
            }),
        ),
        (
            FelCall::read_memory(&device, 0x1000, 4),
            Box::new({
                let device = device.clone();
                move |transport| {
                    transport
                        .read_memory(&device, 0x1000, 4, &Cancellation::default())
                        .map(|_| ())
                }
            }),
        ),
        (
            FelCall::read_status(&device),
            Box::new({
                let device = device.clone();
                move |transport| {
                    transport
                        .read_status(&device, &Cancellation::default())
                        .map(|_| ())
                }
            }),
        ),
    ];
    for (call, invoke) in cases {
        let label = call.label();
        let transport = ScriptedFel::new();
        transport.inject_error(call, Error::Timeout);
        assert!(
            matches!(invoke(&transport), Err(Error::Timeout)),
            "{label} must report the injected failure"
        );
        assert_eq!(transport.calls().len(), 1, "{label}");
        // A later operation must never be served after a transport failure.
        assert!(
            matches!(invoke(&transport), Err(Error::ScriptUnexpected(_))),
            "{label} continued after failure"
        );
        assert_eq!(transport.calls().len(), 2, "{label}");
    }
}

#[test]
fn unavailable_fel_transport_cannot_discover_or_upload() {
    use flasher_core::fel::UnavailableFel;
    let transport = UnavailableFel;
    let cancel = Cancellation::default();
    assert!(matches!(
        transport.discover(&cancel),
        Err(Error::FelUnavailable)
    ));
    assert!(matches!(
        transport.upload_to_ram(&device(), 0x4000_0000, b"payload", &cancel),
        Err(Error::FelUnavailable)
    ));
    assert!(matches!(
        transport.execute(&device(), 0x4000_0000, &cancel),
        Err(Error::FelUnavailable)
    ));
}
