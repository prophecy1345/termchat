//! Notes to self (ADR-046): a chat kept as a contact that is no one. On one
//! device nothing goes on the wire; with linked devices each note, edit and
//! delete goes to every other device of ours as `Body::Note`, and a note is
//! taken only from one of our own devices.

use super::*;
use termchat_core::proto::NOTES_LEVEL;

/// ADR-046 amendment 1: which notes to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteFilter {
    All,
    /// Notes with no tag: the inbox.
    Untagged,
    Favorites,
    Links,
    /// Notes with this tag, lower case, without the `#`.
    Tag(String),
}

/// The tags of `text`: words after `#` of letters, digits, `_` and `-`,
/// lower case, each once.
pub fn tags_of(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for (at, _) in text.match_indices('#') {
        let before = text[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '#') {
            continue;
        }
        let tag: String = text[at + 1..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .flat_map(char::to_lowercase)
            .collect();
        if !tag.is_empty() && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// Whether note `m` belongs under `filter`. A deleted note leaves no
/// "message deleted" behind: there is no one else to tell.
pub fn note_shown(m: &Msg, filter: &NoteFilter) -> bool {
    if m.deleted {
        return false;
    }
    match filter {
        NoteFilter::All => true,
        NoteFilter::Untagged => tags_of(&m.text).is_empty(),
        NoteFilter::Favorites => m.my_reaction.is_some(),
        NoteFilter::Links => ["http://", "https://", "www."]
            .iter()
            .any(|p| m.text.to_lowercase().contains(p)),
        NoteFilter::Tag(t) => tags_of(&m.text).contains(t),
    }
}

/// The Notes chat's key: all 0xff is no valid Ed25519 point, and unlike all
/// zeros no default value, so it can never name a contact, a device or an
/// identity by accident.
pub const NOTES_KEY: Ed25519Key = [0xff; 32];

/// Amendment 2: at most this many messages to catch a device up, the
/// relay's burst for one queue.
const CATCH_UP_NOTES: usize = 30;

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

    /// Every tag in Notes with how many notes carry it, most used first.
    pub fn note_tags(&self) -> Vec<(String, usize)> {
        let Some(i) = self.notes() else {
            return Vec::new();
        };
        let mut counts: Vec<(String, usize)> = Vec::new();
        for m in self.data.contacts[i].messages.iter().filter(|m| !m.deleted) {
            for t in tags_of(&m.text) {
                match counts.iter_mut().find(|(x, _)| *x == t) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((t, 1)),
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        counts
    }

    /// Stars note `m`, or takes the star away; our other devices follow.
    pub fn toggle_star(&mut self, m: usize) -> Result<(), String> {
        let i = self.notes().ok_or("no Notes here")?;
        let starred = self.data.contacts[i]
            .messages
            .get(m)
            .ok_or("no such note")?
            .my_reaction
            .is_some();
        self.react(i, m, if starred { None } else { Some(0) })
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

    /// ADR-046 amendment 2: our device `d` reads notes from now; send it
    /// those written since it was linked, which it could not take before.
    pub(super) fn catch_up_notes(&mut self, d: usize) {
        let Some(i) = self.notes() else { return };
        let (key, since, now) = (
            self.data.contacts[d].key,
            self.data.contacts[d].added_at,
            now(),
        );
        let mut kept: Vec<(Body, Option<Body>)> = Vec::new();
        let mut count = 0;
        // Newest first while counting, so the cap drops the oldest.
        for m in self.data.contacts[i].messages.iter().rev() {
            let gone = m.expires_at.is_some_and(|end| end <= now);
            // A queued note goes with the next flush; documents are not in Notes.
            let Some(id) = m.id.filter(|_| !(m.deleted || gone || m.at < since)) else {
                continue;
            };
            if m.status == Status::Queued || m.file.is_some() {
                continue;
            }
            let star = m.my_reaction.map(|r| Body::React {
                target: Ref { id, theirs: false },
                reaction: Some(r),
            });
            count += 1 + usize::from(star.is_some());
            if count > CATCH_UP_NOTES {
                break;
            }
            let expires_in = m
                .expires_at
                .map(|end| u32::try_from(end - now).unwrap_or(u32::MAX));
            let body = Body::Message {
                id,
                sent_at: m.at,
                expires_in,
                reply_to: m.reply_to,
                text: m.text.clone(),
            };
            kept.push((body, star));
        }
        kept.reverse();
        let (notes, stars): (Vec<_>, Vec<_>) = kept.into_iter().unzip();
        for body in notes.into_iter().chain(stars.into_iter().flatten()) {
            let note = Body::Note {
                body: Box::new(body),
            };
            if let Err(e) = self.transmit(&key, note) {
                return self.note(e);
            }
        }
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
            // Amendment 1: a star, only on one of our notes.
            Body::React { target, reaction } if !target.theirs => {
                if let Some(m) = at(msgs, target.id) {
                    msgs[m].my_reaction = reaction;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_filters() {
        assert_eq!(
            tags_of("#Work and #read-later, #work again"),
            ["work", "read-later"]
        );
        assert_eq!(tags_of("issue#12 ## #"), Vec::<String>::new());
        assert_eq!(tags_of("#рецепт_1"), ["рецепт_1"]);
        let note = |text: &str, star| {
            let mut m = Msg::default();
            m.text = text.into();
            m.my_reaction = star;
            m
        };
        let link = note("see https://example.org #read", None);
        let plain = note("buy milk", Some(0));
        assert!(note_shown(&link, &NoteFilter::Links));
        assert!(!note_shown(&plain, &NoteFilter::Links));
        assert!(note_shown(&plain, &NoteFilter::Untagged));
        assert!(!note_shown(&link, &NoteFilter::Untagged));
        assert!(note_shown(&plain, &NoteFilter::Favorites));
        assert!(note_shown(&link, &NoteFilter::Tag("read".into())));
        assert!(note_shown(&link, &NoteFilter::All));
        let mut gone = note("buy milk", None);
        gone.wipe();
        assert!(!note_shown(&gone, &NoteFilter::All));
    }
}
