//! Notes to self (ADR-046): a chat kept as a contact that is no one. On one
//! device nothing goes on the wire; with linked devices each note, edit and
//! delete goes to every other device of ours as `Body::Note`, and a note is
//! taken only from one of our own devices.

use super::*;
use termchat_core::proto::NOTES_LEVEL;

/// The Notes chat's key: all 0xff is no valid Ed25519 point, and unlike all
/// zeros no default value, so it can never name a contact, a device or an
/// identity by accident.
pub const NOTES_KEY: Ed25519Key = [0xff; 32];

impl Engine {
    /// Index of the Notes chat.
    pub fn notes(&self) -> Option<usize> {
        self.data.contacts.iter().position(|c| c.notes)
    }

    /// Every identity's main profile has one.
    pub(super) fn ensure_notes(&mut self) {
        if self.notes().is_none() {
            // After the people, before our devices: a chat like the others,
            // sorted by its last note; pin it to keep it on top.
            let at = self.data.people();
            self.data.contacts.insert(
                at,
                Contact {
                    key: NOTES_KEY,
                    notes: true,
                    ..Default::default()
                },
            );
            self.persist();
        }
    }

    /// Our own devices that take notes now.
    fn note_devices(&self) -> Vec<Ed25519Key> {
        let c = self.data.contacts.iter();
        let takes = |c: &&Contact| c.own && !c.left && c.understands >= NOTES_LEVEL;
        c.filter(takes)
            .map(|c| c.key)
            .filter(|k| self.has_session(k))
            .collect()
    }

    /// What the Notes chat has to send: new notes, then edits and deletes,
    /// to each device of ours. Here they are done at once.
    pub(super) fn flush_notes(&mut self, i: usize) {
        let devices = self.note_devices();
        let mut out = Vec::new();
        for m in &mut self.data.contacts[i].messages {
            if m.status != Status::Queued || m.file.is_some() {
                continue;
            }
            m.status = Status::Sent;
            let expires_in = m
                .expires_at
                .map(|end| u32::try_from(end.saturating_sub(m.at)).unwrap_or(u32::MAX));
            if let Some(id) = m.id {
                out.push(Body::Message {
                    id,
                    sent_at: m.at,
                    expires_in,
                    reply_to: m.reply_to,
                    text: m.text.clone(),
                });
            }
        }
        out.append(&mut self.data.contacts[i].outbox);
        // Called on every tick through `deliver`: write only when there was work.
        if out.is_empty() {
            return;
        }
        for body in out {
            for d in &devices {
                let note = Body::Note {
                    body: Box::new(body.clone()),
                };
                if let Err(e) = self.transmit(d, note) {
                    self.note(e);
                }
            }
        }
        self.persist();
    }

    /// A note from one of our own devices, filed into the Notes chat.
    pub(super) fn note_in(&mut self, body: Body) {
        let Some(i) = self.notes() else { return };
        let msgs = &mut self.data.contacts[i].messages;
        let at = |msgs: &Vec<Msg>, id: u64| msgs.iter().position(|m| m.id == Some(id));
        match body {
            Body::Message {
                id,
                sent_at,
                expires_in,
                reply_to,
                text,
            } if at(msgs, id).is_none() => {
                let now = now();
                let mut m = Msg::default();
                (m.mine, m.text, m.at) = (true, text, sent_at.min(now));
                m.expires_at = expires_in.map(|s| now + u64::from(s));
                (m.status, m.id, m.reply_to) = (Status::Sent, Some(id), reply_to);
                msgs.push(m);
            }
            Body::Edit { id, text } => {
                if let Some(m) = at(msgs, id) {
                    msgs[m].replace_text(text);
                    msgs[m].edited = true;
                }
            }
            Body::Delete { id } => {
                if let Some(m) = at(msgs, id) {
                    msgs[m].wipe();
                }
            }
            _ => {}
        }
    }
}
