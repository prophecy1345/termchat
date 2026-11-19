//! Windows over the chat: contacts, invites, the safety number, settings,
//! backup (ADR-018). The engine does the work; these only ask and show.

use eframe::egui::{self, RichText};
use termchat_core::{fingerprint, safety_number};
use termchat_engine::engine::{clean_text, news};
use termchat_engine::settings::{LOCK_CHOICES, Settings};
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

/// Shown after "The same" until the other screen agrees too.
pub(crate) const LINK_CONFIRMED: &str = "confirmed: linking finishes when the other screen agrees";

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
    /// Delete our message `m` of the open chat for both sides (ADR-025)?
    DeleteMessage(usize),
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
    /// ADR-027: a new group's name.
    NewGroup {
        name: String,
        error: String,
    },
    /// The open group's members.
    Members,
    /// Contacts the admin can invite to the open group; why the last failed.
    InviteToGroup(String),
    /// Remove this member from the open group?
    RemoveMember([u8; 32]),
    /// Leave the open group?
    LeaveGroup,
    /// ADR-028: this device's link code, for the main device.
    LinkCode(String),
    /// Our devices.
    Devices,
    /// A new device's link code being pasted.
    AddDevice {
        code: String,
        error: String,
    },
    /// The fingerprint to compare on both screens, for own device `i`.
    Link(usize),
    /// Remove this device?
    RemoveDevice([u8; 32]),
    /// A device's new label, seen only by our own devices.
    RenameDevice {
        key: [u8; 32],
        label: String,
        error: String,
    },
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
                match next {
                    Some((Ok(t), k)) => {
                        *text = t;
                        *kind = k;
                    }
                    Some((Err(e), _)) => self.status = e,
                    None => {}
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
                    // The invite opens once its queue exists (ADR-023).
                    match self.engine.incognito_invite(nickname.trim()) {
                        Ok(()) => {
                            self.status =
                                "making a new identity; its invite opens in a moment".into();
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
            Dialog::DeleteMessage(m) if self.open_group().is_some() => {
                let Some(g) = self.open_group() else {
                    return false;
                };
                ui.heading("Delete the message");
                ui.label("Delete it for everyone in the group? Their apps remove it; a copy or screenshot someone made stays.");
                let (go, stay) = buttons(ui, p, "Delete");
                if go {
                    if let Err(e) = self.engine.delete_group_message(g, *m) {
                        self.status = e;
                    }
                    return false;
                }
                stay
            }
            Dialog::NewGroup { name, error } => {
                ui.set_width(560.0);
                ui.heading("New group");
                ui.add_space(8.0);
                ui.label("A small private group, by invitation only, up to 32 people.");
                ui.label("You are its admin: only you add and remove members.");
                ui.add_space(14.0);
                ui.label(RichText::new("NAME").small().strong().color(p.dim));
                ui.add_space(4.0);
                let field = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text("Weekend trip")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(10, 8))
                        .font(egui::TextStyle::Body),
                );
                if !field.has_focus() && name.is_empty() {
                    field.request_focus();
                }
                ui.add_space(4.0);
                hint(
                    ui,
                    p,
                    "Seen only by its members. Nobody can find the group.",
                );
                error_line(ui, p, error);
                ui.add_space(6.0);
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let (go, stay) = buttons(ui, p, "Create");
                if go || enter {
                    match self.engine.create_group(name.trim()) {
                        Ok(g) => {
                            self.select_group(g);
                            return false;
                        }
                        Err(e) => *error = e,
                    }
                }
                stay
            }
            Dialog::Members => {
                let Some(g) = self.open_group() else {
                    return false;
                };
                let group = &self.engine.data.groups[g];
                let me = self.engine.me();
                let admin_key = group.roster.as_ref().map(|r| r.admin);
                ui.set_width(560.0);
                ui.heading(format!(
                    "{} · members",
                    termchat_engine::clean(group.name())
                ));
                ui.add_space(6.0);
                ui.label("The admin added them. Compare safety numbers in person to be sure.");
                ui.add_space(12.0);
                let mut remove = None;
                for m in group.members() {
                    row(ui, p, |ui| {
                        avatar_letter(ui, p, &m.nickname);
                        ui.vertical(|ui| {
                            let mut name = termchat_engine::clean(&m.nickname);
                            if m.ed25519 == me {
                                name.push_str(" (you)");
                            }
                            ui.label(RichText::new(name).monospace());
                            let fp = fingerprint(&m.ed25519);
                            let mut under = fp[..fp.len().min(11)].to_string();
                            if admin_key == Some(m.ed25519) {
                                under.push_str(" · ADMIN");
                            }
                            ui.label(RichText::new(under).monospace().small().color(p.dim));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if group.is_admin() && m.ed25519 != me && ui.button("Remove…").clicked()
                            {
                                remove = Some(m.ed25519);
                            }
                        });
                    });
                }
                ui.add_space(12.0);
                let mut invite = false;
                let close = ui
                    .horizontal(|ui| {
                        if group.is_admin() {
                            invite = crate::synth::button(
                                ui,
                                p,
                                "Invite a contact",
                                egui::vec2(170.0, 32.0),
                            )
                            .clicked();
                        }
                        ui.add(egui::Button::new("Close").min_size(egui::vec2(100.0, 32.0)))
                            .clicked()
                    })
                    .inner;
                if let Some(k) = remove {
                    self.dialog = Some(Dialog::RemoveMember(k));
                    return false;
                }
                if invite {
                    self.dialog = Some(Dialog::InviteToGroup(String::new()));
                    return false;
                }
                !close
            }
            Dialog::InviteToGroup(error) => {
                let Some(g) = self.open_group() else {
                    return false;
                };
                ui.set_width(560.0);
                ui.heading("Invite to the group");
                ui.add_space(6.0);
                ui.label("Your own contacts only. They join from their chat with you.");
                ui.add_space(12.0);
                let rows = self.engine.invitable(g);
                if rows.is_empty() {
                    ui.label(RichText::new("No contact to invite.").color(p.dim));
                }
                let group = &self.engine.data.groups[g];
                let mut pick = None;
                for i in rows {
                    let c = &self.engine.data.contacts[i];
                    let invited = group.asking.contains(&c.key)
                        || group.invited.iter().any(|(k, _)| *k == c.key);
                    let name = self.engine.name(c);
                    row(ui, p, |ui| {
                        avatar_letter(ui, p, &name);
                        ui.label(RichText::new(&name).monospace());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = match invited {
                                true => RichText::new("✓ Invited").color(p.accent),
                                false => RichText::new("Invite"),
                            };
                            let b = egui::Button::new(label).min_size(egui::vec2(100.0, 28.0));
                            if ui.add_enabled(!invited, b).clicked() {
                                pick = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = pick {
                    *error = self.engine.invite_to_group(g, i).err().unwrap_or_default();
                }
                error_line(ui, p, error);
                ui.add_space(12.0);
                !ui.add(egui::Button::new("Close").min_size(egui::vec2(100.0, 32.0)))
                    .clicked()
            }
            Dialog::RemoveMember(who) => {
                let Some(g) = self.open_group() else {
                    return false;
                };
                let name = self.engine.data.groups[g]
                    .name_of(who)
                    .map(termchat_engine::clean)
                    .unwrap_or_default();
                ui.set_width(560.0);
                ui.heading("Remove from the group");
                ui.add_space(6.0);
                ui.label(format!("Remove {name} from the group?"));
                ui.label("The group moves to a new queue: they read nothing from now on.");
                let (go, stay) = buttons(ui, p, "Remove");
                if go {
                    if let Err(e) = self.engine.remove_from_group(g, who) {
                        self.status = e;
                    }
                    return false;
                }
                stay
            }
            Dialog::LinkCode(code) => {
                ui.set_width(640.0);
                ui.heading("Link to my main device");
                hint(
                    ui,
                    p,
                    "On your main device: Settings, Devices, Add a device. Scan or paste this code, then compare the fingerprint both screens show.",
                );
                ui.add_space(12.0);
                ui.horizontal_top(|ui| {
                    crate::synth::qr(ui, code, 200.0);
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        let width = ui.available_width();
                        let text = RichText::new(code.as_str())
                            .monospace()
                            .size(11.5)
                            .color(p.accent);
                        ui.add(egui::Label::new(text).wrap());
                        ui.add_space(10.0);
                        if crate::synth::button(ui, p, "Copy code", egui::vec2(width, 36.0))
                            .clicked()
                        {
                            ui.ctx().copy_text(code.clone());
                            self.status = "link code copied".into();
                        }
                    });
                });
                ui.add_space(12.0);
                !ui.add(egui::Button::new("Close").min_size(egui::vec2(100.0, 32.0)))
                    .clicked()
            }
            Dialog::Devices => {
                ui.set_width(560.0);
                ui.heading("Devices");
                ui.add_space(6.0);
                ui.label("Up to four, this one included. Contacts write to each of them.");
                let main = self.engine.is_primary();
                if !main {
                    ui.label("Your main device adds, renames and removes them.");
                }
                ui.add_space(12.0);
                let me = self.engine.me();
                let mut remove = None;
                let mut rename = None;
                for (n, (k, label)) in self.engine.device_rows().into_iter().enumerate() {
                    let old = label.clone();
                    row(ui, p, |ui| {
                        ui.vertical(|ui| {
                            let mut name = match label.is_empty() {
                                true => "device".to_string(),
                                false => label,
                            };
                            if k == me {
                                name.push_str(" (this one)");
                            }
                            ui.label(RichText::new(name).monospace());
                            let fp = fingerprint(&k);
                            let mut under = fp[..fp.len().min(11)].to_string();
                            if n == 0 {
                                under.push_str(" · MAIN");
                            }
                            ui.label(RichText::new(under).monospace().small().color(p.dim));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if main && k != me && ui.button("Remove…").clicked() {
                                remove = Some(k);
                            }
                            if main && ui.button("Rename…").clicked() {
                                rename = Some((k, old));
                            }
                        });
                    });
                }
                ui.add_space(12.0);
                let mut add = false;
                let close = ui
                    .horizontal(|ui| {
                        add = main
                            && crate::synth::button(ui, p, "Add a device", egui::vec2(170.0, 32.0))
                                .clicked();
                        ui.add(egui::Button::new("Close").min_size(egui::vec2(100.0, 32.0)))
                            .clicked()
                    })
                    .inner;
                if let Some(k) = remove {
                    self.dialog = Some(Dialog::RemoveDevice(k));
                    return false;
                }
                if let Some((key, label)) = rename {
                    self.dialog = Some(Dialog::RenameDevice {
                        key,
                        label,
                        error: String::new(),
                    });
                    return false;
                }
                if add {
                    self.dialog = Some(Dialog::AddDevice {
                        code: String::new(),
                        error: String::new(),
                    });
                    return false;
                }
                !close
            }
            Dialog::AddDevice { code, error } => {
                ui.heading("Add a device");
                ui.label(
                    "On the new device: Settings, Link to my main device. Paste its code here.",
                );
                ui.add(
                    egui::TextEdit::multiline(code)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace),
                );
                error_line(ui, p, error);
                let (go, stay) = buttons(ui, p, "Add");
                if go {
                    match self.engine.add_device(code.trim()) {
                        Ok(i) => {
                            self.dialog = Some(Dialog::Link(i));
                            return false;
                        }
                        Err(e) => *error = e,
                    }
                }
                stay
            }
            Dialog::Link(i) if self.engine.link_waiting(*i) => {
                ui.set_width(560.0);
                ui.heading("Link a device");
                ui.add_space(6.0);
                ui.label("Reaching the new device… Its screen shows the fingerprint in a moment (through Tor this takes up to half a minute). Then compare.");
                ui.add_space(10.0);
                ui.spinner();
                if ui.button("Cancel").clicked() {
                    self.engine.delete(*i);
                    self.status = "linking cancelled".into();
                    return false;
                }
                true
            }
            Dialog::Link(i) => {
                ui.set_width(560.0);
                ui.heading("Link a device");
                ui.add_space(6.0);
                ui.label("Both screens show this fingerprint. Go on only if they are the same: someone who copied the link code would show another one.");
                ui.add_space(10.0);
                ui.label(
                    RichText::new(self.engine.link_fingerprint())
                        .monospace()
                        .size(18.0)
                        .color(p.accent),
                );
                let (go, stay) = buttons(ui, p, "The same");
                if go {
                    self.engine.confirm_link(*i);
                    self.status = LINK_CONFIRMED.into();
                    return false;
                }
                if !stay {
                    self.engine.delete(*i);
                    self.status = "linking cancelled".into();
                }
                stay
            }
            Dialog::RenameDevice { key, label, error } => {
                ui.set_width(480.0);
                ui.heading("Rename a device");
                ui.label("A label like \"phone\" or \"laptop\", seen only on your own devices.");
                ui.add(egui::TextEdit::singleline(label).desired_width(f32::INFINITY));
                error_line(ui, p, error);
                let (go, stay) = buttons(ui, p, "Save");
                if go {
                    match self.engine.set_device_label(key, label) {
                        Ok(()) => {
                            self.dialog = Some(Dialog::Devices);
                            return false;
                        }
                        Err(e) => *error = e,
                    }
                }
                stay
            }
            Dialog::RemoveDevice(key) => {
                ui.set_width(560.0);
                ui.heading("Remove a device");
                ui.add_space(6.0);
                ui.label("It keeps what it has, and reads and sends nothing more.");
                let (go, stay) = buttons(ui, p, "Remove");
                if go {
                    self.status = match self.engine.remove_device(key) {
                        Ok(()) => "device removed".into(),
                        Err(e) => e,
                    };
                    return false;
                }
                stay
            }
            Dialog::LeaveGroup => {
                let Some(g) = self.open_group() else {
                    return false;
                };
                ui.set_width(560.0);
                ui.heading("Leave the group");
                ui.add_space(6.0);
                ui.label("The others are told. The chat stays here, read-only.");
                let (go, stay) = buttons(ui, p, "Leave");
                if go {
                    if let Err(e) = self.engine.leave_group(g) {
                        self.status = e;
                    }
                    return false;
                }
                stay
            }
            Dialog::DeleteMessage(m) => {
                let Some(i) = open else { return false };
                ui.heading("Delete the message");
                ui.label("Delete it for both of you? Their app removes it; a copy or screenshot they made stays.");
                let (go, stay) = buttons(ui, p, "Delete");
                if go {
                    if let Err(e) = self.engine.delete_message(i, *m) {
                        self.status = e;
                    }
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
                        "The relay operator will READ the text below. The relay does not know who \
                         wrote it, so it cannot ban anyone. The contact gets blocked.",
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
                        ui.label("Lock after");
                        egui::ComboBox::from_id_salt("lock")
                            .width(220.0)
                            .selected_text(format!("{} min idle", s.lock_minutes.clamp(1, 30)))
                            .show_ui(ui, |ui| {
                                for m in LOCK_CHOICES {
                                    ui.selectable_value(
                                        &mut s.lock_minutes,
                                        m,
                                        format!("{m} min idle"),
                                    );
                                }
                            });
                        ui.end_row();
                    });
                hint(
                    ui,
                    p,
                    "A longer time gives whoever finds this computer unlocked more time to read.",
                );
                section(ui, p, "Contacts");
                let mut shared = self.engine.data.shared_link;
                if ui
                    .checkbox(&mut shared, "One connection for all contacts")
                    .changed()
                {
                    self.status = match self.engine.set_shared_link(shared) {
                        Ok(done) | Err(done) => done,
                    };
                }
                hint(
                    ui,
                    p,
                    "Uses less battery and network, but the relay then sees which queues are yours. \
                     Off: a connection per contact (ADR-023).",
                );
                section(ui, p, "Devices");
                let e = &self.engine;
                let linkable = e.is_primary() && e.data.devices.is_none() && e.data.people() == 0;
                let mut device = None;
                ui.columns(2, |cols| {
                    if (e.is_primary() || e.data.devices.is_some())
                        && wide(&mut cols[0], p, "Devices…")
                    {
                        device = Some(Dialog::Devices);
                    }
                    if linkable && wide(&mut cols[1], p, "Link to my main device") {
                        device = Some(Dialog::LinkCode(String::new()));
                    }
                });
                if !e.is_primary() {
                    hint(
                        ui,
                        p,
                        "This device is linked to your main one, which adds and removes devices and creates groups.",
                    );
                }
                if let Some(d) = device {
                    self.dialog = Some(match d {
                        Dialog::LinkCode(_) => match self.engine.link_code() {
                            Ok(code) => Dialog::LinkCode(code),
                            Err(e) => Dialog::Notice(e),
                        },
                        d => d,
                    });
                    return false;
                }
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
                // A chat, or a group (ADR-027 amendment).
                let group = self.open_group();
                if open.is_none() && group.is_none() {
                    return false;
                }
                ui.heading("Send a file");
                ui.label(
                    "Up to 1 MiB, end-to-end encrypted like a message. 200 KB takes about \
                     10 s, 1 MiB about 3 min. The relay sees roughly how big it is.",
                );
                ui.horizontal(|ui| {
                    // The system's own Open dialog; the field stays for the keyboard.
                    if ui.button("Choose…").clicked()
                        && let Some(file) = rfd::FileDialog::new().pick_file()
                    {
                        *path = file.display().to_string();
                    }
                    ui.add(
                        egui::TextEdit::singleline(path)
                            .hint_text("~/Documents/contract.pdf, or drop a file on the window")
                            .desired_width(f32::INFINITY),
                    );
                });
                error_line(ui, p, error);
                let (go, stay) = buttons(ui, p, "Send");
                if go {
                    let sent =
                        data::read_document(&data::expand(path)).and_then(|(name, bytes)| {
                            match (group, open) {
                                (Some(g), _) => self.engine.send_group_file(g, &name, bytes),
                                (None, Some(i)) => self.engine.send_file(i, &name, bytes),
                                (None, None) => Err("nothing open".into()),
                            }
                        });
                    match sent {
                        Ok(()) => return false,
                        Err(e) => *error = e,
                    }
                }
                stay
            }
            Dialog::SaveFile { m, folder, error } => {
                let group = self.open_group();
                let bytes = match (group, open) {
                    (Some(g), _) => self.engine.group_file_bytes(g, *m),
                    (None, Some(i)) => self.engine.file_bytes(i, *m),
                    (None, None) => None,
                };
                let Some((name, _)) = bytes else {
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
                    if ui.button("Choose…").clicked()
                        && let Some(dir) = rfd::FileDialog::new()
                            .set_directory(data::expand(folder))
                            .pick_folder()
                    {
                        *folder = dir.display().to_string();
                    }
                });
                error_line(ui, p, error);
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        let dir = data::expand(folder);
                        let saved = match (group, open) {
                            (Some(g), _) => self.engine.save_group_file(g, *m, &dir),
                            (None, Some(i)) => self.engine.save_file(i, *m, &dir),
                            (None, None) => Err("nothing open".into()),
                        };
                        match saved {
                            Ok(path) => {
                                self.status = format!("saved to {}", path.display());
                                keep = false;
                            }
                            Err(e) => *error = e,
                        }
                    }
                    if ui.button("Discard").clicked() {
                        match (group, open) {
                            (Some(g), _) => self.engine.discard_group_file(g, *m),
                            (None, Some(i)) => self.engine.discard_file(i, *m),
                            (None, None) => {}
                        }
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
                     deletes your queues.\nMOVED (restored on another device): erases only \
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

/// One row of a list in a dialog: a framed line.
fn row(ui: &mut egui::Ui, p: &Pal, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(p.bg)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(add);
        });
    ui.add_space(4.0);
}

/// A square with a name's first letter, as in the contact list.
fn avatar_letter(ui: &mut egui::Ui, p: &Pal, name: &str) {
    let letter: String = name
        .chars()
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4, p.surface);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional(13.0),
        p.text,
    );
    ui.add_space(4.0);
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
