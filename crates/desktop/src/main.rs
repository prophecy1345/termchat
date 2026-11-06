//! Desktop client (GUI): a shell over `termchat-engine` (ADR-018).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use termchat_engine::engine::{DEFAULT_ONION, DEFAULT_RELAY};
use termchat_engine::{AUTOLOCK, Engine, Happened, Setup, Unlocked, data};
use zeroize::Zeroizing;

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("termchat-gui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    termchat_engine::no_core_dumps();
    let path = data::home().join("state");
    // A folder without locks (some network drives) must not stop termchat.
    let lock = data::lock(&path);
    let screen = match lock {
        Ok(None) => Screen::Busy,
        _ if path.exists() => Screen::Unlock(Unlock::default()),
        _ => Screen::Setup(SetupForm::default()),
    };
    // The engine's relay tasks run on this runtime.
    let Ok(rt) = tokio::runtime::Runtime::new() else {
        eprintln!("termchat-gui: cannot start the async runtime");
        std::process::exit(1);
    };
    let _rt = rt.enter();
    let options = eframe::NativeOptions {
        // Never a contact name: other programs see the title (ADR-018).
        viewport: egui::ViewportBuilder::default()
            .with_title("termchat")
            .with_app_id("termchat")
            .with_inner_size([900.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "termchat",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(App {
                path,
                screen,
                last_input: Instant::now(),
                last_tick: Instant::now(),
                status: String::new(),
                _lock: lock,
            }))
        }),
    )
}

enum Screen {
    /// Another client has the state open.
    Busy,
    /// First run.
    Setup(SetupForm),
    Unlock(Unlock),
    Main(Box<Engine>),
}

#[derive(Default)]
struct Unlock {
    password: Zeroizing<String>,
    error: String,
}

struct SetupForm {
    nickname: String,
    password: Zeroizing<String>,
    again: Zeroizing<String>,
    tor: bool,
    relay: String,
    onion: String,
    error: String,
}

impl Default for SetupForm {
    fn default() -> Self {
        Self {
            nickname: String::new(),
            password: Zeroizing::default(),
            again: Zeroizing::default(),
            tor: cfg!(feature = "tor"),
            relay: DEFAULT_RELAY.into(),
            onion: DEFAULT_ONION.into(),
            error: String::new(),
        }
    }
}

struct App {
    path: PathBuf,
    screen: Screen,
    last_input: Instant,
    last_tick: Instant,
    /// One line: the last note from the engine.
    status: String,
    /// Held while the window is open (ADR-018).
    _lock: std::io::Result<Option<std::fs::File>>,
}

impl App {
    /// Saves and forgets the open state; the password opens it again.
    fn lock(&mut self) {
        if let Screen::Main(engine) = &mut self.screen {
            engine.persist();
        }
        self.status.clear();
        self.screen = Screen::Unlock(Unlock::default());
    }

    /// Everything is erased: a first run again, with `note` (empty after the
    /// duress password, so the screen looks fresh, ADR-003).
    fn wiped(&mut self, note: String) {
        self.status.clear();
        self.screen = Screen::Setup(SetupForm {
            error: note,
            ..SetupForm::default()
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| !i.events.is_empty()) {
            self.last_input = Instant::now();
        }
        // Relay events arrive on a channel; a few checks a second are enough.
        // Polling for now; a waker from the relay task if latency matters.
        ctx.request_repaint_after(Duration::from_millis(250));
        let Screen::Main(engine) = &mut self.screen else {
            return;
        };
        if self.last_input.elapsed() >= AUTOLOCK {
            return self.lock();
        }
        while let Ok(ev) = engine.events.try_recv() {
            engine.on_relay(ev);
        }
        if self.last_tick.elapsed() >= Duration::from_secs(1) {
            self.last_tick = Instant::now();
            engine.tick();
        }
        let mut wiped = None;
        for h in std::mem::take(&mut engine.happened) {
            match h {
                Happened::Note(n) | Happened::Notice(n) => self.status = n,
                Happened::Wiped(n) => wiped = Some(n),
                Happened::Alert { .. } | Happened::WipeAnyway(_) => {}
            }
        }
        if let Some(note) = wiped {
            self.wiped(note);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| match &mut self.screen {
            Screen::Busy => {
                ui.heading("termchat is already open in another window");
                ui.label("Close it first: two clients on one account would overwrite each other.");
            }
            Screen::Setup(form) => {
                if let Some(engine) = setup(ui, form, &self.path) {
                    self.screen = Screen::Main(engine);
                }
            }
            Screen::Unlock(unlock) => match unlock_screen(ui, unlock, &self.path) {
                Some(Unlocked::Open(engine)) => self.screen = Screen::Main(engine),
                Some(Unlocked::Wiped) => self.wiped(String::new()),
                None => {}
            },
            Screen::Main(engine) => {
                ui.heading(engine.data.nickname.as_str());
                ui.label(match engine.connected {
                    true => "connected",
                    false => "connecting…",
                });
                ui.label(format!("{} contacts", engine.data.contacts.len()));
                ui.label(self.status.as_str());
                if ui.button("Lock").clicked() {
                    self.lock();
                }
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Screen::Main(engine) = &mut self.screen {
            engine.persist();
        }
    }
}

/// Returns the engine once the password opens the state.
fn unlock_screen(ui: &mut egui::Ui, u: &mut Unlock, path: &std::path::Path) -> Option<Unlocked> {
    ui.heading("termchat");
    ui.label("Password");
    let field = ui.add(egui::TextEdit::singleline(&mut *u.password).password(true));
    field.request_focus();
    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let go = ui.button("Unlock").clicked() || enter;
    ui.colored_label(ui.visuals().error_fg_color, u.error.as_str());
    if !go {
        return None;
    }
    // Argon2 blocks the window for about a second; a worker
    // thread if that bothers anyone.
    let result = Engine::unlock(path, u.password.as_bytes());
    u.password = Zeroizing::default();
    match result {
        Ok(r) => Some(r),
        Err(e) => {
            u.error = e;
            None
        }
    }
}

/// The first-run form; returns the new identity's engine.
fn setup(ui: &mut egui::Ui, f: &mut SetupForm, path: &std::path::Path) -> Option<Box<Engine>> {
    ui.heading("termchat.app setup");
    egui::Grid::new("setup").num_columns(2).show(ui, |ui| {
        ui.label("Nickname (only your contacts see it)");
        ui.text_edit_singleline(&mut f.nickname);
        ui.end_row();
        ui.label("Password");
        ui.add(egui::TextEdit::singleline(&mut *f.password).password(true));
        ui.end_row();
        ui.label("Password again");
        ui.add(egui::TextEdit::singleline(&mut *f.again).password(true));
        ui.end_row();
    });
    ui.checkbox(&mut f.tor, "Connect through Tor");
    egui::CollapsingHeader::new("Relay").show(ui, |ui| {
        egui::Grid::new("relay").num_columns(2).show(ui, |ui| {
            ui.label("Relay URL");
            ui.text_edit_singleline(&mut f.relay);
            ui.end_row();
            ui.label("Onion URL (empty: none)");
            ui.text_edit_singleline(&mut f.onion);
            ui.end_row();
        });
    });
    let go = ui.button("Create").clicked();
    ui.colored_label(ui.visuals().error_fg_color, f.error.as_str());
    if !go {
        return None;
    }
    if *f.password != *f.again {
        f.error = "the passwords differ".into();
        return None;
    }
    let onion = f.onion.trim();
    let s = Setup {
        relay_url: f.relay.trim(),
        onion_url: (!onion.is_empty()).then_some(onion),
        tor: f.tor,
        nickname: f.nickname.trim(),
        password: &f.password,
    };
    match Engine::setup(path, &s) {
        Ok(engine) => Some(engine),
        Err(e) => {
            f.error = e;
            None
        }
    }
}
