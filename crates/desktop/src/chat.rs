//! The unlocked window: contact list, chat, status bar (ADR-018). The look
//! follows the Android app (ADR-011, section 5).

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Align2, Color32, Layout, RichText, Stroke};
use termchat_engine::data::{Contact, Status};
use termchat_engine::engine::{age, clean_text, next_timer, span};
use termchat_engine::notify::{self, SOUND_GAP};
use termchat_engine::settings::Settings;
use termchat_engine::{Engine, Happened, themes};

use crate::dialog::Dialog;

type Key = [u8; 32];

const TOAST_FOR: Duration = Duration::from_secs(5);
const MAX_TOASTS: usize = 4;

/// The colours the window uses, from a shared theme (`engine::themes`).
pub struct Pal {
    pub bg: Color32,
    pub surface: Color32,
    pub line: Color32,
    pub mine: Color32,
    pub theirs: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub warn: Color32,
    pub danger: Color32,
    /// Synthwave neon: the theme's mauve, and a pink between it and red.
    pub mauve: Color32,
    pub pink: Color32,
}

impl Pal {
    /// Unknown names give the first theme.
    pub fn named(name: &str) -> Self {
        let c = themes::THEMES
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or(&themes::THEMES[0])
            .1;
        let rgb = |i: usize| {
            let [_, r, g, b] = c[i].to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // Indices are `themes::SLOTS`.
        Self {
            accent: rgb(0),
            bg: rgb(1),
            surface: rgb(2),
            mine: rgb(3),
            theirs: rgb(4),
            line: rgb(5),
            dim: rgb(8),
            text: rgb(9),
            danger: rgb(14),
            mauve: rgb(11),
            pink: crate::synth::mix(rgb(11), rgb(14), 0.5),
            warn: rgb(16),
        }
    }

    /// Hack, the monospace font, also backs the proportional one: it has
    /// ● ○ ↓ and more that the proportional font lacks.
    pub fn fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        let mono = fonts
            .families
            .get(&egui::FontFamily::Monospace)
            .and_then(|f| f.first())
            .cloned();
        if let Some(hack) = mono {
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .push(hack);
        }
        ctx.set_fonts(fonts);
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        v.panel_fill = self.bg;
        v.window_fill = self.surface;
        v.extreme_bg_color = self.surface;
        v.override_text_color = Some(self.text);
        v.selection.bg_fill = self.mine;
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.line);
        // Windows float in a violet glow, like the site's terminal.
        v.window_shadow = egui::Shadow {
            offset: [0, 10],
            blur: 48,
            spread: 0,
            color: self.mauve.gamma_multiply(0.18),
        };
        v.window_stroke = Stroke::new(1.0, self.mauve.gamma_multiply(0.35));
        ctx.set_visuals(v);
    }
}

struct Toast {
    key: Key,
    text: String,
    until: Instant,
}

pub struct Chat {
    pub engine: Box<Engine>,
    pub(crate) open: Option<Key>,
    input: String,
    toasts: Vec<Toast>,
    last_sound: Option<Instant>,
    last_tick: Instant,
    /// One line: the last note from the engine.
    pub(crate) status: String,
    pub(crate) dialog: Option<Dialog>,
}

impl Chat {
    pub fn new(engine: Box<Engine>) -> Self {
        let mut chat = Self {
            engine,
            open: None,
            input: String::new(),
            toasts: Vec::new(),
            last_sound: None,
            last_tick: Instant::now(),
            status: String::new(),
            dialog: None,
        };
        chat.engine.resort();
        chat
    }

    /// Takes in relay events and timers. `Some(note)`: everything was erased.
    pub fn step(&mut self, focused: bool, sound: bool) -> Option<String> {
        self.engine.open = self.open;
        let mut moved = false;
        while let Ok(ev) = self.engine.events.try_recv() {
            self.engine.on_relay(ev);
            moved = true;
        }
        if self.last_tick.elapsed() >= Duration::from_secs(1) {
            self.last_tick = Instant::now();
            self.engine.tick();
        }
        if moved {
            self.engine.resort();
        }
        if let Some(i) = self.open.and_then(|k| self.engine.index(&k)) {
            self.engine.data.contacts[i].unread = 0;
        }
        self.toasts.retain(|t| t.until > Instant::now());
        self.absorb(focused, sound)
    }

    fn absorb(&mut self, focused: bool, sound: bool) -> Option<String> {
        for h in std::mem::take(&mut self.engine.happened) {
            match h {
                Happened::Alert { key, request } => self.alert(key, request, focused, sound),
                Happened::Note(n) => self.status = n,
                Happened::Notice(n) => self.dialog = Some(Dialog::Notice(n)),
                Happened::Wiped(n) => return Some(n),
                Happened::WipeAnyway(why) => self.dialog = Some(Dialog::WipeAnyway(why)),
            }
        }
        None
    }

    /// A card and a sound, with the name but never the text (ADR-008).
    fn alert(&mut self, key: Key, request: bool, focused: bool, sound: bool) {
        if focused && self.open == Some(key) {
            return;
        }
        let Some(c) = self
            .engine
            .index(&key)
            .map(|i| &self.engine.data.contacts[i])
        else {
            return;
        };
        let what = match request {
            true => "contact request",
            false => "new message",
        };
        if self.toasts.len() >= MAX_TOASTS {
            self.toasts.remove(0);
        }
        self.toasts.push(Toast {
            key,
            text: format!("{what} · {}", self.engine.name(c)),
            until: Instant::now() + TOAST_FOR,
        });
        let quiet = self.last_sound.is_some_and(|t| t.elapsed() < SOUND_GAP);
        if sound && !quiet {
            self.last_sound = Some(Instant::now());
            notify::sound();
        }
    }

    pub(crate) fn select(&mut self, key: Key) {
        self.open = Some(key);
        self.input.clear();
    }

    /// Draws the window. True: the user asked to lock.
    pub fn ui(&mut self, ui: &mut egui::Ui, p: &Pal, s: &mut Settings) -> bool {
        let mut lock = false;
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(p.surface).inner_margin(6))
            .show(ui, |ui| self.status_bar(ui, p));
        egui::Panel::left("contacts")
            .resizable(false)
            .exact_size(260.0)
            .frame(egui::Frame::new().fill(p.bg).inner_margin(10))
            .show(ui, |ui| lock = self.sidebar(ui, p));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(0))
            .show(ui, |ui| self.chat(ui, p));
        self.toasts(ui.ctx(), p);
        self.dialogs(ui.ctx(), p, s);
        lock
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, p: &Pal) -> bool {
        ui.horizontal(|ui| {
            crate::logo::mark(ui, 26.0);
            ui.label(RichText::new("termchat.app").strong().size(16.0));
        });
        ui.add_space(8.0);
        let (requests, contacts): (Vec<_>, Vec<_>) = (0..self.engine.data.contacts.len())
            .partition(|&i| self.engine.is_request(&self.engine.data.contacts[i].key));
        let mut picked = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .max_height(ui.available_height() - 36.0)
            .show(ui, |ui| {
                for (title, rows) in [("Requests", &requests), ("Contacts", &contacts)] {
                    if rows.is_empty() && title == "Requests" {
                        continue;
                    }
                    ui.label(RichText::new(format!("{title}  {}", rows.len())).color(p.dim));
                    for &i in rows {
                        if let Some(k) = self.row(ui, p, i) {
                            picked = Some(k);
                        }
                    }
                    ui.add_space(10.0);
                }
                if contacts.is_empty() && requests.is_empty() {
                    ui.label(RichText::new("no contacts yet").color(p.dim));
                }
            });
        if let Some(k) = picked {
            self.select(k);
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                if ui.button("+ Add").clicked() {
                    self.dialog = Some(Dialog::add());
                }
                if ui
                    .button("Invite")
                    .on_hover_text("Your invite, to send to a friend")
                    .clicked()
                {
                    self.dialog = Some(Dialog::Invite {
                        text: self.engine.invite(),
                        kind: crate::dialog::Share::Once,
                    });
                }
                if ui.button("Settings").clicked() {
                    self.dialog = Some(Dialog::Settings);
                }
                ui.button("Lock")
                    .on_hover_text("Ask for the password again")
                    .clicked()
            })
            .inner
        })
        .inner
    }

    /// One contact in the list; its key if clicked.
    fn row(&self, ui: &mut egui::Ui, p: &Pal, i: usize) -> Option<Key> {
        let c = &self.engine.data.contacts[i];
        let name = self.engine.name(c);
        let open = self.open == Some(c.key);
        let fill = match open {
            true => p.surface,
            false => Color32::TRANSPARENT,
        };
        let r = egui::Frame::new()
            .fill(fill)
            .corner_radius(4)
            .inner_margin(6)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    avatar(ui, p, &name, 32.0);
                    ui.vertical(|ui| {
                        let colour = if c.blocked || c.left { p.dim } else { p.text };
                        ui.label(RichText::new(&name).monospace().color(colour));
                        let (state, ok) = list_state(c, self.engine.is_request(&c.key));
                        ui.label(caps(state, if ok { p.accent } else { p.warn }));
                    });
                    if c.unread > 0 {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let n = if c.unread > 99 {
                                "99+".into()
                            } else {
                                c.unread.to_string()
                            };
                            ui.label(RichText::new(n).strong().color(p.accent));
                        });
                    }
                });
            })
            .response
            .interact(egui::Sense::click());
        r.clicked().then_some(c.key)
    }

    fn chat(&mut self, ui: &mut egui::Ui, p: &Pal) {
        let Some(i) = self.open.and_then(|k| self.engine.index(&k)) else {
            ui.centered_and_justified(|ui| {
                crate::synth::backdrop(ui, p, ui.max_rect(), true);
                ui.label(RichText::new("Pick a contact on the left.").color(p.dim));
            });
            return;
        };
        let request = self.engine.is_request(&self.engine.data.contacts[i].key);
        self.header(ui, p, i, request);
        egui::Panel::bottom("input")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(10))
            .show(ui, |ui| self.input_row(ui, p, i));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(12))
            .show(ui, |ui| {
                let all = ui.max_rect().expand(12.0);
                crate::synth::backdrop(ui, p, all, false);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if let Some(m) = messages(ui, p, &self.engine.data.contacts[i]) {
                            self.dialog = Some(Dialog::save_file(m));
                        }
                    });
            });
    }

    fn header(&mut self, ui: &mut egui::Ui, p: &Pal, i: usize, request: bool) {
        let c = &self.engine.data.contacts[i];
        let name = self.engine.name(c);
        let mut accept = false;
        let mut next = None;
        egui::Panel::top("chat-header")
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(12)
                    .stroke(Stroke::new(1.0, p.line)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    avatar(ui, p, &name, 40.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&name).strong().size(16.0));
                        let (state, ok) = chat_state(c);
                        ui.label(caps(state, if ok { p.accent } else { p.warn }));
                        if let Some(prof) = self.engine.profile_of(c) {
                            let as_ = termchat_engine::clean(&prof.nickname).to_uppercase();
                            ui.label(caps(&format!("INCOGNITO AS {as_}"), p.dim));
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.menu_button("•••", |ui| next = chat_menu(ui, c, request));
                        if request {
                            accept = ui.button("Accept").clicked();
                            ui.label(RichText::new("contact request").color(p.warn));
                        }
                    });
                });
                let waiting = c.hidden.len() + c.held_back as usize;
                if request && waiting > 0 {
                    ui.label(
                        RichText::new(format!("{waiting} messages hidden until you accept"))
                            .color(p.dim),
                    );
                }
            });
        if accept {
            self.engine.accept(i);
        }
        match next {
            Some(Menu::Open(d)) => self.dialog = Some(d),
            Some(Menu::Block) => self.engine.edit(i, |c| c.blocked = !c.blocked),
            Some(Menu::NoHistory) => self.engine.edit(i, |c| c.no_history = !c.no_history),
            Some(Menu::Report) => {
                if let Some(text) = self.engine.report_text(i) {
                    self.dialog = Some(Dialog::Report(text));
                }
            }
            None => {}
        }
    }

    fn input_row(&mut self, ui: &mut egui::Ui, p: &Pal, i: usize) {
        let timer = self.engine.data.contacts[i].expires_in;
        let label = match timer {
            Some(s) => format!("timer {}", span(u64::from(s))),
            None => "timer off".into(),
        };
        let file_text = RichText::new("File");
        let timer_text = RichText::new(label).monospace();
        // SEND is as wide as the two buttons under it.
        let width = |t: &RichText| {
            egui::WidgetText::from(t.clone())
                .into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button)
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x
        };
        let send_width = width(&file_text) + ui.spacing().item_spacing.x + width(&timer_text);
        let mut send = false;
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let column = Layout::top_down(Align::Min);
                ui.allocate_ui_with_layout(egui::vec2(send_width, 0.0), column, |ui| {
                    let button = crate::synth::button(ui, p, "SEND", egui::vec2(send_width, 28.0));
                    send = button.clicked();
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(file_text).corner_radius(2))
                            .on_hover_text("Send a file, up to 1 MiB")
                            .clicked()
                        {
                            self.dialog = Some(Dialog::SendFile {
                                path: String::new(),
                                error: String::new(),
                            });
                        }
                        let t = ui
                            .add(egui::Button::new(timer_text).corner_radius(2))
                            .on_hover_text("Disappearing messages: click for the next timer");
                        if t.clicked() {
                            self.engine
                                .edit(i, |c| c.expires_in = next_timer(c.expires_in));
                        }
                    });
                });
                let id = ui.make_persistent_id("composer");
                let focused = ui.memory(|m| m.has_focus(id));
                // Enter sends; Shift+Enter breaks the line.
                let enter = focused
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                send |= enter;
                // Three lines on view, wrapped; the rest scrolls inside the frame.
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let rows = ui
                    .fonts_mut(|f| f.layout_no_wrap("x\nx\nx".into(), font, Color32::WHITE))
                    .size()
                    .y;
                let v = ui.visuals();
                let stroke = match focused {
                    true => v.selection.stroke,
                    false => v.widgets.noninteractive.bg_stroke,
                };
                egui::Frame::new()
                    .fill(v.extreme_bg_color)
                    .stroke(stroke)
                    .corner_radius(4)
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(rows)
                            .min_scrolled_height(rows)
                            .auto_shrink([false, true])
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                            )
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.input)
                                        .id(id)
                                        .frame(egui::Frame::NONE)
                                        .margin(egui::Margin::ZERO)
                                        .min_size(egui::vec2(0.0, rows))
                                        .desired_width(f32::INFINITY)
                                        .hint_text("Encrypted message")
                                        .font(egui::TextStyle::Monospace),
                                )
                            });
                    });
            });
        });
        if !send || self.input.trim().is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.input);
        if let Err(e) = self.engine.send(i, &text) {
            self.input = text;
            self.status = e;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, p: &Pal) {
        let (mut backup, mut news) = (false, false);
        let unread_news = self.engine.news_unread();
        let e = &self.engine;
        ui.horizontal(|ui| {
            let (text, ok) = match (e.connected, e.data.tor) {
                (true, true) => ("● online via Tor", true),
                (false, true) => ("○ connecting via Tor…", false),
                (true, false) => ("● online", true),
                (false, false) => ("○ connecting…", false),
            };
            ui.label(RichText::new(text).color(if ok { p.accent } else { p.warn }));
            ui.label(RichText::new(termchat_engine::clean(&e.data.nickname)).color(p.dim));
            if !e.data.backed_up {
                backup = ui
                    .link(RichText::new("! no backup yet").color(p.warn))
                    .clicked();
            }
            let note = match &e.update {
                Some(v) => format!("termchat.app {v} is out"),
                None => "new in this version".into(),
            };
            if unread_news {
                news = ui.link(RichText::new(note).color(p.accent)).clicked();
            }
            ui.label(RichText::new(&self.status).color(p.warn));
        });
        if backup {
            self.dialog = Some(Dialog::backup());
        }
        if news {
            self.dialog = Some(Dialog::News);
        }
    }

    fn toasts(&mut self, ctx: &egui::Context, p: &Pal) {
        let mut picked = None;
        for (n, t) in self.toasts.iter().enumerate() {
            let y = -40.0 - 52.0 * n as f32;
            egui::Area::new(egui::Id::unique(("toast", n)))
                .anchor(Align2::RIGHT_BOTTOM, [-16.0, y])
                .show(ctx, |ui| {
                    let r = egui::Frame::new()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0, p.accent))
                        .corner_radius(4)
                        .inner_margin(10)
                        .show(ui, |ui| ui.label(&t.text))
                        .response
                        .interact(egui::Sense::click());
                    if r.clicked() {
                        picked = Some(t.key);
                    }
                });
        }
        if let Some(k) = picked {
            self.toasts.retain(|t| t.key != k);
            self.select(k);
        }
    }
}

/// What the chat menu asked for.
enum Menu {
    Open(Dialog),
    Block,
    NoHistory,
    Report,
}

/// The "⋮" menu of the open chat.
fn chat_menu(ui: &mut egui::Ui, c: &Contact, request: bool) -> Option<Menu> {
    let verified = if c.verified {
        "verified"
    } else {
        "not verified"
    };
    let pick = |ui: &mut egui::Ui, label: &str| ui.button(label).clicked();
    let next = match () {
        _ if pick(ui, &format!("Safety number ({verified})")) => Menu::Open(Dialog::Verify),
        _ if pick(ui, "Rename…") => {
            Menu::Open(Dialog::Rename(c.petname.clone().unwrap_or_default()))
        }
        _ if pick(
            ui,
            if c.no_history {
                "Keep history"
            } else {
                "No history"
            },
        ) =>
        {
            Menu::NoHistory
        }
        _ if pick(ui, if c.blocked { "Unblock" } else { "Block" }) => Menu::Block,
        _ if pick(ui, "Report…") => Menu::Report,
        _ if pick(
            ui,
            if request {
                "Reject request…"
            } else {
                "Delete…"
            },
        ) =>
        {
            Menu::Open(Dialog::Delete)
        }
        _ => return None,
    };
    ui.close();
    Some(next)
}

/// The chat history. `Some(m)`: Save was clicked on message `m`, a file.
fn messages(ui: &mut egui::Ui, p: &Pal, c: &Contact) -> Option<usize> {
    let width = ui.available_width();
    let mut save = None;
    for (n, m) in c.messages.iter().enumerate() {
        let text = match &m.file {
            Some(f) => clean_text(f.label().trim_start_matches("📄 ")),
            None => clean_text(&m.text),
        };
        if m.status == Status::System {
            ui.vertical_centered(|ui| {
                egui::Frame::new()
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(2)
                    .inner_margin(6)
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).monospace().color(p.dim))
                    });
            });
            ui.add_space(6.0);
            continue;
        }
        let mark = match &m.status {
            Status::Queued | Status::Sending(_) => " · sending".to_string(),
            Status::Sent => " · sent".into(),
            Status::Failed(why) => format!(" · failed: {why}"),
            _ => String::new(),
        };
        let align = if m.mine { Align::Max } else { Align::Min };
        ui.with_layout(Layout::top_down(align), |ui| {
            ui.set_max_width(width * 0.7);
            egui::Frame::new()
                .fill(if m.mine { p.mine } else { p.theirs })
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| ui.label(RichText::new(text).monospace()));
            let colour = if matches!(m.status, Status::Failed(_)) {
                p.danger
            } else {
                p.dim
            };
            ui.label(
                RichText::new(format!("{}{mark}", age(m.at)))
                    .small()
                    .color(colour),
            );
            let unsaved = m
                .file
                .as_ref()
                .is_some_and(|f| !m.mine && f.complete() && f.data.is_some());
            if unsaved && ui.link(RichText::new("Save…").color(p.accent)).clicked() {
                save = Some(n);
            }
        });
        ui.add_space(4.0);
    }
    save
}

/// A square with initials, like the Android app; the "·fingerprint" a
/// name may carry (ADR-007) is not part of them.
fn avatar(ui: &mut egui::Ui, p: &Pal, name: &str, size: f32) {
    let initials: String = name
        .split('·')
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    let initials = if initials.is_empty() {
        "?".into()
    } else {
        initials
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4, p.surface);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        initials,
        egui::FontId::proportional(size * 0.4),
        p.text,
    );
}

fn caps(text: &str, colour: Color32) -> RichText {
    RichText::new(text).monospace().small().color(colour)
}

/// The line under a name in the list, and whether it is good news.
fn list_state(c: &Contact, request: bool) -> (&'static str, bool) {
    match () {
        _ if request => ("CONTACT REQUEST", false),
        _ if c.left => ("LEFT", false),
        _ if c.blocked => ("BLOCKED", false),
        _ if !c.verified => ("NOT VERIFIED", false),
        _ => ("ENCRYPTED", true),
    }
}

/// The line under the name in the chat header.
fn chat_state(c: &Contact) -> (&'static str, bool) {
    match () {
        _ if c.left => ("LEFT", false),
        _ if c.blocked => ("BLOCKED", false),
        _ if c.verified => ("ENCRYPTED · VERIFIED", true),
        _ => ("ENCRYPTED · NOT VERIFIED", false),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // tests only
    use super::*;
    use termchat_engine::engine::now;

    #[test]
    fn every_theme_name_resolves_and_unknown_is_the_first() {
        assert_eq!(
            Pal::named("termchat").accent,
            Color32::from_rgb(0xc3, 0xe6, 0x6e)
        );
        assert_eq!(Pal::named("nope").bg, Pal::named(themes::THEMES[0].0).bg);
    }

    /// Every text the frame drew.
    fn texts(out: &egui::FullOutput) -> String {
        fn walk(s: &egui::Shape, acc: &mut String) {
            match s {
                egui::Shape::Text(t) => {
                    acc.push_str(t.galley.text());
                    acc.push('\n');
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, acc)),
                _ => {}
            }
        }
        let mut acc = String::new();
        for c in &out.shapes {
            walk(&c.shape, &mut acc);
        }
        acc
    }

    /// The whole window, headless: list, header, bubbles, card, status bar.
    #[test]
    fn unlocked_window_draws_a_chat() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _rt = rt.enter();
        let dir = std::env::temp_dir().join(format!("termchat-gui-{}", std::process::id()));
        let setup = termchat_engine::Setup {
            // Nothing listens there: the engine keeps trying, offline.
            relay_url: "ws://127.0.0.1:9/",
            onion_url: None,
            tor: false,
            nickname: "me",
            password: "correct horse battery staple violin",
        };
        let mut engine = Engine::setup(&dir.join("state"), &setup).unwrap();
        let msg = |mine, text: &str, status| termchat_engine::data::Msg {
            mine,
            text: text.into(),
            at: now(),
            expires_at: None,
            status,
            file: None,
        };
        engine.data.contacts.push(Contact {
            key: [1; 32],
            petname: Some("Alice Smith".into()),
            unread: 2,
            messages: vec![
                msg(false, "hi there", Status::Received),
                msg(true, "hello", Status::Sent),
                msg(false, "request accepted", Status::System),
                {
                    let mut m = msg(false, "notes.txt", Status::Received);
                    m.file = Some(termchat_engine::data::FileMsg {
                        name: "notes.txt".into(),
                        size: 5,
                        done: 1,
                        data: Some(b"hello".to_vec()),
                        ..Default::default()
                    });
                    m
                },
            ],
            ..Contact::default()
        });
        let mut chat = Chat::new(engine);
        let mut settings = Settings::default();
        let (ctx, pal) = (egui::Context::default(), Pal::named("termchat"));
        pal.apply(&ctx);
        let mut frame = |chat: &mut Chat| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                chat.ui(ui, &pal, &mut settings);
            });
            // No GPU here to take the font atlas.
            out.textures_delta.clear();
            out
        };
        let shown = texts(&frame(&mut chat));
        assert!(shown.contains("Pick a contact"), "{shown}");
        assert!(shown.contains("Contacts  1") && shown.contains("NOT VERIFIED"));

        chat.select([1; 32]);
        // As after an update: this version's notes are not read yet.
        chat.engine.data.news_seen.clear();
        chat.alert([1; 32], false, false, false);
        assert!(chat.step(true, false).is_none());
        assert_eq!(
            chat.engine.data.contacts[0].unread, 0,
            "an open chat is read"
        );
        frame(&mut chat);
        let shown = texts(&frame(&mut chat));
        for want in [
            "ENCRYPTED · NOT VERIFIED",
            "hi there",
            "hello",
            "request accepted",
            "notes.txt",
            "Save…",
            "new in this version",
        ] {
            assert!(shown.contains(want), "{want} missing in:\n{shown}");
        }
        assert!(shown.contains("new message · Alice Smith"), "the card");
        assert!(shown.contains("connecting"), "the status bar");

        let invite = chat.engine.invite();
        let dialogs = [
            (Dialog::Verify, "Safety number"),
            (Dialog::Rename(String::new()), "Rename"),
            (Dialog::Delete, "Delete the contact"),
            (Dialog::Report("x".into()), "Report to the relay operator"),
            (Dialog::Settings, "Settings"),
            (Dialog::backup(), "Back up your identity"),
            (Dialog::add(), "Add a contact"),
            (
                Dialog::Invite {
                    text: invite,
                    kind: crate::dialog::Share::Once,
                },
                "YOUR FINGERPRINT",
            ),
            (Dialog::Notice("all good".into()), "all good"),
            (Dialog::save_file(3), "Save a file"),
            (
                Dialog::SendFile {
                    path: String::new(),
                    error: String::new(),
                },
                "Send a file",
            ),
            (Dialog::Password(Default::default()), "Change password"),
            (Dialog::Duress(Default::default()), "Duress password"),
            (
                Dialog::Bridges {
                    text: Default::default(),
                    error: String::new(),
                },
                "none: Tor connects directly",
            ),
            (
                Dialog::Wipe {
                    typed: String::new(),
                },
                "Type WIPE or MOVED",
            ),
            (Dialog::WipeAnyway("offline".into()), "Wipe without telling"),
            // The newest heading: older ones may be scrolled out of sight.
            (Dialog::News, env!("CARGO_PKG_VERSION")),
        ];
        for (d, want) in dialogs {
            chat.dialog = Some(d);
            frame(&mut chat);
            let shown = texts(&frame(&mut chat));
            assert!(shown.contains(want), "{want} missing in:\n{shown}");
            assert!(chat.dialog.is_some(), "{want}: stays open until answered");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
