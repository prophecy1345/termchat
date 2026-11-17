//! The unlocked window: contact list, chat, status bar (ADR-018). The look
//! follows the Android app (ADR-011, section 5).

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Align2, Color32, Layout, RichText, Stroke};
use termchat_core::proto::{REACTIONS, Ref};
use termchat_engine::data::{Contact, Group, Msg, Status};
use termchat_engine::engine::{
    OLDER_APP, REACTION_NAMES, TIMERS, age, clean_text, next_timer, span,
};
use termchat_engine::notify::{self, SOUND_GAP};
use termchat_engine::settings::Settings;
use termchat_engine::{Engine, Happened, reactions, themes};

use crate::dialog::Dialog;

type Key = [u8; 32];

/// What the right side shows: a chat or a group (ADR-027), by index.
#[derive(Clone, Copy)]
enum Open {
    Chat(usize),
    Group(usize),
}

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
    /// Each reaction's colour (ADR-025, `reactions::COLOURS`).
    pub reactions: [Color32; 7],
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
            reactions: reactions::COLOURS.map(rgb),
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

    /// A light background: the theme is a light one.
    pub fn light(&self) -> bool {
        let [r, g, b, _] = self.bg.to_array();
        u32::from(r) + u32::from(g) + u32::from(b) > 3 * 128
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let light = self.light();
        let mut v = match light {
            true => egui::Visuals::light(),
            false => egui::Visuals::dark(),
        };
        // egui's own widget greys suit only its own dark look: on a light
        // theme buttons, fields and lists came out dark grey with dark text.
        if light {
            let mix = crate::synth::mix;
            let w = &mut v.widgets;
            // The theme's light surface, a frame in its line colour, a touch
            // of accent when hovered or pressed: text keeps the contrast it
            // has everywhere else in the theme.
            for (state, fill) in [
                (&mut w.inactive, self.surface),
                (&mut w.hovered, mix(self.surface, self.accent, 0.12)),
                (&mut w.active, mix(self.surface, self.accent, 0.22)),
                (&mut w.open, mix(self.surface, self.accent, 0.12)),
            ] {
                state.bg_fill = fill;
                state.weak_bg_fill = fill;
                state.bg_stroke = Stroke::new(1.0, self.line);
                state.fg_stroke.color = self.text;
            }
            w.hovered.bg_stroke = Stroke::new(1.0, self.accent);
        }
        v.panel_fill = self.bg;
        v.window_fill = self.surface;
        // Fields a shade off the window they sit in, or they vanish into it.
        v.extreme_bg_color = self.bg;
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
    /// The open group, when no chat is open (ADR-027).
    pub(crate) group: Option<[u8; 16]>,
    input: String,
    toasts: Vec<Toast>,
    last_sound: Option<Instant>,
    last_tick: Instant,
    /// One line: the last note from the engine.
    pub(crate) status: String,
    pub(crate) dialog: Option<Dialog>,
    /// What SEND does with the text (ADR-025).
    compose: Compose,
    /// Put the cursor in the field, at the end of its text, next frame.
    focus_input: bool,
}

/// What SEND does with the typed text (ADR-025).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Compose {
    #[default]
    New,
    /// An answer to this message; `theirs`: the contact's.
    Reply(Ref),
    /// New text for our message with this id.
    Edit(u64),
}

/// What a click in the history asks for, by message index.
enum Act {
    Save(usize),
    Reply(usize),
    Edit(usize),
    Delete(usize),
    React(usize, u8),
}

impl Chat {
    pub fn new(engine: Box<Engine>) -> Self {
        let mut chat = Self {
            engine,
            open: None,
            group: None,
            input: String::new(),
            toasts: Vec::new(),
            last_sound: None,
            last_tick: Instant::now(),
            status: String::new(),
            dialog: None,
            compose: Compose::New,
            focus_input: false,
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
        if let Some(g) = self.open_group() {
            self.engine.data.groups[g].unread = 0;
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
                Happened::Invite(text) => {
                    self.dialog = Some(Dialog::Invite {
                        text,
                        kind: crate::dialog::Share::Incognito,
                    })
                }
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
        self.group = None;
        self.input.clear();
        self.compose = Compose::New;
    }

    pub(crate) fn select_group(&mut self, g: usize) {
        self.group = self.engine.data.groups.get(g).map(|g| g.id);
        self.open = None;
        self.input.clear();
        self.compose = Compose::New;
    }

    pub(crate) fn open_group(&self) -> Option<usize> {
        let id = self.group?;
        self.engine.data.groups.iter().position(|g| g.id == id)
    }

    fn msgs(&self, at: Open) -> &[Msg] {
        match at {
            Open::Chat(i) => &self.engine.data.contacts[i].messages,
            Open::Group(g) => &self.engine.data.groups[g].messages,
        }
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
        let mut picked_group = None;
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
                    ui.add_space(10.0);
                }
                // ADR-027: small private groups, after the contacts.
                let groups = self.engine.data.groups.len();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Groups  {groups}")).color(p.dim));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button("+ New")
                            .on_hover_text("A new private group")
                            .clicked()
                        {
                            self.dialog = Some(Dialog::NewGroup {
                                name: String::new(),
                                error: String::new(),
                            });
                        }
                    });
                });
                for g in 0..groups {
                    if self.group_row(ui, p, g) {
                        picked_group = Some(g);
                    }
                }
            });
        if let Some(k) = picked {
            self.select(k);
        }
        if let Some(g) = picked_group {
            self.select_group(g);
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
                    match self.engine.invite() {
                        Ok(text) => {
                            self.dialog = Some(Dialog::Invite {
                                text,
                                kind: crate::dialog::Share::Once,
                            })
                        }
                        Err(e) => self.status = e,
                    }
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
                        let (state, ok) = match self.engine.needs_invite(c) {
                            true => ("NEEDS A NEW INVITE", false),
                            false => list_state(c, self.engine.is_request(&c.key)),
                        };
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

    /// One group in the list; true if clicked.
    fn group_row(&self, ui: &mut egui::Ui, p: &Pal, g: usize) -> bool {
        let group = &self.engine.data.groups[g];
        let name = termchat_engine::clean(group.name());
        let fill = match self.group == Some(group.id) {
            true => p.surface,
            false => Color32::TRANSPARENT,
        };
        egui::Frame::new()
            .fill(fill)
            .corner_radius(4)
            .inner_margin(6)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    avatar(ui, p, &format!("# {name}"), 32.0);
                    ui.vertical(|ui| {
                        let colour = if group.left { p.dim } else { p.text };
                        ui.label(RichText::new(&name).monospace().color(colour));
                        let (state, ok) = group_state(group);
                        ui.label(caps(&state, if ok { p.accent } else { p.dim }));
                    });
                    if group.unread > 0 {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let n = group.unread.min(99).to_string();
                            ui.label(RichText::new(n).strong().color(p.accent));
                        });
                    }
                });
            })
            .response
            .interact(egui::Sense::click())
            .clicked()
    }

    fn group_chat(&mut self, ui: &mut egui::Ui, p: &Pal, g: usize) {
        self.group_header(ui, p, g);
        egui::Panel::bottom("input")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(10))
            .show(ui, |ui| self.input_row(ui, p, Open::Group(g)));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(12))
            .show(ui, |ui| {
                let all = ui.max_rect().expand(12.0);
                crate::synth::backdrop(ui, p, all, false);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let group = &self.engine.data.groups[g];
                        let me = self.engine.me();
                        let view = View {
                            msgs: &group.messages,
                            group: Some(group),
                            me,
                            rich: true,
                        };
                        if let Some(act) = messages(ui, p, view) {
                            self.act(Open::Group(g), act);
                        }
                    });
            });
    }

    fn group_header(&mut self, ui: &mut egui::Ui, p: &Pal, g: usize) {
        let group = &self.engine.data.groups[g];
        let name = termchat_engine::clean(group.name());
        let admin = group.is_admin();
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
                    avatar(ui, p, &format!("# {name}"), 40.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&name).strong().size(16.0));
                        let (mut state, ok) = group_state(group);
                        if admin {
                            state.push_str(" · YOU ARE ADMIN");
                        }
                        ui.label(caps(&state, if ok { p.accent } else { p.dim }));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Right to left: the last one stands first.
                        if !group.left && ui.button("Leave").clicked() {
                            next = Some(Dialog::LeaveGroup);
                        }
                        if admin && ui.button("Invite").clicked() {
                            next = Some(Dialog::InviteToGroup(String::new()));
                        }
                        if ui.button("Members").clicked() {
                            next = Some(Dialog::Members);
                        }
                    });
                });
            });
        if let Some(d) = next {
            self.dialog = Some(d);
        }
    }

    fn chat(&mut self, ui: &mut egui::Ui, p: &Pal) {
        if let Some(g) = self.open_group() {
            return self.group_chat(ui, p, g);
        }
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
            .show(ui, |ui| self.input_row(ui, p, Open::Chat(i)));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(12))
            .show(ui, |ui| {
                let all = ui.max_rect().expand(12.0);
                crate::synth::backdrop(ui, p, all, false);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let view = View {
                            msgs: &self.engine.data.contacts[i].messages,
                            group: None,
                            me: self.engine.me(),
                            rich: self.engine.rich(i),
                        };
                        if let Some(act) = messages(ui, p, view) {
                            self.act(Open::Chat(i), act);
                        }
                    });
            });
    }

    fn act(&mut self, at: Open, act: Act) {
        let msgs = self.msgs(at);
        let target = |n: usize| {
            let m: &Msg = &msgs[n];
            m.id.map(|id| Ref {
                id,
                theirs: !m.mine,
            })
        };
        let rich = match at {
            Open::Chat(i) => self.engine.rich(i),
            Open::Group(_) => true,
        };
        match act {
            Act::Save(n) => self.dialog = Some(Dialog::save_file(n)),
            _ if !rich => self.status = OLDER_APP.into(),
            Act::Reply(n) => {
                self.compose = target(n).map_or(Compose::New, Compose::Reply);
                self.focus_input = true;
            }
            Act::Edit(n) => {
                if let Some(Ref { id, .. }) = target(n) {
                    self.input = msgs[n].text.clone();
                    self.compose = Compose::Edit(id);
                    self.focus_input = true;
                }
            }
            Act::Delete(n) => self.dialog = Some(Dialog::DeleteMessage(n)),
            Act::React(n, r) => {
                let done = match at {
                    Open::Chat(i) => {
                        // The same one again takes it back.
                        let to = (msgs[n].my_reaction != Some(r)).then_some(r);
                        self.engine.react(i, n, to)
                    }
                    Open::Group(g) => {
                        let mine = my_group_reaction(
                            &self.engine.data.groups[g],
                            &msgs[n],
                            self.engine.me(),
                        );
                        let to = (mine != Some(r)).then_some(r);
                        self.engine.react_group(g, n, to)
                    }
                };
                if let Err(e) = done {
                    self.status = e;
                }
            }
        }
    }

    /// Where the message `r` points at is now. In a group the id alone names it.
    fn locate(&self, at: Open, r: Ref) -> Option<usize> {
        let group = matches!(at, Open::Group(_));
        self.msgs(at)
            .iter()
            .position(|m| m.id == Some(r.id) && (group || m.mine != r.theirs) && !m.deleted)
    }

    fn header(&mut self, ui: &mut egui::Ui, p: &Pal, i: usize, request: bool) {
        let c = &self.engine.data.contacts[i];
        let name = self.engine.name(c);
        let mut accept = false;
        let mut next = None;
        let mut join = None;
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
                        let (state, ok) = match self.engine.needs_invite(c) {
                            true => ("NEEDS A NEW INVITE: ASK THEM FOR ONE, OR SEND YOURS", false),
                            false => chat_state(c),
                        };
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
                // ADR-027: their group invites, answered here.
                for (n, inv) in self.engine.data.group_invites.iter().enumerate() {
                    if inv.from != c.key || request {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        let what = termchat_engine::clean(&inv.roster.name);
                        ui.label(
                            RichText::new(format!("invites you to the group \"{what}\""))
                                .color(p.warn),
                        );
                        if ui.button("Join").clicked() {
                            join = Some((n, true));
                        }
                        if ui.button("Decline").clicked() {
                            join = Some((n, false));
                        }
                    });
                }
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
        match join {
            Some((n, true)) => match self.engine.accept_group(n) {
                Ok(g) => self.select_group(g),
                Err(e) => self.status = e,
            },
            Some((n, false)) => self.engine.decline_group(n),
            None => {}
        }
        match next {
            Some(Menu::Open(d)) => self.dialog = Some(d),
            Some(Menu::Block) => {
                let on = !self.engine.data.contacts[i].blocked;
                self.engine.block(i, on);
            }
            Some(Menu::NoHistory) => self.engine.edit(i, |c| c.no_history = !c.no_history),
            Some(Menu::Report) => {
                if let Some(text) = self.engine.report_text(i) {
                    self.dialog = Some(Dialog::Report(text));
                }
            }
            None => {}
        }
    }

    fn input_row(&mut self, ui: &mut egui::Ui, p: &Pal, at: Open) {
        let timer = match at {
            Open::Chat(i) => self.engine.data.contacts[i].expires_in,
            Open::Group(g) => self.engine.data.groups[g].timer(),
        };
        let label = |t: Option<u32>| {
            RichText::new(match t {
                Some(s) => format!("timer {}", span(u64::from(s))),
                None => "timer off".into(),
            })
            .monospace()
        };
        let file_text = RichText::new("File");
        let timer_text = label(timer);
        // SEND is as wide as the two buttons under it; the timer button keeps
        // the width of its longest label, so nothing jumps when it changes.
        let width = |t: &RichText| {
            egui::WidgetText::from(t.clone())
                .into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button)
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x
        };
        let timer_width = TIMERS.iter().map(|t| width(&label(*t))).fold(0.0, f32::max);
        let send_width = width(&file_text) + ui.spacing().item_spacing.x + timer_width;
        // Three lines on view, wrapped; the rest scrolls inside the frame.
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let rows = ui
            .fonts_mut(|f| f.layout_no_wrap("x\nx\nx".into(), font, Color32::WHITE))
            .size()
            .y;
        // The field's frame: text, margins, stroke. The buttons span it exactly.
        let (margin, stroke_width): (i8, f32) = (6, 1.0);
        let field_height = rows + 2.0 * (f32::from(margin) + stroke_width);
        let mut send = false;
        let banner = match self.compose {
            Compose::New => None,
            Compose::Reply(r) => Some(match self.locate(at, r) {
                Some(n) => {
                    let text = clean_text(&self.msgs(at)[n].text);
                    let first = text.lines().next().unwrap_or_default().to_string();
                    format!("↳ Answering: {first}")
                }
                None => "↳ Answering a message that is gone".into(),
            }),
            Compose::Edit(_) => Some("Editing your message…".into()),
        };
        if let Some(banner) = banner {
            let esc = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            ui.horizontal(|ui| {
                // The strip itself cancels: a click, or Esc.
                let cancel = ui
                    .add(
                        egui::Label::new(RichText::new(banner).color(p.accent))
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text("Click or Esc to cancel");
                if cancel.clicked() || esc {
                    if matches!(self.compose, Compose::Edit(_)) {
                        self.input.clear();
                    }
                    self.compose = Compose::New;
                }
            });
        }
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let column = Layout::top_down(Align::Min);
                let size = egui::vec2(send_width, field_height);
                ui.allocate_ui_with_layout(size, column, |ui| {
                    ui.set_min_height(field_height);
                    let button = crate::synth::button(ui, p, "SEND", egui::vec2(send_width, 28.0));
                    send = button.clicked();
                    // File and the timer sit on the field's bottom edge.
                    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                        ui.horizontal(|ui| {
                            // Groups carry text only for now.
                            if matches!(at, Open::Chat(_))
                                && ui
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
                                .add(
                                    egui::Button::new(timer_text)
                                        .corner_radius(2)
                                        .min_size(egui::vec2(timer_width, 0.0)),
                                )
                                .on_hover_text(match at {
                                    Open::Chat(_) => {
                                        "Disappearing messages: click for the next timer"
                                    }
                                    Open::Group(_) => {
                                        "Disappearing messages: the group's admin sets it"
                                    }
                                });
                            if t.clicked() {
                                match at {
                                    Open::Chat(i) => self
                                        .engine
                                        .edit(i, |c| c.expires_in = next_timer(c.expires_in)),
                                    Open::Group(g) => {
                                        if let Err(e) =
                                            self.engine.set_group_timer(g, next_timer(timer))
                                        {
                                            self.status = e;
                                        }
                                    }
                                }
                            }
                        });
                    });
                });
                let id = ui.make_persistent_id("composer");
                let focused = ui.memory(|m| m.has_focus(id));
                // Enter sends; Shift+Enter breaks the line.
                let enter = focused
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                send |= enter;
                let v = ui.visuals();
                let stroke = match focused {
                    true => v.selection.stroke,
                    false => v.widgets.noninteractive.bg_stroke,
                };
                let stroke = Stroke::new(stroke_width, stroke.color);
                egui::Frame::new()
                    .fill(v.extreme_bg_color)
                    .stroke(stroke)
                    .corner_radius(4)
                    .inner_margin(egui::Margin::symmetric(8, margin))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(rows)
                            .min_scrolled_height(rows)
                            .auto_shrink([false, true])
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                            )
                            .show(ui, |ui| {
                                if std::mem::take(&mut self.focus_input) {
                                    let end = egui::text::CCursor::new(self.input.chars().count());
                                    let mut state = egui::TextEdit::load_state(ui.ctx(), id)
                                        .unwrap_or_default();
                                    state
                                        .cursor
                                        .set_char_range(Some(egui::text::CCursorRange::one(end)));
                                    state.store(ui.ctx(), id);
                                    ui.memory_mut(|m| m.request_focus(id));
                                }
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
        let gone = "the message you answer is gone; Esc to write without it";
        let sent = match (self.compose, at) {
            (Compose::New, Open::Chat(i)) => self.engine.send(i, &text),
            (Compose::New, Open::Group(g)) => self.engine.send_group(g, &text),
            (Compose::Reply(r), _) => match (self.locate(at, r), at) {
                (Some(n), Open::Chat(i)) => self.engine.reply(i, n, &text),
                (Some(n), Open::Group(g)) => self.engine.reply_group(g, n, &text),
                (None, _) => Err(gone.into()),
            },
            (Compose::Edit(id), _) => match (self.locate(at, Ref { id, theirs: false }), at) {
                (Some(n), Open::Chat(i)) => self.engine.edit_message(i, n, &text),
                (Some(n), Open::Group(g)) => self.engine.edit_group_message(g, n, &text),
                (None, _) => Err("the message is gone; Esc to stop editing".into()),
            },
        };
        match sent {
            Ok(()) => self.compose = Compose::New,
            Err(e) => {
                self.input = text;
                self.status = e;
            }
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

/// What the history draws: a chat's messages, or a group's (ADR-027).
struct View<'a> {
    msgs: &'a [Msg],
    group: Option<&'a Group>,
    me: Key,
    /// The other side reads answers and reactions (ADR-025); always in a group.
    rich: bool,
}

/// Our reaction to `m` in group `g`.
fn my_group_reaction(g: &Group, m: &Msg, me: Key) -> Option<u8> {
    let id = m.id?;
    g.reactions_to(id).find(|(k, _)| *k == me).map(|(_, r)| r)
}

/// The history; what a click on a message asks for.
fn messages(ui: &mut egui::Ui, p: &Pal, v: View) -> Option<Act> {
    let width = ui.available_width();
    let mut act = None;
    let mut last_author = None;
    for (n, m) in v.msgs.iter().enumerate() {
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
            last_author = None;
            continue;
        }
        let my_reaction = match v.group {
            Some(g) => my_group_reaction(g, m, v.me),
            None => m.my_reaction,
        };
        // In a group, who wrote it, over each run of their messages.
        if let Some(g) = v.group
            && !m.mine
            && last_author != Some(m.author)
        {
            let who = m.author.and_then(|k| g.name_of(&k)).unwrap_or("someone");
            ui.label(
                RichText::new(termchat_engine::clean(who))
                    .small()
                    .color(p.mauve),
            );
        }
        last_author = Some(m.author);
        let mut mark = match &m.status {
            Status::Queued | Status::Sending(_) => " · sending".to_string(),
            Status::Sent => " · sent".into(),
            Status::Failed(why) => format!(" · failed: {why}"),
            _ => String::new(),
        };
        if m.edited {
            mark.push_str(" · edited");
        }
        let align = if m.mine { Align::Max } else { Align::Min };
        ui.with_layout(Layout::top_down(align), |ui| {
            ui.set_max_width(width * 0.7);
            let bubble = egui::Frame::new()
                .fill(if m.mine { p.mine } else { p.theirs })
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    if let Some(r) = m.reply_to {
                        let quoted = v.msgs.iter().find(|q| {
                            q.id == Some(r.id)
                                && (v.group.is_some() || q.mine != r.theirs)
                                && !q.deleted
                        });
                        let quote = match quoted {
                            Some(q) => {
                                let q = clean_text(&q.text);
                                format!("↳ {}", q.lines().next().unwrap_or_default())
                            }
                            None => "↳ message no longer here".into(),
                        };
                        let quote = RichText::new(quote).monospace().italics().color(p.dim);
                        ui.add(egui::Label::new(quote).truncate());
                    }
                    match m.deleted {
                        true => ui.label(RichText::new("message deleted").italics().color(p.dim)),
                        false => ui.label(RichText::new(text).monospace()),
                    }
                })
                .response;
            let pointable = v.rich && m.id.is_some() && !m.deleted;
            if pointable {
                let r = bubble
                    .interact(egui::Sense::click())
                    .on_hover_text(match m.mine {
                        true => "Double-click: answer. Right-click: react, edit, delete",
                        false => "Double-click: answer. Right-click: react",
                    });
                // As in most desktop messengers.
                if r.double_clicked() {
                    act = Some(Act::Reply(n));
                }
                r.context_menu(|ui| {
                    ui.horizontal(|ui| {
                        for r in 0..REACTIONS {
                            let on = my_reaction == Some(r);
                            let hint = REACTION_NAMES[usize::from(r)];
                            if sprite(ui, p, r, 2.0, on).on_hover_text(hint).clicked() {
                                act = Some(Act::React(n, r));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Answer").clicked() {
                        act = Some(Act::Reply(n));
                        ui.close();
                    }
                    if m.mine && m.file.is_none() && ui.button("Edit").clicked() {
                        act = Some(Act::Edit(n));
                        ui.close();
                    }
                    let delete = match v.group {
                        Some(_) => "Delete for everyone…",
                        None => "Delete for both…",
                    };
                    if m.mine && ui.button(delete).clicked() {
                        act = Some(Act::Delete(n));
                        ui.close();
                    }
                });
            }
            let colour = if matches!(m.status, Status::Failed(_)) {
                p.danger
            } else {
                p.dim
            };
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}{mark}", age(m.at)))
                        .small()
                        .color(colour),
                );
                // In a group: each reaction once, how many, and who on hover.
                if let (Some(g), Some(id)) = (v.group, m.id) {
                    let mut counts: Vec<(u8, Vec<String>)> = Vec::new();
                    for (k, r) in g.reactions_to(id) {
                        let who = match k == v.me {
                            true => "you".to_string(),
                            false => termchat_engine::clean(g.name_of(&k).unwrap_or("someone")),
                        };
                        match counts.iter_mut().find(|(x, _)| *x == r) {
                            Some((_, names)) => names.push(who),
                            None => counts.push((r, vec![who])),
                        }
                    }
                    for (r, names) in counts {
                        let hint =
                            format!("{}: {}", REACTION_NAMES[usize::from(r)], names.join(", "));
                        sprite(ui, p, r, 1.5, false).on_hover_text(hint);
                        if names.len() > 1 {
                            ui.label(RichText::new(names.len().to_string()).small().color(p.dim));
                        }
                    }
                }
                // Theirs, then ours (ADR-025).
                for (r, mine) in [(m.their_reaction, false), (m.my_reaction, true)] {
                    if let Some(r) = r {
                        let who = if mine { "you" } else { "they" };
                        let hint = format!("{who}: {}", REACTION_NAMES[usize::from(r)]);
                        sprite(ui, p, r, 1.5, false).on_hover_text(hint);
                    }
                }
            });
            let unsaved = m
                .file
                .as_ref()
                .is_some_and(|f| !m.mine && f.complete() && f.data.is_some());
            if unsaved && ui.link(RichText::new("Save…").color(p.accent)).clicked() {
                act = Some(Act::Save(n));
            }
        });
        ui.add_space(4.0);
    }
    act
}

/// Reaction `r` as its arcade sprite, `px` points a pixel, with a soft glow
/// (ADR-025). `on`: ours, framed.
fn sprite(ui: &mut egui::Ui, p: &Pal, r: u8, px: f32, on: bool) -> egui::Response {
    let side = 11.0 * px + 4.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    let name = REACTION_NAMES[usize::from(r).min(REACTION_NAMES.len() - 1)];
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    let colour = p.reactions[usize::from(r).min(6)];
    let painter = ui.painter_at(rect.expand(2.0));
    if on || response.hovered() {
        painter.rect_stroke(rect, 2, Stroke::new(1.0, colour), egui::StrokeKind::Inside);
    }
    let origin = rect.min + egui::vec2(2.0, 2.0);
    let cell = |x: u8, y: u8, grow: f32| {
        let at = origin + egui::vec2(f32::from(x) * px, f32::from(y) * px);
        egui::Rect::from_min_size(at, egui::vec2(px, px)).expand(grow)
    };
    for (x, y) in reactions::pixels(r) {
        painter.rect_filled(cell(x, y, px * 0.6), 0, colour.gamma_multiply(0.18));
    }
    for (x, y) in reactions::pixels(r) {
        painter.rect_filled(cell(x, y, 0.0), 0, colour);
    }
    response
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

/// The line under a group's name: its size, or that we are out of it.
fn group_state(g: &Group) -> (String, bool) {
    match (g.left, g.members().len()) {
        (true, _) => ("NO LONGER A MEMBER".into(), false),
        // Until the relay has made its queue.
        (false, 0) => ("SETTING UP…".into(), false),
        (false, 1) => ("1 MEMBER".into(), true),
        (false, n) => (format!("{n} MEMBERS"), true),
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
            unfetched: false,
            ..Default::default()
        };
        engine.data.contacts.push(Contact {
            key: [1; 32],
            petname: Some("Alice Smith".into()),
            unread: 2,
            // A contact with a queue: one without would need a new invite (ADR-023).
            theirs: Some(termchat_engine::data::Addr {
                relay_url: "ws://127.0.0.1:9/".into(),
                onion_url: None,
                send_id: [3; 16],
            }),
            understands: termchat_core::proto::UNDERSTANDS,
            messages: vec![
                {
                    let mut m = msg(false, "hi there", Status::Received);
                    m.id = Some(7);
                    m.their_reaction = Some(1);
                    m
                },
                {
                    // ADR-025: an answer, edited, with our reaction.
                    let mut m = msg(true, "hello", Status::Sent);
                    m.id = Some(8);
                    m.reply_to = Some(Ref {
                        id: 7,
                        theirs: true,
                    });
                    m.edited = true;
                    m.my_reaction = Some(6);
                    m
                },
                {
                    let mut m = msg(false, "gone", Status::Received);
                    m.id = Some(9);
                    m.wipe();
                    m
                },
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
            "↳ hi there",
            "· edited",
            "message deleted",
        ] {
            assert!(shown.contains(want), "{want} missing in:\n{shown}");
        }
        assert!(shown.contains("new message · Alice Smith"), "the card");
        assert!(shown.contains("connecting"), "the status bar");
        chat.act(Open::Chat(0), Act::Reply(0));
        let shown = texts(&frame(&mut chat));
        assert!(shown.contains("↳ Answering: hi there"), "{shown}");

        // No relay here, so no invite queue: an invite made by hand to draw.
        let invite = termchat_core::proto::Invite {
            version: termchat_core::proto::INVITE_VERSION,
            relay_url: "wss://relay.example.org/".into(),
            onion_url: None,
            identity: chat.engine.core().identity(),
            queue: [1; 16],
            code: [2; 16],
        }
        .encode();
        let dialogs = [
            (Dialog::Verify, "Safety number"),
            (Dialog::Rename(String::new()), "Rename"),
            (Dialog::Delete, "Delete the contact"),
            (Dialog::DeleteMessage(1), "Delete the message"),
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
            (Dialog::save_file(4), "Save a file"),
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

        // ADR-027: a group in the list, its history with authors and
        // counted reactions, and its dialogs.
        chat.dialog = None;
        let g = chat.engine.create_group("team").unwrap();
        chat.select_group(g);
        let me = chat.engine.me();
        let group = &mut chat.engine.data.groups[g];
        let mut said = msg(true, "group hello", Status::Sent);
        said.id = Some(7);
        said.author = Some(me);
        group.messages.push(said);
        group.set_reaction(7, me, Some(1));
        let shown = texts(&frame(&mut chat));
        for want in ["Groups  1", "team", "SETTING UP…", "group hello"] {
            assert!(shown.contains(want), "{want} missing in:\n{shown}");
        }
        let dialogs = [
            (
                Dialog::NewGroup {
                    name: String::new(),
                    error: String::new(),
                },
                "New group",
            ),
            (Dialog::Members, "team · members"),
            (Dialog::InviteToGroup(String::new()), "Invite to the group"),
            (Dialog::RemoveMember([1; 32]), "Remove from the group"),
            (Dialog::LeaveGroup, "Leave the group"),
            (Dialog::DeleteMessage(0), "Delete it for everyone"),
        ];
        for (d, want) in dialogs {
            chat.dialog = Some(d);
            frame(&mut chat);
            let shown = texts(&frame(&mut chat));
            assert!(shown.contains(want), "{want} missing in:\n{shown}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
