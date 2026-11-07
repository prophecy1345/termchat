//! Windows over the chat: contacts, invites, the safety number, settings,
//! backup (ADR-018). The engine does the work; these only ask and show.

use eframe::egui::{self, RichText};
use termchat_core::{fingerprint, safety_number};
use termchat_engine::engine::{clean_text, news};
use termchat_engine::settings::Settings;
use termchat_engine::{data, themes};
use zeroize::Zeroizing;

use crate::chat::{Chat, Pal};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Share {
    /// Works once.
    Once,
    /// Reusable until renewed.
    Public,
    /// Works once, from a new identity of its own (ADR-014).
    Incognito,
}

pub enum Dialog {
    Notice(String),
    Add {
        invite: String,
        incognito: bool,
        error: String,
    },
    Invite {
        text: String,
        kind: Share,
    },
    /// The nickname for a new incognito identity.
    Incognito {
        nickname: String,
        error: String,
    },
    Verify,
    Rename(String),
    Delete,
    /// The exact text that would be disclosed to the relay operator.
    Report(String),
    Settings,
    Backup {
        path: String,
        password: Zeroizing<String>,
        again: Zeroizing<String>,
        error: String,
    },
    /// A document to send from a path (ADR-017).
    SendFile {
        path: String,
        error: String,
    },
    /// Message `m` of the open chat, a received document.
    SaveFile {
        m: usize,
        folder: String,
        error: String,
    },
    Password(Passwords),
    Duress(Passwords),
    /// Bridge lines to paste; they replace the current ones (ADR-015).
    Bridges {
        text: Zeroizing<String>,
        error: String,
    },
    Wipe {
        typed: String,
    },
    /// The wipe is waiting for the relay (ADR-009).
    Leaving,
    /// The relay cannot be told; the reason.
    WipeAnyway(String),
    News,
}

/// The current password, a new one and its repeat.
#[derive(Default)]
pub struct Passwords {
    current: Zeroizing<String>,
    new: Zeroizing<String>,
    again: Zeroizing<String>,
    error: String,
}

impl Dialog {
    pub fn add() -> Self {
        Self::Add {
            invite: String::new(),
            incognito: false,
            error: String::new(),
        }
    }

    pub fn save_file(m: usize) -> Self {
        Self::SaveFile {
            m,
            folder: "~/Downloads".into(),
            error: String::new(),
        }
    }

    pub fn backup() -> Self {
        Self::Backup {
            path: String::new(),
            password: Zeroizing::default(),
            again: Zeroizing::default(),
            error: String::new(),
        }
    }
}

impl Chat {
    /// Shows the open dialog, if any. Esc or a click outside closes it.
    pub fn dialogs(&mut self, ctx: &egui::Context, p: &Pal, s: &mut Settings) {
        let Some(mut d) = self.dialog.take() else {
            return;
        };
        let mut keep = true;
        let r = egui::Modal::new(egui::Id::unique("dialog")).show(ctx, |ui| {
            ui.set_width(480.0);
            keep = self.body(ui, p, s, &mut d);
        });
        // A body may have opened the next dialog in place of this one.
        if keep && !r.should_close() && self.dialog.is_none() {
            self.dialog = Some(d);
        }
    }

    /// One dialog's content. False: close it.
    fn body(&mut self, ui: &mut egui::Ui, p: &Pal, s: &mut Settings, d: &mut Dialog) -> bool {
        let open = self.open.and_then(|k| self.engine.index(&k));
        match d {
            Dialog::Notice(text) => {
                ui.label(text.as_str());
                !ui.button("OK").clicked()
            }
            Dialog::Add {
                invite,
                incognito,
                error,
            } => {
                ui.heading("Add a contact");
                ui.label("Paste the invite they sent you.");
                ui.add(
                    egui::TextEdit::multiline(invite)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace),
                );
                ui.checkbox(incognito, "Answer from a new incognito identity")
                    .on_hover_text("They will not see your main identity (ADR-014)");
                error_line(ui, p, error);
                let (go, stay) = buttons(ui, p, "Add");
                if go {
                    match self.engine.add(invite.trim(), *incognito) {
                        Ok(i) => {
                            let key = self.engine.data.contacts[i].key;
                            self.select(key);
                            return false;
                        }
                        Err(e) => *error = e,
                    }
                }
                stay
            }
            Dialog::Invite { text, kind } => {
                let (title, about) = match kind {
                    Share::Once => ("Your invite", "Works once: send it to one friend."),
                    Share::Public => (
                        "Your public invite",
                        "Reusable: anyone with it can ask to add you.",
                    ),
                    Share::Incognito => (
                        "Your incognito invite",
                        "Works once, from a new identity used only with them.",
                    ),
                };
                ui.set_width(640.0);
                ui.heading(title);
                hint(ui, p, about);
                ui.add_space(12.0);
                ui.horizontal_top(|ui| {
                    crate::synth::qr(ui, text, 200.0);
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        let width = ui.available_width();
                        egui::Frame::new()
                            .fill(p.bg)
                            .stroke(egui::Stroke::new(1.0, p.mauve.gamma_multiply(0.6)))
                            .corner_radius(4)
                            .inner_margin(12)
                            .show(ui, |ui| {
                                ui.set_width(width - 26.0);
                                let code = RichText::new(text.as_str())
                                    .monospace()
                                    .size(11.5)
                                    .color(p.accent);
                                ui.add(egui::Label::new(code).wrap());
                            });
                        ui.add_space(10.0);
                        if crate::synth::button(ui, p, "Copy invite", egui::vec2(width, 36.0))
                            .clicked()
                        {
                            ui.ctx().copy_text(text.clone());
                            self.status = "invite copied".into();
                        }
                        if *kind != Share::Incognito {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new("YOUR FINGERPRINT")
                                    .small()
                                    .strong()
                                    .color(p.dim),
                            );
                            ui.label(
                                RichText::new(fingerprint(&self.engine.me()))
                                    .monospace()
                                    .color(p.text),
                            );
                        }
                    });
                });
                hint(
                    ui,
                    p,
                    "Scan the code with termchat.app on a phone, or send the text.",
                );
                let mut keep = true;
                let mut next = None;
                match kind {
                    Share::Once => {
                        section(ui, p, "Other invites");
                        ui.columns(3, |cols| {
                            if wide(&mut cols[0], p, "Public invite") {
                                next = Some((self.engine.public_invite(false), Share::Public));
                            }
                            if wide(&mut cols[1], p, "Incognito invite") {
                                self.dialog = Some(Dialog::Incognito {
                                    nickname: String::new(),
                                    error: String::new(),
                                });
                            }
                            if wide(&mut cols[2], p, "Revoke unused invites") {
                                let n = self.engine.revoke_invites();
                                self.status = format!("{n} unused one-time invite(s) revoked");
                                keep = false;
                            }
                        });
                    }
                    Share::Public => {
                        section(ui, p, "Public invite");
                        if wide(ui, p, "Renew: the old public invite stops working") {
                            next = Some((self.engine.public_invite(true), Share::Public));
                            self.status = "new public invite: the old one no longer works".into();
                        }
                    }
                    Share::Incognito => {}
                }
                if let Some((t, k)) = next {
                    *text = t;
                    *kind = k;
                }
                ui.add_space(14.0);
                keep &= !ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Button::new("Close").min_size(egui::vec2(110.0, 32.0)))
                            .clicked()
                    })
                    .inner;
                keep
            }
            Dialog::Incognito { nickname, error } => {
                ui.heading("Incognito invite");
                ui.label("A new identity, used only with the person who takes this invite.");
                ui.label("Nickname they will see:");
                ui.text_edit_singleline(nickname);
                error_line(ui, p, error);
                let (go, stay) = buttons(ui, p, "Create");
                if go {
                    match self.engine.incognito_invite(nickname.trim()) {
                        Ok(text) => {
                            self.dialog = Some(Dialog::Invite {
                                text,
                                kind: Share::Incognito,
                            });
                            return false;
                        }
                        Err(e) => *error = e,
                    }
                }
                stay
            }
            Dialog::Verify => {
                let Some(i) = open else { return false };
                let c = &self.engine.data.contacts[i];
                let name = self.engine.name(c);
                let number = safety_number(&self.engine.me_for(c), &c.key);
                let groups: Vec<&str> = number.split(' ').collect();
                ui.heading("Safety number");
                ui.label(format!("Compare with {name} in person or by phone."));
                ui.label("Same numbers on both screens = nobody is in the middle.");
                ui.add_space(6.0);
                for row in groups.chunks(4) {
                    ui.label(RichText::new(row.join(" ")).monospace().strong().size(16.0));
                }
                ui.add_space(6.0);
                let verified = c.verified;
                match verified {
                    true => ui.label(RichText::new("VERIFIED").color(p.accent)),
                    false => ui.label(RichText::new("not verified").color(p.warn)),
                };
                let flip = match verified {
                    true => "Mark as not verified",
                    false => "They match: mark verified",
                };
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.button(flip).clicked() {
                        self.engine.edit(i, |c| c.verified = !c.verified);
                    }
                    keep = !ui.button("Close").clicked();
                });
                keep
            }
            Dialog::Rename(name) => {
                let Some(i) = open else { return false };
                ui.heading("Rename");
                ui.label("Only on this device; empty uses the name they chose.");
                ui.text_edit_singleline(name);
                let (go, stay) = buttons(ui, p, "Save");
                if go {
                    let name = name.trim();
                    let name = (!name.is_empty()).then(|| name.chars().take(64).collect());
                    self.engine.edit(i, |c| c.petname = name);
                    return false;
                }
                stay
            }
            Dialog::Delete => {
                let Some(i) = open else { return false };
                let c = &self.engine.data.contacts[i];
                let name = self.engine.name(c);
                let (title, what, verb) = match self.engine.is_request(&c.key) {
                    true => (
                        "Reject the request",
                        format!("Reject {name}? They cannot simply ask again."),
                        "Reject",
                    ),
                    false => (
                        "Delete the contact",
                        format!("Delete {name} and the whole chat? This cannot be undone."),
                        "Delete",
                    ),
                };
                ui.heading(title);
                ui.label(what);
                let (go, stay) = buttons(ui, p, verb);
                if go {
                    self.engine.delete(i);
                    self.open = None;
                    return false;
                }
                stay
            }
            Dialog::Report(text) => {
                let Some(i) = open else { return false };
                ui.heading("Report to the relay operator");
                ui.label(
                    RichText::new(
                        "The relay operator will READ the text below. Reports from 3 different \
                         people this key wrote to ban it on this relay. The contact gets blocked.",
                    )
                    .color(p.warn),
                );
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(clean_text(text)).monospace().color(p.dim));
                    });
                let (go, stay) = buttons(ui, p, "Report and block");
                if go {
                    self.engine.report(i, std::mem::take(text));
                    return false;
                }
                stay
            }
            Dialog::Settings => {
                ui.set_width(600.0);
                ui.heading("Settings");
                section(ui, p, "Appearance");
                egui::Grid::new("settings")
                    .num_columns(2)
                    .spacing([24.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Theme");
                        egui::ComboBox::from_id_salt("theme")
                            .width(220.0)
                            .selected_text(s.desktop_theme.as_str())
                            .show_ui(ui, |ui| {
                                for (name, _) in themes::THEMES {
                                    ui.selectable_value(
                                        &mut s.desktop_theme,
                                        (*name).into(),
                                        *name,
                                    );
                                }
                            });
                        ui.end_row();
                        ui.label("New messages");
                        ui.checkbox(&mut s.sound, "Play the system sound");
                        ui.end_row();
                    });
                section(ui, p, "Contacts");
                let mut codes_only = self.engine.data.invites.codes_only;
                if ui
                    .checkbox(
                        &mut codes_only,
                        "Accept contact requests only from my invites",
                    )
                    .changed()
                {
                    self.engine.set_codes_only(codes_only);
                }
                hint(
                    ui,
                    p,
                    "Requests without one of your invite codes are dropped (ADR-013).",
                );
                section(ui, p, "Account and security");
                let tor = match self.engine.data.tor {
                    true => "Turn Tor off",
                    false => "Turn Tor on",
                };
                let mut next = None;
                ui.columns(2, |cols| {
                    if wide(&mut cols[0], p, "Back up your identity") {
                        next = Some(Dialog::backup());
                    }
                    if wide(&mut cols[1], p, "Change password") {
                        next = Some(Dialog::Password(Passwords::default()));
                    }
                    if wide(&mut cols[0], p, "Set a duress password") {
                        next = Some(Dialog::Duress(Passwords::default()));
                    }
                    if wide(&mut cols[1], p, tor) {
                        next = Some(Dialog::Notice(self.engine.toggle_tor()));
                    }
                    if wide(&mut cols[0], p, "Tor bridges") {
                        next = Some(Dialog::Bridges {
                            text: Zeroizing::default(),
                            error: String::new(),
                        });
                    }
                    if wide(&mut cols[1], p, "What's new in this version") {
                        next = Some(Dialog::News);
                    }
                });
                section(ui, p, "Danger zone");
                let wipe = egui::Button::new(RichText::new("Wipe everything").color(p.danger))
                    .stroke(egui::Stroke::new(1.0, p.danger))
                    .fill(p.surface)
                    .corner_radius(4);
                if ui.add_sized([ui.available_width(), 36.0], wipe).clicked() {
                    next = Some(Dialog::Wipe {
                        typed: String::new(),
                    });
                }
                hint(
                    ui,
                    p,
                    "Erases keys, contacts and history from this device. No undo.",
                );
                ui.add_space(14.0);
                let close = ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Button::new("Close").min_size(egui::vec2(110.0, 32.0)))
                            .clicked()
                    })
                    .inner;
                if next.is_some() {
                    self.dialog = next;
                }
                !close
            }
            Dialog::SendFile { path, error } => {
                let Some(i) = open else { return false };
                ui.heading("Send a file");
                ui.label(
                    "Up to 1 MiB, end-to-end encrypted like a message. 200 KB takes about \
                     10 s, 1 MiB about 3 min. The relay sees roughly how big it is.",
                );
                ui.add(
                    egui::TextEdit::singleline(path)
                        .hint_text("~/Documents/contract.pdf")
                        .desired_width(f32::INFINITY),
                );
                error_line(ui, p, error);
                let (go, stay) = buttons(ui, p, "Send");
                if go {
                    let sent = data::read_document(&data::expand(path))
                        .and_then(|(name, bytes)| self.engine.send_file(i, &name, bytes));
                    match sent {
                        Ok(()) => return false,
                        Err(e) => *error = e,
                    }
                }
                stay
            }
            Dialog::SaveFile { m, folder, error } => {
                let Some(i) = open else { return false };
                let Some((name, _)) = self.engine.file_bytes(i, *m) else {
                    return false;
                };
                ui.heading("Save a file");
                ui.label(RichText::new(clean_text(&name)).monospace());
                ui.label(
                    RichText::new(
                        "Files from contacts can be harmful: termchat.app never opens them, \
                         check before you do. An existing file is never overwritten.",
                    )
                    .color(p.warn),
                );
                ui.horizontal(|ui| {
                    ui.label("Into folder");
                    ui.text_edit_singleline(folder);
                });
                error_line(ui, p, error);
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        match self.engine.save_file(i, *m, &data::expand(folder)) {
                            Ok(path) => {
                                self.status = format!("saved to {}", path.display());
                                keep = false;
                            }
                            Err(e) => *error = e,
                        }
                    }
                    if ui.button("Discard").clicked() {
                        self.engine.discard_file(i, *m);
                        keep = false;
                    }
                    keep &= !ui.button("Later").clicked();
                });
                keep
            }
            Dialog::Password(pw) => {
                ui.heading("Change password");
                ui.label(
                    "The data stays as it is; only the key slot changes. If you set a \
                     duress password, keep the new one far from it: a typo that hits the \
                     duress password erases everything.",
                );
                let (go, stay) = password_fields(ui, p, pw, "New password");
                if !go {
                    return stay;
                }
                if let Some(new) = pw.matching() {
                    match self.engine.change_password(&pw.current, &new) {
                        Ok(()) => {
                            self.dialog = Some(Dialog::Notice("Password changed.".into()));
                            return false;
                        }
                        Err(e) => pw.error = e,
                    }
                }
                true
            }
            Dialog::Duress(pw) => {
                ui.heading("Duress password");
                ui.label(
                    RichText::new(
                        "Entering the duress password at unlock silently erases everything \
                         and shows the first-run screen. It does NOT help if someone copied \
                         the disk first. The code is public, so they may know the feature. \
                         Destroying data after seizure can be a crime where you live.",
                    )
                    .color(p.warn),
                );
                let (go, stay) = password_fields(ui, p, pw, "Duress password (empty: off)");
                if !go {
                    return stay;
                }
                if let Some(new) = pw.matching() {
                    let duress = (!new.is_empty()).then_some(new.as_str());
                    match self.engine.set_duress(&pw.current, duress) {
                        Ok(done) => {
                            self.dialog = Some(Dialog::Notice(done));
                            return false;
                        }
                        Err(e) => pw.error = e,
                    }
                }
                true
            }
            Dialog::Bridges { text, error } => {
                ui.heading("Tor bridges");
                ui.label(
                    "For networks that block Tor. Get bridges at bridges.torproject.org or \
                     from @GetBridgesBot in Telegram, and paste them, one per line.",
                );
                ui.label(RichText::new("In use now:").color(p.dim));
                let now = self.engine.bridges();
                if now.is_empty() {
                    ui.label(RichText::new("  none: Tor connects directly").color(p.dim));
                }
                for b in now {
                    ui.label(
                        RichText::new(format!("  {}", clean_text(&b)))
                            .monospace()
                            .color(p.dim),
                    );
                }
                ui.add(
                    egui::TextEdit::multiline(&mut **text)
                        .hint_text("obfs4 192.0.2.1:443 …")
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace),
                );
                error_line(ui, p, error);
                let mut keep = true;
                let mut lines = None;
                ui.horizontal(|ui| {
                    if ui.button("Use these").clicked() {
                        lines = Some(text.as_str().to_owned());
                    }
                    if ui.button("No bridges").clicked() {
                        lines = Some(String::new());
                    }
                    keep = !ui.button("Cancel").clicked();
                });
                if let Some(lines) = lines {
                    match self.engine.set_bridges(&lines) {
                        Ok(done) => {
                            self.dialog = Some(Dialog::Notice(done));
                            return false;
                        }
                        Err(e) => *error = e,
                    }
                }
                keep
            }
            Dialog::Wipe { typed } => {
                ui.heading("Wipe everything");
                ui.label(
                    "Erases keys, contacts and history from this device. No undo.\n\
                     WIPE deletes the account: contacts are told you left and the relay \
                     forgets your key.\nMOVED (restored on another device): erases only \
                     this device and tells nobody.",
                );
                ui.horizontal(|ui| {
                    ui.label("Type WIPE or MOVED");
                    ui.text_edit_singleline(typed);
                });
                let (go, stay) = buttons(ui, p, "Wipe");
                if go {
                    match typed.trim() {
                        "WIPE" => {
                            if self.engine.leave(false) {
                                self.dialog = Some(Dialog::Leaving);
                            }
                            return false;
                        }
                        "MOVED" => {
                            self.engine.leave(true);
                            return false;
                        }
                        _ => {}
                    }
                }
                stay
            }
            Dialog::Leaving => {
                ui.heading("Wipe everything");
                ui.label("Telling your contacts and the relay…");
                ui.spinner();
                true
            }
            Dialog::WipeAnyway(why) => {
                ui.heading("Wipe everything");
                ui.label(format!("{why}\nWipe without telling your contacts?"));
                let (go, stay) = buttons(ui, p, "Wipe anyway");
                if go {
                    self.engine.finish_wipe("");
                    return false;
                }
                stay
            }
            Dialog::News => {
                self.engine.news_seen();
                ui.heading("What's new");
                if let Some(v) = &self.engine.update {
                    ui.label(
                        RichText::new(format!(
                            "termchat.app {v} is out. Update the way you installed it: the \
                             installer, Homebrew, or the release page on GitHub."
                        ))
                        .color(p.accent),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        markdown(ui, p, news());
                    });
                !ui.button("Close").clicked()
            }
            Dialog::Backup {
                path,
                password,
                again,
                error,
            } => {
                ui.heading("Back up your identity");
                ui.label(
                    "Your keys and contacts, without messages, in one file with its own \
                     password. Without a backup, a lost device means a lost identity.",
                );
                egui::Grid::new("backup").num_columns(2).show(ui, |ui| {
                    ui.label("Save to");
                    ui.add(
                        egui::TextEdit::singleline(path).hint_text("/Volumes/USB/termchat-backup"),
                    );
                    ui.end_row();
                    ui.label("Backup password");
                    ui.add(egui::TextEdit::singleline(&mut **password).password(true));
                    ui.end_row();
                    ui.label("Again");
                    ui.add(egui::TextEdit::singleline(&mut **again).password(true));
                    ui.end_row();
                });
                error_line(ui, p, error);
                let (go, stay) = buttons(ui, p, "Write backup");
                if !go {
                    return stay;
                }
                if path.trim().is_empty() {
                    *error = "where should it go? A USB stick, for example".into();
                } else if **password != **again {
                    *error = "passwords do not match".into();
                } else {
                    let to = data::expand(path);
                    match self.engine.backup(&to, password) {
                        Ok(()) => {
                            self.dialog = Some(Dialog::Notice(format!(
                                "Backup written to {}.\nMove it off this device: a wipe erases \
                                 it only while it stays at this path.",
                                to.display()
                            )));
                            return false;
                        }
                        Err(e) => *error = e,
                    }
                }
                true
            }
        }
    }
}

impl Passwords {
    /// The new password, once both fields agree.
    fn matching(&mut self) -> Option<Zeroizing<String>> {
        match self.new == self.again {
            true => Some(self.new.clone()),
            false => {
                self.error = "passwords do not match".into();
                None
            }
        }
    }
}

/// Current, new, repeat, the error and the buttons: (Save clicked, keep open).
fn password_fields(ui: &mut egui::Ui, p: &Pal, pw: &mut Passwords, new: &str) -> (bool, bool) {
    egui::Grid::new("passwords").num_columns(2).show(ui, |ui| {
        ui.label("Current password");
        ui.add(egui::TextEdit::singleline(&mut *pw.current).password(true));
        ui.end_row();
        ui.label(new);
        ui.add(egui::TextEdit::singleline(&mut *pw.new).password(true));
        ui.end_row();
        ui.label("Again");
        ui.add(egui::TextEdit::singleline(&mut *pw.again).password(true));
        ui.end_row();
    });
    error_line(ui, p, &pw.error);
    buttons(ui, p, "Save")
}

/// A section title in small capitals, like the Android app, and a rule.
fn section(ui: &mut egui::Ui, p: &Pal, title: &str) {
    ui.add_space(16.0);
    ui.label(
        RichText::new(title.to_uppercase())
            .small()
            .strong()
            .color(p.dim),
    );
    ui.separator();
}

fn hint(ui: &mut egui::Ui, p: &Pal, text: &str) {
    ui.label(RichText::new(text).small().color(p.dim));
}

/// A full-width button. True when clicked.
fn wide(ui: &mut egui::Ui, p: &Pal, text: &str) -> bool {
    let button = egui::Button::new(RichText::new(text).size(14.0))
        .fill(p.theirs)
        .stroke(egui::Stroke::new(1.0, p.line))
        .corner_radius(4);
    ui.add_sized([ui.available_width(), 36.0], button).clicked()
}

/// The main button and Cancel. Returns (main clicked, keep open).
fn buttons(ui: &mut egui::Ui, p: &Pal, main: &str) -> (bool, bool) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let width = 24.0 + 8.0 * main.chars().count() as f32;
        let go = crate::synth::button(ui, p, main, egui::vec2(width.max(110.0), 32.0)).clicked();
        let cancel = ui
            .add(egui::Button::new("Cancel").min_size(egui::vec2(100.0, 32.0)))
            .clicked();
        (go, !cancel)
    })
    .inner
}

fn error_line(ui: &mut egui::Ui, p: &Pal, error: &str) {
    if !error.is_empty() {
        ui.label(RichText::new(error).color(p.danger));
    }
}

/// Just enough Markdown for the changelog (as in the terminal client):
/// headings, bullets, **bold**, `code`, [links](as their text).
// No tables or code blocks; add them when the changelog uses them.
fn markdown(ui: &mut egui::Ui, p: &Pal, src: &str) {
    for para in src.split("\n\n") {
        let mut para = para.trim().to_string();
        if let Some(h) = para.strip_prefix("### ").or(para.strip_prefix("## ")) {
            ui.add_space(6.0);
            ui.label(RichText::new(h).heading().color(p.accent));
            continue;
        }
        while let Some(mid) = para.find("](") {
            let (Some(open), Some(close)) = (para[..mid].rfind('['), para[mid..].find(')')) else {
                break;
            };
            let label = para[open + 1..mid].to_string();
            para.replace_range(open..mid + close + 1, &label);
        }
        // A paragraph is a list when its first line is a bullet; wrapped
        // lines join their item.
        let mut items: Vec<String> = Vec::new();
        for line in para.lines() {
            match line.strip_prefix("- ") {
                Some(item) => items.push(format!("• {item}")),
                None => match items.last_mut() {
                    Some(last) => {
                        last.push(' ');
                        last.push_str(line.trim());
                    }
                    None => items.push(line.trim().to_string()),
                },
            }
        }
        for item in items {
            let mut job = egui::text::LayoutJob::default();
            let font = egui::FontId::proportional(14.0);
            for (i, code) in item.split('`').enumerate() {
                for (j, part) in code.split("**").enumerate() {
                    let color = if i % 2 == 1 {
                        p.accent
                    } else if j % 2 == 1 {
                        ui.visuals().strong_text_color()
                    } else {
                        ui.visuals().text_color()
                    };
                    let font = if i % 2 == 1 {
                        egui::FontId::monospace(13.0)
                    } else {
                        font.clone()
                    };
                    job.append(part, 0.0, egui::TextFormat::simple(font, color));
                }
            }
            ui.label(job);
            ui.add_space(3.0);
        }
    }
}
