//! Desktop client (GUI): a shell over `termchat-engine` (ADR-018).
// No console window behind it on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod chat;
mod dialog;
mod logo;
mod synth;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chat::{Chat, Pal};
use eframe::egui;
use termchat_engine::engine::{DEFAULT_ONION, DEFAULT_RELAY};
use termchat_engine::settings::{self, Settings};
use termchat_engine::{Engine, Setup, Unlocked, data};
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
            .with_title("termchat.app")
            .with_app_id("termchat")
            .with_icon(logo::icon())
            .with_inner_size([900.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "termchat",
        options,
        Box::new(|cc| {
            // Plain, so the lock screen has the theme too (ADR-008).
            let settings = Settings::load(&settings::path(&path));
            Pal::fonts(&cc.egui_ctx);
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
        if self.last_input.elapsed() >= self.settings.lock_after() {
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
            synth::scanlines(ui.ctx());
            return;
        }
        let open = |engine| Screen::Main(Box::new(Chat::new(engine)));
        let (pal, path) = (&self.pal, &self.path);
        let mut wiped = false;
        egui::CentralPanel::default().show(ui, |ui| {
            synth::backdrop(ui, pal, ui.clip_rect(), true);
            match &mut self.screen {
                Screen::Busy => {
                    ui.add_space((ui.available_height() / 2.0 - 40.0).max(0.0));
                    ui.vertical_centered(|ui| {
                        ui.heading("termchat.app is already open in another window");
                        ui.label("Close it first: two clients on one account would overwrite each other.");
                    });
                }
                Screen::Setup(form) => match first_run(ui, pal, form, path) {
                    Some(Ok(engine)) => self.screen = open(engine),
                    Some(Err(())) => {
                        self.screen = Screen::Unlock(Unlock {
                            error: "restored: enter the backup's password".into(),
                            ..Unlock::default()
                        })
                    }
                    None => {}
                },
                Screen::Unlock(unlock) => match unlock_screen(ui, pal, unlock, path) {
                    Some(Unlocked::Open(engine)) => self.screen = open(engine),
                    Some(Unlocked::Wiped) => wiped = true,
                    None => {}
                },
                Screen::Main(_) => {}
            }
        });
        if wiped {
            self.wiped(String::new());
        }
        synth::scanlines(ui.ctx());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Screen::Main(chat) = &mut self.screen {
            chat.engine.persist();
        }
    }
}

/// Returns the engine once the password opens the state. Centred, one big
/// field; Return unlocks, as it is the only field here.
fn unlock_screen(
    ui: &mut egui::Ui,
    pal: &Pal,
    u: &mut Unlock,
    path: &std::path::Path,
) -> Option<Unlocked> {
    const WIDTH: f32 = 360.0;
    ui.add_space((ui.available_height() / 2.0 - 120.0).max(0.0));
    let mut go = false;
    ui.vertical_centered(|ui| {
        logo::mark(ui, 96.0);
        ui.label(egui::RichText::new("termchat.app").strong().size(28.0));
        ui.add_space(20.0);
        let field = ui.add(
            egui::TextEdit::singleline(&mut *u.password)
                .password(true)
                // The hint takes no font from the field: same size, so it
                // sits in the middle like the typed text.
                .hint_text(egui::RichText::new("Password").size(20.0))
                .font(egui::FontId::proportional(20.0))
                .desired_width(WIDTH)
                .margin(egui::Margin::symmetric(14, 12)),
        );
        field.request_focus();
        ui.add_space(12.0);
        go = synth::button(ui, pal, "Unlock", egui::vec2(WIDTH, 40.0)).clicked();
        ui.add_space(10.0);
        ui.colored_label(ui.visuals().error_fg_color, u.error.as_str());
    });
    go |= ui.input(|i| i.key_pressed(egui::Key::Enter));
    if !go || u.password.is_empty() {
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

/// The first-run screen: a centred card with the setup form and, below it,
/// restoring a backup. `Ok`: a new identity; `Err`: a backup is in place.
fn first_run(
    ui: &mut egui::Ui,
    pal: &Pal,
    f: &mut SetupForm,
    path: &std::path::Path,
) -> Option<Result<Box<Engine>, ()>> {
    const WIDTH: f32 = 440.0;
    let mut done = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space((ui.available_height() / 2.0 - 330.0).max(24.0));
        ui.vertical_centered(|ui| {
            logo::mark(ui, 72.0);
            ui.label(egui::RichText::new("termchat.app").strong().size(28.0));
            ui.label(
                egui::RichText::new("No phone, no email, no name: a key pair and a nickname.")
                    .color(pal.dim),
            );
            ui.add_space(16.0);
            // The theme's base, so the fields (its surface colour) stand out.
            egui::Frame::new()
                .fill(pal.bg)
                .stroke(egui::Stroke::new(1.0, pal.mauve.gamma_multiply(0.35)))
                .corner_radius(6)
                .inner_margin(20)
                .show(ui, |ui| {
                    ui.set_width(WIDTH);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        if let Some(engine) = setup(ui, pal, f, path, WIDTH) {
                            done = Some(Ok(engine));
                        } else if restore(ui, f, path) {
                            done = Some(Err(()));
                        }
                    });
                });
            ui.add_space(24.0);
        });
    });
    done
}

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

/// The first-run form; returns the new identity's engine.
fn setup(
    ui: &mut egui::Ui,
    pal: &Pal,
    f: &mut SetupForm,
    path: &std::path::Path,
    width: f32,
) -> Option<Box<Engine>> {
    let field = |ui: &mut egui::Ui, label: &str, edit: egui::TextEdit| {
        ui.label(egui::RichText::new(label).small().color(pal.dim));
        ui.add(
            edit.desired_width(width)
                .margin(egui::Margin::symmetric(10, 8)),
        );
        ui.add_space(6.0);
    };
    field(
        ui,
        "NICKNAME · only your contacts see it",
        egui::TextEdit::singleline(&mut f.nickname),
    );
    field(
        ui,
        "PASSWORD",
        egui::TextEdit::singleline(&mut *f.password).password(true),
    );
    field(
        ui,
        "PASSWORD AGAIN",
        egui::TextEdit::singleline(&mut *f.again).password(true),
    );
    ui.checkbox(
        &mut f.tor,
        "Connect through Tor (hides your IP from the relay)",
    );
    egui::CollapsingHeader::new("Relay").show(ui, |ui| {
        field(ui, "RELAY URL", egui::TextEdit::singleline(&mut f.relay));
        field(
            ui,
            "ONION URL · empty: none",
            egui::TextEdit::singleline(&mut f.onion),
        );
    });
    ui.add_space(8.0);
    let go = synth::button(ui, pal, "Create my identity", egui::vec2(width, 40.0)).clicked();
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // tests only
    use super::*;

    /// One frame of the unlock screen with Return pressed.
    fn press_return(u: &mut Unlock, path: &std::path::Path) -> Option<Unlocked> {
        let ctx = egui::Context::default();
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let input = egui::RawInput {
            events: vec![enter],
            ..Default::default()
        };
        let mut out = None;
        let pal = Pal::named("termchat");
        let mut full = ctx.run_ui(input, |ui| out = unlock_screen(ui, &pal, u, path));
        full.textures_delta.clear();
        out
    }

    #[test]
    fn return_unlocks() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _rt = rt.enter();
        let dir = std::env::temp_dir().join(format!("termchat-unlock-{}", std::process::id()));
        let path = dir.join("state");
        let password = "correct horse battery staple violin";
        let setup = Setup {
            relay_url: "ws://127.0.0.1:9/",
            onion_url: None,
            tor: false,
            nickname: "me",
            password,
        };
        drop(Engine::setup(&path, &setup).unwrap());

        let mut u = Unlock::default();
        assert!(press_return(&mut u, &path).is_none(), "nothing typed yet");
        *u.password = "wrong".into();
        assert!(press_return(&mut u, &path).is_none());
        assert_eq!(u.error, "wrong password");
        assert!(u.password.is_empty(), "a tried password is forgotten");
        *u.password = password.into();
        assert!(matches!(
            press_return(&mut u, &path),
            Some(Unlocked::Open(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
