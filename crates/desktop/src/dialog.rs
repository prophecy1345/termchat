//! Windows over the chat: contacts, invites, the safety number, settings,
//! backup (ADR-018). The engine does the work; these only ask and show.

use eframe::egui::{self, RichText};
use termchat_core::{fingerprint, safety_number};
use termchat_engine::engine::clean_text;
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
}

impl Dialog {
    pub fn add() -> Self {
        Self::Add {
            invite: String::new(),
            incognito: false,
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
                let (go, stay) = buttons(ui, "Add");
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
                ui.heading(title);
                ui.label(about);
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(text.as_str()).monospace()).wrap());
                if *kind != Share::Incognito {
                    ui.label(
                        RichText::new(format!(
                            "Your fingerprint  {}",
                            fingerprint(&self.engine.me())
                        ))
                        .color(p.dim),
                    );
                }
                ui.add_space(6.0);
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(text.clone());
                        self.status = "invite copied".into();
                    }
                    match kind {
                        Share::Once => {
                            if ui.button("Public invite").clicked() {
                                *text = self.engine.public_invite(false);
                                *kind = Share::Public;
                            }
                            if ui.button("Incognito…").clicked() {
                                self.dialog = Some(Dialog::Incognito {
                                    nickname: String::new(),
                                    error: String::new(),
                                });
                            }
                            if ui.button("Revoke unused").clicked() {
                                let n = self.engine.revoke_invites();
                                self.status = format!("{n} unused one-time invite(s) revoked");
                                keep = false;
                            }
                        }
                        Share::Public => {
                            if ui
                                .button("Renew")
                                .on_hover_text("The old one stops working")
                                .clicked()
                            {
                                *text = self.engine.public_invite(true);
                                self.status =
                                    "new public invite: the old one no longer works".into();
                            }
                        }
                        Share::Incognito => {}
                    }
                    keep &= !ui.button("Close").clicked();
                });
                keep
            }
            Dialog::Incognito { nickname, error } => {
                ui.heading("Incognito invite");
                ui.label("A new identity, used only with the person who takes this invite.");
                ui.label("Nickname they will see:");
                ui.text_edit_singleline(nickname);
                error_line(ui, p, error);
                let (go, stay) = buttons(ui, "Create");
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
                let (go, stay) = buttons(ui, "Save");
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
                let (go, stay) = buttons(ui, verb);
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
                let (go, stay) = buttons(ui, "Report and block");
                if go {
                    self.engine.report(i, std::mem::take(text));
                    return false;
                }
                stay
            }
            Dialog::Settings => {
                ui.heading("Settings");
                egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
                    ui.label("Theme");
                    egui::ComboBox::from_id_salt("theme")
                        .selected_text(s.desktop_theme.as_str())
                        .show_ui(ui, |ui| {
                            for (name, _) in themes::THEMES {
                                ui.selectable_value(&mut s.desktop_theme, (*name).into(), *name);
                            }
                        });
                    ui.end_row();
                    ui.label("Sound for new messages");
                    ui.checkbox(&mut s.sound, "");
                    ui.end_row();
                });
                ui.add_space(6.0);
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.button("Back up…").clicked() {
                        self.dialog = Some(Dialog::backup());
                    }
                    keep = !ui.button("Close").clicked();
                });
                keep
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
                let (go, stay) = buttons(ui, "Write backup");
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

/// The main button and Cancel. Returns (main clicked, keep open).
fn buttons(ui: &mut egui::Ui, main: &str) -> (bool, bool) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let go = ui.button(main).clicked();
        let cancel = ui.button("Cancel").clicked();
        (go, !cancel)
    })
    .inner
}

fn error_line(ui: &mut egui::Ui, p: &Pal, error: &str) {
    if !error.is_empty() {
        ui.label(RichText::new(error).color(p.danger));
    }
}
