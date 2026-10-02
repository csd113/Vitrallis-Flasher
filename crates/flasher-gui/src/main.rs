//! Native desktop wizard. Blocking I/O stays on a cancellable worker.
use eframe::egui::{self, Color32, RichText};
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
    session::{Event, MockFel, Session, Stage},
    simulation,
};
use std::{
    collections::VecDeque,
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
    time::Duration,
};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 730.0])
            .with_min_inner_size([850.0, 650.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Vitrallis Flasher",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            cc.egui_ctx.style_mut_of(egui::Theme::Dark, |style| {
                style.spacing.button_padding = egui::vec2(12.0, 7.0);
                style.spacing.interact_size.y = 32.0;
                style
                    .text_styles
                    .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
                style
                    .text_styles
                    .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
            });
            Ok(Box::new(Wizard::default()))
        }),
    )
}
struct Prepared {
    session: Session<MockFel>,
    _directory: tempfile::TempDir,
}
enum Update {
    Event(Event),
    Ready(Box<Prepared>),
    Finished(Result<String, Error>),
}
struct Wizard {
    stage: Stage,
    simulation: bool,
    profile: Profile,
    tool: String,
    manifest_path: String,
    cache_path: String,
    offline_path: String,
    phrase: String,
    prepared: Option<Prepared>,
    worker: Option<Receiver<Update>>,
    cancel: Cancellation,
    log: VecDeque<String>,
    message: String,
    progress: Option<(u64, u64)>,
    assets_open: bool,
}
impl Default for Wizard {
    fn default() -> Self {
        Self {
            stage: Stage::Welcome,
            simulation: true,
            profile: Profile::default(),
            tool: String::new(),
            manifest_path: String::new(),
            cache_path: String::new(),
            offline_path: String::new(),
            phrase: String::new(),
            prepared: None,
            worker: None,
            cancel: Cancellation::default(),
            log: VecDeque::new(),
            message: "Recover with confidence. Check every image before any erase.".into(),
            progress: None,
            assets_open: false,
        }
    }
}
impl Drop for Wizard {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Wizard {
    fn record(&mut self, text: String) {
        if self.log.len() == 128 {
            self.log.pop_front();
        }
        self.log.push_back(text);
    }
    fn poll(&mut self) {
        loop {
            let update = self.worker.as_ref().map(Receiver::try_recv);
            match update {
                Some(Ok(Update::Event(e))) => {
                    self.stage = e.stage;
                    self.message.clone_from(&e.message);
                    if e.total > 0 {
                        self.progress = Some((e.done, e.total));
                    }
                    self.record(format!("{} · {}", e.stage.label(), e.message));
                }
                Some(Ok(Update::Ready(prepared))) => {
                    self.worker = None;
                    if self.cancel.check().is_err() {
                        self.stage = Stage::Recovery;
                        self.message = Error::Cancelled.to_string();
                    } else {
                        self.prepared = Some(*prepared);
                        self.stage = Stage::ConfirmErase;
                    }
                }
                Some(Ok(Update::Finished(result))) => {
                    self.worker = None;
                    match result {
                        Ok(message) => {
                            self.record(message.clone());
                            self.message = message;
                        }
                        Err(e) => {
                            self.stage = Stage::Recovery;
                            self.message = e.to_string();
                            self.record(self.message.clone());
                        }
                    }
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.worker = None;
                    self.stage = Stage::Recovery;
                    self.message = "Worker stopped unexpectedly. Start a fresh preflight.".into();
                }
                _ => break,
            }
        }
    }
    fn start(&mut self, action: impl FnOnce(SyncSender<Update>, Cancellation) + Send + 'static) {
        self.cancel = Cancellation::default();
        self.progress = None;
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::sync_channel(64);
        self.worker = Some(rx);
        std::thread::spawn(move || action(tx, cancel));
    }
    fn detect(&mut self) {
        self.stage = Stage::Detect;
        if self.simulation {
            let profile = self.profile;
            self.record(format!("Selected: {}", profile.label()));
            self.start(move |tx, cancel| {
                let result = (|| {
                    let directory = flasher_core::assets::temporary_directory()?;
                    let cache = Cache::open(directory.path())?;
                    let verified = simulation::prepare_profile(&cache, &cancel, profile)?;
                    let mut session = Session::new(MockFel {
                        delay: Duration::from_millis(300),
                        ..Default::default()
                    });
                    session.preflight(verified, &cancel, |e| {
                        let _ = tx.try_send(Update::Event(e));
                    })?;
                    Ok(Prepared {
                        session,
                        _directory: directory,
                    })
                })();
                let update = match result {
                    Ok(p) => Update::Ready(Box::new(p)),
                    Err(e) => Update::Finished(Err(e)),
                };
                let _ = tx.send(update);
            });
        } else {
            let tool = self.tool.clone();
            self.start(move |tx, cancel| {
                let result = (|| {
                    let devices = if tool.is_empty() {
                        NativeFel.discover(&cancel)?
                    } else {
                        RealFel::new(SunxiTool::open(Path::new(&tool))?).discover(&cancel)?
                    };
                    let device = select(&devices)?;
                    let _ = tx.try_send(Update::Event(Event {
                        stage: Stage::Preflight,
                        message: format!(
                            "Found {} candidate, SID {}. Board and NAND remain unverified.",
                            device.soc, device.sid
                        ),
                        done: 0,
                        total: 0,
                    }));
                    Err(Error::PhysicalBlocked)
                })();
                let _ = tx.send(Update::Finished(result));
            });
        }
    }
    fn install(&mut self) {
        let Some(mut prepared) = self.prepared.take() else {
            return;
        };
        let phrase = self.phrase.clone();
        self.phrase.clear();
        self.start(move |tx, cancel| {
            let result = prepared
                .session
                .install(&phrase, &cancel, |e| {
                    let _ = tx.try_send(Update::Event(e));
                })
                .map(|()| {
                    "Simulation complete. No USB writes, NAND erasure or reboot occurred.".into()
                });
            let _ = tx.send(Update::Finished(result));
        });
    }
    fn fetch(&mut self) {
        let manifest_path = self.manifest_path.clone();
        let cache_path = self.cache_path.clone();
        let offline_path = self.offline_path.clone();
        self.stage = Stage::Download;
        self.start(move |tx,cancel| {
            let result = (|| {
                let manifest = Manifest::open(Path::new(&manifest_path))?;
                let cache = Cache::open(Path::new(&cache_path))?;
                let offline = if offline_path.is_empty() { None } else { Some(Path::new(&offline_path)) };
                let verified = acquire(&manifest,&cache,offline,&cancel,|role,done,total| { let _ = tx.try_send(Update::Event(Event { stage: Stage::Download, message: format!("Checking {role:?}: {done} / {total} bytes"), done,total })); })?;
                Ok(format!("{} assets verified for {}. Integrity checked; physical approval remains blocked.",verified.len(),verified.manifest().release()))
            })();
            let _ = tx.send(Update::Finished(result));
        });
    }
    fn content(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new(self.stage.label()).size(30.0));
        ui.add_space(14.0);
        ui.label(RichText::new(&self.message).size(17.0));
        ui.add_space(20.0);
        self.stage_content(ui);
        if let Some((done, total)) = self.progress {
            ui.label(format!("{done} / {total} bytes"));
        }
        ui.add_space(18.0);
        if self.worker.is_some() {
            if ui.button("Cancel operation").clicked() {
                self.cancel.cancel();
                self.message = "Cancellation requested; waiting for bounded I/O to finish.".into();
            }
            ui.small(
                "Network reads may take up to five seconds; connection setup up to ten seconds.",
            );
        } else if self.stage != Stage::Welcome && ui.button("Start fresh").clicked() {
            self.prepared = None;
            self.phrase.clear();
            self.stage = Stage::Welcome;
            self.message = "Choose simulation or read-only device diagnostics.".into();
        }
    }
    fn stage_content(&mut self, ui: &mut egui::Ui) {
        match self.stage {
            Stage::Welcome => self.welcome_panel(ui),
            Stage::Fel => {
                ui.label("1. Disconnect power and USB.\n2. Connect the FEL pin to GND using the documented board pinout.\n3. Connect a USB data cable. Keep other FEL devices disconnected.");
                ui.add_space(18.0);
                if self.simulation {
                    ui.label(
                        "Simulation uses a virtual Hynix PocketCHIP. No connection is needed.",
                    );
                } else {
                    ui.label(usb_guidance());
                    ui.add_space(10.0);
                    ui.label("Optional reviewed sunxi-fel executable (absolute path; blank uses native USB)");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tool)
                            .char_limit(4096)
                            .desired_width(f32::INFINITY),
                    );
                }
                if ui.button("Detect PocketCHIP").clicked() {
                    self.detect();
                }
            }
            Stage::ConfirmErase => self.confirmation_panel(ui),
            Stage::Complete => {
                ui.label(format!("Simulated profile: {}", self.profile.label()));
                ui.small("No OS, shell or startup setting was installed or changed.");
                ui.label("The simulated installer completed write, readback verification and clean unmount.\n\nFor a future validated physical run: remove the FEL jumper only after successful verification, then power-cycle to boot NAND.");
            }
            Stage::Recovery => {
                ui.colored_label(
                    Color32::from_rgb(250, 182, 95),
                    "No automatic retry or reboot",
                );
                ui.label("Keep the device connected while diagnosing an interrupted physical installation. Save the detailed log. Re-enter FEL and run a fresh preflight before retrying; the old confirmation is invalid.");
                ui.add_space(12.0);
                ui.label(usb_guidance());
            }
            _ => {
                if self.worker.is_some() {
                    ui.spinner();
                }
            }
        }
    }
    fn welcome_panel(&mut self, ui: &mut egui::Ui) {
        ui.label("Choose your Debian 13 desktop");
        for profile in Profile::ALL {
            ui.radio_value(&mut self.profile, profile, profile.label());
        }
        ui.small(self.profile.description());
        ui.small("Stock means the PocketHome experience. A recovery reimage erases existing data; preservation is not yet validated.");
        ui.hyperlink_to(
            "Vitrallis Shell · installer and release status",
            "https://github.com/csd113/Vitrallis-Shell#install-on-pocketchip",
        );
        ui.add_space(12.0);
        ui.radio_value(
            &mut self.simulation,
            true,
            "Explore a complete recovery simulation",
        );
        ui.radio_value(
            &mut self.simulation,
            false,
            "Inspect a real FEL device (read-only)",
        );
        ui.add_space(12.0);
        ui.label("Both physical upgrades are blocked pending validated images and secure recovery. Vitrallis also needs a complete released bundle.");
        if ui.button("Get started").clicked() {
            self.stage = Stage::Fel;
            self.message = "Connect your PocketCHIP in FEL recovery mode.".into();
        }
    }
    fn confirmation_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(self.profile.label());
        ui.small(self.profile.description());
        ui.colored_label(
            Color32::from_rgb(250, 182, 95),
            "ERASE is irreversible on hardware. This run is simulated.",
        );
        if let Some(prepared) = &self.prepared {
            ui.label(format!(
                "Identified NAND: {}. All eight asset roles verified.",
                prepared
                    .session
                    .nand()
                    .map_or("Unknown", flasher_core::device::Nand::label)
            ));
            if let Some(expected) = prepared.session.confirmation() {
                ui.add_space(10.0);
                ui.label("Type the exact device-and-image confirmation below. It expires after five minutes.");
                ui.add(egui::Label::new(RichText::new(&expected).monospace()).wrap());
                ui.add(
                    egui::TextEdit::multiline(&mut self.phrase)
                        .char_limit(256)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                if ui
                    .add_enabled(
                        self.phrase == expected,
                        egui::Button::new("Confirm simulated erase and install"),
                    )
                    .clicked()
                {
                    self.install();
                }
            }
        }
    }
}
impl eframe::App for Wizard {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.poll();
        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        egui::Panel::top("brand").show(root, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.heading("VITRALLIS");
                ui.label("/ PocketCHIP Recovery");
                ui.separator();
                ui.colored_label(
                    Color32::from_rgb(109, 215, 190),
                    if self.simulation {
                        "SIMULATION"
                    } else {
                        "READ-ONLY DIAGNOSTICS"
                    },
                );
            });
            ui.add_space(10.0);
        });
        egui::Panel::left("steps")
            .exact_size(190.0)
            .show(root, |ui| {
                ui.add_space(18.0);
                for stage in Stage::ALL {
                    let active = stage == self.stage;
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(stage.label())
                            .color(if active {
                                Color32::from_rgb(109, 215, 190)
                            } else {
                                Color32::GRAY
                            })
                            .size(if active { 17.0 } else { 14.0 }),
                    );
                }
                ui.add_space(24.0);
                if ui
                    .add_enabled(
                        self.worker.is_none() && self.prepared.is_none(),
                        egui::Button::new("Verify local / online assets"),
                    )
                    .clicked()
                {
                    self.assets_open = true;
                }
            });
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(155.0)
            .min_size(130.0)
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Detailed log");
                    if ui.button("Copy log").clicked() {
                        ui.ctx()
                            .copy_text(self.log.iter().cloned().collect::<Vec<_>>().join("\n"));
                    }
                });
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.monospace(line);
                        }
                    });
            });
        egui::CentralPanel::default().show(root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(15.0);
                self.content(ui);
            });
        });
        if self.assets_open {
            let mut open = true;
            let mut fetch = false;
            egui::Window::new("Verify an asset set").open(&mut open).resizable(false).default_width(540.0).show(root,|ui| {
                ui.label("Select a reviewed manifest yourself. A matching hash checks integrity; it does not approve an image for NAND installation.");
                for (label,value) in [("Manifest JSON path",&mut self.manifest_path),("Existing private cache directory",&mut self.cache_path),("Offline hash-named asset directory (blank = HTTPS)",&mut self.offline_path)] {
                    ui.label(label); ui.add(egui::TextEdit::singleline(value).char_limit(4096).desired_width(f32::INFINITY));
                }
                fetch = ui.add_enabled(!self.manifest_path.is_empty() && !self.cache_path.is_empty(),egui::Button::new("Verify complete asset set")).clicked();
            });
            self.assets_open = open;
            if fetch {
                self.assets_open = false;
                self.fetch();
            }
        }
    }
}
