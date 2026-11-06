//! Desktop client (GUI): a shell over `termchat-engine` (ADR-018).
// No console window behind it on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod chat;
mod dialog;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chat::{Chat, Pal};
use eframe::egui;
use termchat_engine::engine::{DEFAULT_ONION, DEFAULT_RELAY};
use termchat_engine::settings::{self, Settings};
use termchat_engine::{AUTOLOCK, Engine, Setup, Unlocked, data};
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
            // Plain, so the lock screen has the theme too (ADR-008).
            let settings = Settings::load(&settings::path(&path));
            let pal = Pal::named(&settings.desktop_theme);
            pal.apply(&cc.egui_ctx);
            Ok(Box::new(App {
                settings,
                path,
                screen,
                pal,
                last_input: Instant::now(),
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
    Main(Box<Chat>),
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
    /// The path of a backup to restore instead.
    backup: String,
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
            backup: String::new(),
            error: String::new(),
        }
    }
}

struct App {
    path: PathBuf,
    screen: Screen,
    settings: Settings,
    pal: Pal,
    last_input: Instant,
    /// Held while the window is open (ADR-018).
    _lock: std::io::Result<Option<std::fs::File>>,
}

impl App {
    /// Saves and forgets the open state; the password opens it again.
    fn lock(&mut self) {
        if let Screen::Main(chat) = &mut self.screen {
            chat.engine.persist();
        }
        self.screen = Screen::Unlock(Unlock::default());
    }

    /// Everything is erased: a first run again, with `note` (empty after the
    /// duress password, so the screen looks fresh, ADR-003).
    fn wiped(&mut self, note: String) {
        self.settings = Settings::default();
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
        let Screen::Main(chat) = &mut self.screen else {
            return;
        };
        if self.last_input.elapsed() >= AUTOLOCK {
            return self.lock();
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if let Some(note) = chat.step(focused, self.settings.sound) {
            self.wiped(note);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Screen::Main(chat) = &mut self.screen {
            let before = self.settings.clone();
            let lock = chat.ui(ui, &self.pal, &mut self.settings);
            if self.settings != before {
                if self.settings.desktop_theme != before.desktop_theme {
                    self.pal = Pal::named(&self.settings.desktop_theme);
                    self.pal.apply(ui.ctx());
                }
                if let Err(e) = self.settings.save(&settings::path(&self.path)) {
                    chat.status = format!("settings not saved: {e}");
                }
            }
            if lock {
                self.lock();
            }
            return;
        }
        let open = |engine| Screen::Main(Box::new(Chat::new(engine)));
        egui::CentralPanel::default().show(ui, |ui| match &mut self.screen {
            Screen::Busy => {
                ui.heading("termchat is already open in another window");
                ui.label("Close it first: two clients on one account would overwrite each other.");
            }
            Screen::Setup(form) => {
                if let Some(engine) = setup(ui, form, &self.path) {
                    self.screen = open(engine);
                } else if restore(ui, form, &self.path) {
                    self.screen = Screen::Unlock(Unlock {
                        error: "restored: enter the backup's password".into(),
                        ..Unlock::default()
                    });
                }
            }
            Screen::Unlock(unlock) => match unlock_screen(ui, unlock, &self.path) {
                Some(Unlocked::Open(engine)) => self.screen = open(engine),
                Some(Unlocked::Wiped) => self.wiped(String::new()),
                None => {}
            },
            Screen::Main(_) => {}
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Screen::Main(chat) = &mut self.screen {
            chat.engine.persist();
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
/// A backup instead of a new identity. True once the file is in place.
fn restore(ui: &mut egui::Ui, f: &mut SetupForm, path: &std::path::Path) -> bool {
    ui.add_space(16.0);
    let mut done = false;
    egui::CollapsingHeader::new("Restore from a backup").show(ui, |ui| {
        ui.label(
            "Afterwards unlock with the backup's password. Chats start fresh; contacts \
             see that you restarted the conversation.",
        );
        ui.horizontal(|ui| {
            ui.label("Backup file");
            ui.add(
                egui::TextEdit::singleline(&mut f.backup).hint_text("/Volumes/USB/termchat-backup"),
            );
            if ui.button("Restore").clicked() {
                match termchat_engine::engine::restore(path, &data::expand(&f.backup)) {
                    Ok(()) => done = true,
                    Err(e) => f.error = e,
                }
            }
        });
    });
    done
}

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
