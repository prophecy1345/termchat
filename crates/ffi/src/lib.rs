//! What the Android app sees of `termchat-engine` (ADR-011): an `Account`
//! with plain methods, and a `Listener` the engine calls back.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use termchat_core::proto::{Ed25519Key, Invite};
use termchat_engine::data::{Contact, Group, Msg, Status};
use termchat_engine::engine::{self, Setup, next_timer};
use termchat_engine::{Engine, Happened, Unlocked};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

uniffi::setup_scaffolding!();

/// How often the relay's events are taken in.
/// Polling, 200 ms of latency; await the channel if that shows.
const POLL: Duration = Duration::from_millis(200);
/// A document named by its id that no message holds any more.
const GONE: &str = "this file is gone";

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("cannot start tokio: {e}"))
    })
}

/// Reaches Kotlin as an exception whose message is the text.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum TermchatError {
    #[error("{0}")]
    Failed(String),
}

impl From<String> for TermchatError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

type Result<T> = std::result::Result<T, TermchatError>;

/// What the app shows after something happened (`Happened` in the engine).
#[derive(uniffi::Enum)]
pub enum Event {
    /// A new message or contact request from contact `index`.
    Alert { index: Option<u32>, request: bool },
    /// One line for a status bar or a toast.
    Note { text: String },
    /// Worth a dialog.
    Notice { text: String },
    /// Everything is erased; go to the first-run screen with this note.
    Wiped { note: String },
    /// A wipe could not tell the relay: show `why`, then ask "wipe anyway?".
    WipeAnyway { why: String },
    /// An incognito invite asked for earlier is ready (ADR-023).
    Invite { text: String },
    /// A new message in group `index` (ADR-027).
    GroupAlert { index: Option<u32> },
}

#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    fn on_event(&self, event: Event);
    /// Something in the account changed: contacts, messages or the link.
    fn on_change(&self);
}

/// UnifiedPush instance of the main identity's invite queue: the connector's
/// default (ADR-014). Every other queue has its own instance (ADR-023).
const MAIN_INSTANCE: &str = "default";

/// The member of group `g` with this fingerprint.
fn member_key(e: &Engine, g: usize, fingerprint: &str) -> std::result::Result<[u8; 32], String> {
    e.data.groups[g]
        .members()
        .iter()
        .find(|m| termchat_core::fingerprint(&m.ed25519) == fingerprint)
        .map(|m| m.ed25519)
        .ok_or_else(|| String::from("not in this group"))
}

fn hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// ADR-046 amendment 1: a tag in Notes and how many notes carry it.
#[derive(uniffi::Record)]
pub struct TagCount {
    pub tag: String,
    pub count: u32,
}

/// ADR-045: an introduction waiting for our answer in a chat.
#[derive(uniffi::Record)]
pub struct IntroView {
    pub id: u64,
    /// `true`: they ask us to meet someone; `false`: they pass on an invite.
    pub ask: bool,
    pub nickname: String,
    pub fingerprint: String,
}

/// ADR-044: an image for the viewer, RGBA, not premultiplied, at most
/// `picture::MAX_SHOWN` on the longest side.
#[derive(uniffi::Record)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn picture(bytes: &[u8]) -> std::result::Result<Picture, String> {
    let p = termchat_engine::picture::decode(bytes)?;
    Ok(Picture {
        width: p.width,
        height: p.height,
        rgba: p.rgba.to_vec(),
    })
}

#[derive(uniffi::Record)]
pub struct ContactView {
    pub index: u32,
    pub name: String,
    pub fingerprint: String,
    pub unread: u32,
    pub verified: bool,
    /// ADR-046: the Notes chat, kept on your own devices.
    pub notes: bool,
    pub blocked: bool,
    pub left: bool,
    /// They asked; Accept or delete.
    pub request: bool,
    pub expires_in: Option<u32>,
    pub no_history: bool,
    pub last_at: u64,
    /// The name they know us by, if we talk to them from an incognito identity (ADR-014).
    pub incognito: Option<String>,
    /// From before anonymous queues: only a new invite brings them back (ADR-023).
    pub needs_invite: bool,
    /// Their app reads answers, edits, deletes and reactions (ADR-025).
    pub rich: bool,
    /// "off", "always" or the time left: no notification for its messages.
    pub mute: String,
    /// At the top of the list.
    pub pinned: bool,
    /// Above the others, under "Archived".
    pub archived: bool,
    /// Its link has not reached the relay yet: its messages wait (ADR-038).
    pub connecting: bool,
}

/// A chat as a plain text file: its suggested name and the text.
#[derive(uniffi::Record)]
pub struct ExportView {
    pub name: String,
    pub text: String,
}

/// A chat or a group, for `forward`.
#[derive(uniffi::Enum)]
pub enum Place {
    Chat { index: u32 },
    Group { index: u32 },
}

/// A message waiting to go later (PLAN Q4).
#[derive(uniffi::Record)]
pub struct LaterView {
    pub at: u64,
    pub text: String,
}

fn views(later: &[termchat_engine::data::Later]) -> Vec<LaterView> {
    later
        .iter()
        .map(|l| LaterView {
            at: l.at,
            text: l.text.clone(),
        })
        .collect()
}

/// A message picked to delete: its `MessageView` id and side.
#[derive(uniffi::Record)]
pub struct Picked {
    pub id: u64,
    pub mine: bool,
}

/// Where the picked messages are now; in a group the id alone names one.
fn picked_rows(msgs: &[Msg], picked: &[Picked], group: bool) -> Vec<usize> {
    let hit = |m: &Msg, p: &Picked| m.id == Some(p.id) && (group || m.mine == p.mine);
    (0..msgs.len())
        .filter(|&n| !msgs[n].deleted && picked.iter().any(|p| hit(&msgs[n], p)))
        .collect()
}

#[derive(uniffi::Record)]
pub struct MessageView {
    pub mine: bool,
    /// What a pick for `delete_here` names it by, with `mine`: rows move
    /// as older messages expire. `None`: it cannot be picked.
    pub id: Option<u64>,
    pub text: String,
    pub at: u64,
    pub system: bool,
    /// "queued", "sending", "sent", "received", "system" or the failure.
    pub status: String,
    /// A document (ADR-017): its name; `text` then holds name, size, progress.
    pub file_name: Option<String>,
    /// A received document still waiting to be saved or discarded.
    pub file_unsaved: bool,
    /// The document's id: it names a received document in the calls that
    /// save, view or forget it, which outlive the row (audit 2026-11-25, 3).
    pub file_id: Option<u32>,
    /// ADR-043: a document offered to us, not asked for or declined yet.
    pub file_offered: bool,
    /// ADR-044: a received image that can be viewed now.
    pub file_picture: bool,
    /// It can be answered and reacted to (ADR-025).
    pub pointable: bool,
    /// Ours: on a device of theirs, and seen by them (ADR-041).
    pub delivered: bool,
    pub read: bool,
    /// Its text, or a received document still here, can go to another chat.
    pub forwardable: bool,
    pub edited: bool,
    pub deleted: bool,
    /// What it answers: the first line of that message, or a note that it is gone.
    pub quote: Option<String>,
    /// Indices into `reactions()`.
    pub their_reaction: Option<u8>,
    pub my_reaction: Option<u8>,
    /// In a group: who wrote it, when not us (ADR-027).
    pub author: Option<String>,
    /// The colour of that name, as an index into the theme's slots.
    pub author_slot: Option<u32>,
    /// In a group: each reaction once, with how many chose it.
    pub counts: Vec<ReactionCount>,
}

#[derive(uniffi::Record)]
pub struct ReactionCount {
    pub reaction: u8,
    pub count: u32,
    /// Who chose it: names, "you" for us.
    pub names: Vec<String>,
}

/// A small private group (ADR-027).
#[derive(uniffi::Record)]
pub struct GroupView {
    pub index: u32,
    pub name: String,
    pub members: u32,
    pub unread: u32,
    /// We hold the group's admin token: we may invite (ADR-042).
    pub admin: bool,
    /// We are the main admin: we remove, name, set the timer, make admins.
    pub main_admin: bool,
    /// Who becomes the main admin if we, the main admin, leave.
    pub next_admin: Option<String>,
    /// We left or were removed: read-only.
    pub left: bool,
    /// The admin's disappearing timer, seconds.
    pub expires_in: Option<u32>,
    pub last_at: u64,
    /// As for a chat: "off", "always" or the time left.
    pub mute: String,
    /// Its link has not reached the relay yet (ADR-038).
    pub connecting: bool,
}

/// One of our devices (ADR-028), the main one first.
#[derive(uniffi::Record)]
pub struct DeviceView {
    /// Seen only by our own devices; empty if none was given.
    pub label: String,
    pub fingerprint: String,
    pub main: bool,
    pub me: bool,
}

#[derive(uniffi::Record)]
pub struct MemberView {
    pub name: String,
    pub fingerprint: String,
    /// An admin, main or helper (ADR-042).
    pub admin: bool,
    /// The main admin.
    pub main_admin: bool,
    pub me: bool,
}

/// A group invite from contact `from`, to join or decline.
#[derive(uniffi::Record)]
pub struct GroupInviteView {
    pub index: u32,
    pub from: u32,
    pub name: String,
}

/// A contact the admin can invite, and whether they are invited already.
#[derive(uniffi::Record)]
pub struct InvitableView {
    pub index: u32,
    pub name: String,
    pub invited: bool,
}

/// How a reaction is drawn (ADR-025): lit pixels as x, y pairs in an 11×11
/// square, and its colour as an index into a theme's 17 colours.
#[derive(uniffi::Record)]
pub struct Reaction {
    pub name: String,
    pub pixels: Vec<u8>,
    pub slot: u32,
}

#[uniffi::export]
pub fn reactions() -> Vec<Reaction> {
    (0..termchat_core::proto::REACTIONS)
        .map(|r| Reaction {
            name: engine::REACTION_NAMES[usize::from(r)].into(),
            pixels: termchat_engine::reactions::pixels(r)
                .flat_map(|(x, y)| [x, y])
                .collect(),
            slot: termchat_engine::reactions::COLOURS[usize::from(r)] as u32,
        })
        .collect()
}

/// One of the apps' icons (engine `icons`): lit pixels as x, y pairs in an
/// 11×11 square. The app colours it.
#[derive(uniffi::Record)]
pub struct IconSprite {
    pub name: String,
    pub pixels: Vec<u8>,
}

#[uniffi::export]
pub fn icons() -> Vec<IconSprite> {
    termchat_engine::icons::ALL
        .iter()
        .map(|&i| IconSprite {
            name: i.name().into(),
            pixels: termchat_engine::icons::pixels(i)
                .flat_map(|(x, y)| [x, y])
                .collect(),
        })
        .collect()
}

#[uniffi::export]
pub fn state_exists(dir: String) -> bool {
    state(&dir).exists()
}

/// Copies a backup in; unlock it with the backup's password next.
#[uniffi::export]
pub fn restore(dir: String, backup: String) -> Result<()> {
    Ok(engine::restore(&state(&dir), Path::new(&backup))?)
}

#[derive(uniffi::Record)]
pub struct Theme {
    pub name: String,
    /// 17 RGB colours, in the order of `termchat_engine::themes::SLOTS`.
    pub colors: Vec<u32>,
}

/// The colour themes the terminal client has too.
#[uniffi::export]
pub fn themes() -> Vec<Theme> {
    termchat_engine::themes::THEMES
        .iter()
        .map(|(name, c)| Theme {
            name: (*name).into(),
            colors: c.to_vec(),
        })
        .collect()
}

/// "What's new": this build's CHANGELOG.md.
#[uniffi::export]
pub fn changelog() -> String {
    let all = include_str!("../../../CHANGELOG.md");
    all[all.find("## ").unwrap_or(0)..].to_string()
}

#[uniffi::export]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

#[uniffi::export]
pub fn generate_password() -> String {
    termchat_engine::password::generate().to_string()
}

fn state(dir: &str) -> PathBuf {
    Path::new(dir).join("state")
}

#[derive(uniffi::Object)]
pub struct Account {
    engine: Arc<Mutex<Box<Engine>>>,
    poll: JoinHandle<()>,
}

impl Drop for Account {
    fn drop(&mut self) {
        self.poll.abort();
        self.lock().persist();
    }
}

/// `None` when the password was the duress one: everything is gone (ADR-003).
#[uniffi::export]
pub fn unlock(
    dir: String,
    password: String,
    listener: Arc<dyn Listener>,
) -> Result<Option<Arc<Account>>> {
    let _rt = runtime().enter();
    match Engine::unlock(&state(&dir), password.as_bytes())? {
        Unlocked::Open(engine) => Ok(Some(Account::start(engine, listener))),
        Unlocked::Wiped => Ok(None),
    }
}

/// The project's public relay and its onion address, prefilled on first run.
#[uniffi::export]
pub fn default_relay() -> Vec<String> {
    vec![engine::DEFAULT_RELAY.into(), engine::DEFAULT_ONION.into()]
}

/// The nickname of a device set up only to be linked; the main device's
/// replaces it (ADR-028).
#[uniffi::export]
pub fn new_device_nickname() -> String {
    termchat_engine::NEW_DEVICE_NICKNAME.into()
}

/// This build can connect through Tor (ADR-004).
#[uniffi::export]
pub fn tor_available() -> bool {
    cfg!(feature = "tor")
}

/// A new identity. With `tor`, it connects only through Tor to `onion_url`.
#[uniffi::export]
pub fn setup(
    dir: String,
    relay_url: String,
    onion_url: Option<String>,
    tor: bool,
    nickname: String,
    password: String,
    listener: Arc<dyn Listener>,
) -> Result<Arc<Account>> {
    let _rt = runtime().enter();
    let onion = onion_url
        .as_deref()
        .map(str::trim)
        .filter(|o| !o.is_empty());
    let setup = Setup {
        relay_url: relay_url.trim(),
        onion_url: onion,
        tor,
        nickname: nickname.trim(),
        password: &password,
    };
    Ok(Account::start(
        Engine::setup(&state(&dir), &setup)?,
        listener,
    ))
}

impl Account {
    fn start(engine: Box<Engine>, listener: Arc<dyn Listener>) -> Arc<Self> {
        let engine = Arc::new(Mutex::new(engine));
        let shared = Arc::clone(&engine);
        let poll = runtime().spawn(async move {
            let mut ticks = 0u32;
            loop {
                let (events, changed) = {
                    let mut e = shared.lock().unwrap_or_else(PoisonError::into_inner);
                    let mut changed = false;
                    while let Ok(ev) = e.events.try_recv() {
                        e.on_relay(ev);
                        changed = true;
                    }
                    ticks = ticks.wrapping_add(1);
                    if ticks.is_multiple_of(5) {
                        e.tick();
                    }
                    let happened = std::mem::take(&mut e.happened);
                    let events: Vec<Event> = happened.into_iter().map(|h| event(&e, h)).collect();
                    (events, changed)
                };
                let changed = changed || !events.is_empty();
                for ev in events {
                    listener.on_event(ev);
                }
                if changed {
                    listener.on_change();
                }
                tokio::time::sleep(POLL).await;
            }
        });
        Arc::new(Self { engine, poll })
    }

    fn lock(&self) -> MutexGuard<'_, Box<Engine>> {
        self.engine.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` on group `index`, if it exists.
    fn with_group<T>(&self, index: u32, f: impl FnOnce(&mut Engine, usize) -> T) -> Result<T> {
        let _rt = runtime().enter();
        let mut e = self.lock();
        let g = usize::try_from(index).unwrap_or(usize::MAX);
        if g >= e.data.groups.len() {
            return Err(String::from("no such group").into());
        }
        Ok(f(&mut e, g))
    }

    /// Runs `f` on contact `index`, if it exists.
    fn with<T>(&self, index: u32, f: impl FnOnce(&mut Engine, usize) -> T) -> Result<T> {
        // Accepting, deleting, blocking or writing can start a relay link,
        // which needs the runtime: without it the spawn panics, the app sees
        // a quiet error, and the link waits for the next unlock (ADR-023).
        let _rt = runtime().enter();
        let mut e = self.lock();
        let i = usize::try_from(index).unwrap_or(usize::MAX);
        if i >= e.data.people() {
            return Err(String::from("no such contact").into());
        }
        Ok(f(&mut e, i))
    }
}

/// One message for the app; `group` when it is a group's (ADR-027).
fn msg_view(m: &Msg, all: &[Msg], group: Option<&Group>, me: Ed25519Key) -> MessageView {
    let mut counts: Vec<ReactionCount> = Vec::new();
    let mut my_reaction = m.my_reaction;
    if let (Some(g), Some(id)) = (group, m.id) {
        my_reaction = None;
        for (k, r) in g.reactions_to(id) {
            let who = match k == me {
                true => {
                    my_reaction = Some(r);
                    "you".to_string()
                }
                false => engine::clean(g.name_of(&k).unwrap_or("someone")),
            };
            match counts.iter_mut().find(|c| c.reaction == r) {
                Some(c) => {
                    c.count += 1;
                    c.names.push(who);
                }
                None => counts.push(ReactionCount {
                    reaction: r,
                    count: 1,
                    names: vec![who],
                }),
            }
        }
    }
    MessageView {
        mine: m.mine,
        id: m.id.filter(|_| !m.deleted),
        text: engine::clean_text(
            &m.file
                .as_ref()
                .map_or_else(|| m.text.clone(), |f| f.label()),
        ),
        at: m.at,
        system: m.status == Status::System,
        status: match &m.status {
            Status::Queued => "queued".into(),
            Status::Sending(_) => "sending".into(),
            Status::Sent => "sent".into(),
            Status::Received => "received".into(),
            Status::System => "system".into(),
            Status::Failed(why) => why.clone(),
        },
        file_name: m.file.as_ref().map(|f| engine::safe_name(&f.name)),
        file_unsaved: !m.mine && m.file.as_ref().is_some_and(|f| f.complete() && f.held()),
        file_id: m.file.as_ref().map(|f| f.id),
        file_offered: !m.mine && m.file.as_ref().is_some_and(|f| f.offered()),
        file_picture: !m.mine && m.file.as_ref().is_some_and(|f| f.viewable()),
        pointable: m.id.is_some() && !m.deleted && m.status != Status::System,
        delivered: m.delivered,
        read: m.read,
        forwardable: m.forwardable().is_some(),
        edited: m.edited,
        deleted: m.deleted,
        quote: m.reply_to.map(|r| {
            // In a group the id alone names it.
            all.iter()
                .find(|q| {
                    q.id == Some(r.id) && (group.is_some() || q.mine != r.theirs) && !q.deleted
                })
                .map_or_else(
                    || "message no longer here".into(),
                    |q| {
                        let text = engine::clean_text(&q.text);
                        text.lines().next().unwrap_or_default().to_string()
                    },
                )
        }),
        their_reaction: if group.is_some() {
            None
        } else {
            m.their_reaction
        },
        my_reaction,
        author: match (group, m.author) {
            (Some(g), Some(k)) if !m.mine => {
                Some(engine::clean(g.name_of(&k).unwrap_or("someone")))
            }
            _ => None,
        },
        author_slot: match (group, m.author) {
            (Some(_), Some(k)) if !m.mine => Some(termchat_engine::themes::name_slot(&k) as u32),
            _ => None,
        },
        counts,
    }
}

fn event(e: &Engine, h: Happened) -> Event {
    match h {
        Happened::Alert { key, request } => Event::Alert {
            index: e.index(&key).map(|i| i as u32),
            request,
        },
        Happened::Note(text) => Event::Note { text },
        Happened::Notice(text) => Event::Notice { text },
        Happened::Wiped(note) => Event::Wiped { note },
        Happened::WipeAnyway(why) => Event::WipeAnyway { why },
        Happened::Invite(text) => Event::Invite { text },
        Happened::GroupAlert { group } => Event::GroupAlert {
            index: e
                .data
                .groups
                .iter()
                .position(|g| g.id == group)
                .map(|g| g as u32),
        },
    }
}

fn view(e: &Engine, i: usize, c: &Contact) -> ContactView {
    ContactView {
        index: i as u32,
        name: e.name(c),
        fingerprint: termchat_core::fingerprint(&c.key),
        unread: u32::try_from(c.unread).unwrap_or(u32::MAX),
        verified: c.verified,
        notes: c.notes,
        blocked: c.blocked,
        left: c.left,
        request: e.is_request(&c.key),
        incognito: e.profile_of(c).map(|p| p.nickname.clone()),
        expires_in: c.expires_in,
        no_history: c.no_history,
        last_at: c.last_at(),
        needs_invite: e.needs_invite(c),
        rich: e.rich(i),
        mute: engine::mute_label(c.muted_until),
        pinned: c.pinned,
        archived: c.archived,
        connecting: e.chat_connecting(&c.key),
    }
}

/// The app's own lyrebird, for Tor bridges (ADR-015); call before setup or unlock.
#[uniffi::export]
pub fn set_transport_program(path: String) {
    engine::set_pt_program(path.into());
}

#[uniffi::export]
impl Account {
    pub fn nickname(&self) -> String {
        self.lock().data.nickname.clone()
    }

    pub fn fingerprint(&self) -> String {
        termchat_core::fingerprint(&self.lock().me())
    }

    pub fn tor(&self) -> bool {
        self.lock().data.tor
    }

    /// Switches Tor on or off and reconnects; returns what to tell the user.
    pub fn toggle_tor(&self) -> String {
        let _rt = runtime().enter();
        self.lock().toggle_tor()
    }

    /// The app on screen again (ADR-012).
    pub fn reconnect(&self) {
        self.lock().reconnect();
    }

    pub fn connected(&self) -> bool {
        self.lock().connected
    }

    /// No reminder needed: backed up, or a linked device, which needs none (ADR-028).
    pub fn backed_up(&self) -> bool {
        !self.lock().needs_backup()
    }

    /// A newer termchat the relay runs (ADR-010).
    pub fn update(&self) -> Option<String> {
        self.lock().update.clone()
    }

    /// One-time: a new code on every call (ADR-013).
    /// `Err` until the invite queue exists, right after the first connect.
    pub fn invite(&self) -> Result<String> {
        Ok(self.lock().invite()?)
    }

    /// Reusable; `renew` replaces it and stops the old one.
    pub fn public_invite(&self, renew: bool) -> Result<String> {
        Ok(self.lock().public_invite(renew)?)
    }

    /// How many unused one-time invites stopped working.
    pub fn revoke_invites(&self) -> u32 {
        u32::try_from(self.lock().revoke_invites()).unwrap_or(u32::MAX)
    }

    /// Requests without an invite code are dropped.
    /// One connection for all contacts instead of one per contact (ADR-023).
    pub fn shared_link(&self) -> bool {
        self.lock().data.shared_link
    }

    pub fn set_shared_link(&self, on: bool) -> Result<String> {
        let _rt = runtime().enter();
        Ok(self.lock().set_shared_link(on)?)
    }

    /// The open chat connects at once instead of waiting out its spread (ADR-038).
    pub fn hurry_open(&self) -> bool {
        self.lock().data.hurry_open
    }

    pub fn set_hurry_open(&self, on: bool) {
        let _rt = runtime().enter();
        self.lock().set_hurry_open(on);
    }

    /// ADR-041: send delivery receipts; send read receipts and show theirs.
    /// The app keeps both with its other settings.
    pub fn set_receipts(&self, delivered: bool, read: bool) {
        let mut e = self.lock();
        (e.receipts, e.read_receipts) = (delivered, read);
    }

    /// In a stable order: an index stays valid until a contact is deleted.
    /// The app sorts by `last_at` for display.
    pub fn contacts(&self) -> Vec<ContactView> {
        let e = self.lock();
        // Our own devices (ADR-028) come after the people, never shown.
        e.data
            .contacts
            .iter()
            .take(e.data.people())
            .enumerate()
            .map(|(i, c)| view(&e, i, c))
            .collect()
    }

    /// ADR-028: this device links others (it is the main one, or alone).
    pub fn is_primary(&self) -> bool {
        self.lock().is_primary()
    }

    /// A new device, with nobody on it yet, may ask to be linked.
    pub fn can_link(&self) -> bool {
        let e = self.lock();
        e.is_primary() && e.data.devices.is_none() && e.data.people() == 0
    }

    /// The primary removed this device: it reads and sends nothing more.
    pub fn removed(&self) -> bool {
        self.lock().removed()
    }

    pub fn link_code(&self) -> Result<String> {
        Ok(self.lock().link_code()?)
    }

    /// The device waiting for the fingerprint check, by index (for
    /// `confirm_link` and `cancel_link`).
    pub fn pending_link(&self) -> Option<u32> {
        self.lock().pending_link().map(|i| i as u32)
    }

    /// The new device cannot show the fingerprint yet (ADR-028).
    pub fn link_waiting(&self, device: u32) -> bool {
        self.lock().link_waiting(device as usize)
    }

    pub fn link_fingerprint(&self) -> String {
        self.lock().link_fingerprint()
    }

    pub fn add_device(&self, code: String) -> Result<u32> {
        let _rt = runtime().enter();
        Ok(self.lock().add_device(&code)? as u32)
    }

    pub fn confirm_link(&self, device: u32) {
        let _rt = runtime().enter();
        self.lock().confirm_link(device as usize);
    }

    pub fn cancel_link(&self, device: u32) {
        let _rt = runtime().enter();
        let mut e = self.lock();
        let i = device as usize;
        if e.data.contacts.get(i).is_some_and(|c| c.own) {
            e.delete(i);
        }
    }

    pub fn devices(&self) -> Vec<DeviceView> {
        let e = self.lock();
        let me = e.me();
        e.device_rows()
            .into_iter()
            .enumerate()
            .map(|(n, (k, label))| DeviceView {
                label,
                fingerprint: termchat_core::fingerprint(&k),
                main: n == 0,
                me: k == me,
            })
            .collect()
    }

    /// Device `row` of `devices()`.
    pub fn remove_device(&self, row: u32) -> Result<()> {
        let _rt = runtime().enter();
        let mut e = self.lock();
        let key = e.device_rows().get(row as usize).map(|(k, _)| *k);
        Ok(e.remove_device(&key.ok_or_else(|| String::from("no such device"))?)?)
    }

    pub fn set_device_label(&self, row: u32, label: String) -> Result<()> {
        let _rt = runtime().enter();
        let mut e = self.lock();
        let key = e.device_rows().get(row as usize).map(|(k, _)| *k);
        Ok(e.set_device_label(&key.ok_or_else(|| String::from("no such device"))?, &label)?)
    }

    pub fn messages(&self, index: u32) -> Result<Vec<MessageView>> {
        self.with(index, |e, i| {
            let all = &e.data.contacts[i].messages;
            all.iter().map(|m| msg_view(m, all, None, e.me())).collect()
        })
    }

    /// Groups in a stable order, like `contacts()`.
    pub fn groups(&self) -> Vec<GroupView> {
        let e = self.lock();
        e.data
            .groups
            .iter()
            .enumerate()
            .map(|(g, x)| GroupView {
                index: g as u32,
                name: engine::clean(x.name()),
                members: x.members().len() as u32,
                unread: u32::try_from(x.unread).unwrap_or(u32::MAX),
                admin: x.is_admin(),
                main_admin: x.is_admin() && x.main_admin() == Some(e.me()),
                next_admin: x
                    .admin_keys()
                    .get(1)
                    .and_then(|k| x.name_of(k))
                    .map(engine::clean),
                left: x.left,
                expires_in: x.timer(),
                last_at: x.messages.last().map_or(0, |m| m.at),
                mute: engine::mute_label(x.muted_until),
                connecting: e.group_connecting(g),
            })
            .collect()
    }

    pub fn group_messages(&self, group: u32) -> Result<Vec<MessageView>> {
        self.with_group(group, |e, g| {
            let x = &e.data.groups[g];
            let me = e.me();
            x.messages
                .iter()
                .map(|m| msg_view(m, &x.messages, Some(x), me))
                .collect()
        })
    }

    pub fn group_members(&self, group: u32) -> Result<Vec<MemberView>> {
        self.with_group(group, |e, g| {
            let x = &e.data.groups[g];
            let admins = x.admin_keys();
            x.members()
                .iter()
                .map(|m| MemberView {
                    name: engine::clean(&m.nickname),
                    fingerprint: termchat_core::fingerprint(&m.ed25519),
                    admin: admins.contains(&m.ed25519),
                    main_admin: admins.first() == Some(&m.ed25519),
                    me: m.ed25519 == e.me(),
                })
                .collect()
        })
    }

    pub fn group_invites(&self) -> Vec<GroupInviteView> {
        let e = self.lock();
        e.data
            .group_invites
            .iter()
            .enumerate()
            .filter_map(|(n, inv)| {
                Some(GroupInviteView {
                    index: n as u32,
                    from: e.index(&inv.from)? as u32,
                    name: engine::clean(&inv.roster.name),
                })
            })
            .collect()
    }

    pub fn invitable(&self, group: u32) -> Result<Vec<InvitableView>> {
        self.with_group(group, |e, g| {
            let x = &e.data.groups[g];
            e.invitable(g)
                .into_iter()
                .map(|i| {
                    let c = &e.data.contacts[i];
                    InvitableView {
                        index: i as u32,
                        name: e.name(c),
                        invited: x.asking.contains(&c.key)
                            || x.invited.iter().any(|(k, _)| *k == c.key),
                    }
                })
                .collect()
        })
    }

    /// Its index; the name is checked by the engine.
    pub fn create_group(&self, name: String) -> Result<u32> {
        let _rt = runtime().enter();
        Ok(self.lock().create_group(&name)? as u32)
    }

    pub fn invite_to_group(&self, group: u32, contact: u32) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.invite_to_group(g, contact as usize))??)
    }

    /// Joins with invite `invite`; the group's index.
    pub fn accept_group(&self, invite: u32) -> Result<u32> {
        let _rt = runtime().enter();
        Ok(self.lock().accept_group(invite as usize)? as u32)
    }

    pub fn decline_group(&self, invite: u32) {
        self.lock().decline_group(invite as usize);
    }

    pub fn send_group(&self, group: u32, text: String) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.send_group(g, &text))??)
    }

    /// Queues a document of at most 1 MiB into a group (ADR-027 amendment).
    pub fn send_group_file(&self, group: u32, name: String, bytes: Vec<u8>) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.send_group_file(g, &name, bytes))??)
    }

    /// ADR-044: received group image `file` (its id), decoded within limits.
    /// Call it off the main thread; the engine stays unlocked while it decodes.
    pub fn view_group_image(&self, group: u32, file: u32) -> Result<Picture> {
        let bytes = self.with_group(group, |e, g| {
            e.group_image_bytes(
                g,
                e.group_file_row(g, file).ok_or_else(|| GONE.to_string())?,
            )
        })??;
        Ok(picture(&bytes)?)
    }

    /// The bytes of received group document `file` (its id), for "Save as".
    pub fn group_file_bytes(&self, group: u32, file: u32) -> Result<Option<Vec<u8>>> {
        self.with_group(group, |e, g| {
            let m = e.group_file_row(g, file)?;
            e.group_file_bytes(g, m).map(|(_, b)| b)
        })
    }

    /// Forgets the bytes of received group document `file` (its id).
    pub fn forget_group_file(&self, group: u32, file: u32) -> Result<()> {
        self.with_group(group, |e, g| {
            if let Some(m) = e.group_file_row(g, file) {
                e.discard_group_file(g, m);
            }
        })
    }

    pub fn reply_group(&self, group: u32, msg: u32, text: String) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.reply_group(g, msg as usize, &text))??)
    }

    pub fn edit_group_message(&self, group: u32, msg: u32, text: String) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.edit_group_message(g, msg as usize, &text))??)
    }

    pub fn delete_group_message(&self, group: u32, msg: u32) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.delete_group_message(g, msg as usize))??)
    }

    pub fn react_group(&self, group: u32, msg: u32, reaction: Option<u8>) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.react_group(g, msg as usize, reaction))??)
    }

    /// The admin moves the group's timer on: off, 5m, 1h, 1d, 1w.
    pub fn next_group_timer(&self, group: u32) -> Result<Option<u32>> {
        self.with_group(group, |e, g| {
            let next = next_timer(e.data.groups[g].timer());
            e.set_group_timer(g, next).map(|()| next)
        })?
        .map_err(Into::into)
    }

    /// The admin removes the member with this fingerprint.
    pub fn remove_from_group(&self, group: u32, fingerprint: String) -> Result<()> {
        Ok(self.with_group(group, |e, g| {
            let who = member_key(e, g, &fingerprint)?;
            e.remove_from_group(g, &who)
        })??)
    }

    /// ADR-042: the main admin makes member `fingerprint` an admin.
    pub fn make_admin(&self, group: u32, fingerprint: String) -> Result<()> {
        Ok(self.with_group(group, |e, g| {
            let who = member_key(e, g, &fingerprint)?;
            e.make_admin(g, &who)
        })??)
    }

    /// ADR-042: the main admin takes the role from helper `fingerprint`.
    pub fn remove_admin(&self, group: u32, fingerprint: String) -> Result<()> {
        Ok(self.with_group(group, |e, g| {
            let who = member_key(e, g, &fingerprint)?;
            e.remove_admin(g, &who)
        })??)
    }

    /// ADR-042: a helper leaves the role.
    pub fn step_down(&self, group: u32) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.step_down(g))??)
    }

    /// ADR-042: the main admin hands the group to `fingerprint`; `leave`: and leaves.
    pub fn hand_over_group(&self, group: u32, fingerprint: String, leave: bool) -> Result<()> {
        Ok(self.with_group(group, |e, g| {
            let who = member_key(e, g, &fingerprint)?;
            e.hand_over_group(g, &who, leave)
        })??)
    }

    pub fn leave_group(&self, group: u32) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.leave_group(g))??)
    }

    /// Clears group `group`'s history on this device; what is on its way stays.
    pub fn clear_group_history(&self, group: u32) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.clear_group_history(g))??)
    }

    /// Clears chat `index`'s history on this device; what is on its way stays.
    pub fn clear_chat(&self, index: u32) -> Result<()> {
        Ok(self.with(index, |e, i| e.clear_chat(i))??)
    }

    /// Deletes the picked messages of chat `index` on this device; what
    /// is on its way stays. How many went.
    pub fn delete_here(&self, index: u32, picked: Vec<Picked>) -> Result<u32> {
        let n = self.with(index, |e, i| {
            let rows = picked_rows(&e.data.contacts[i].messages, &picked, false);
            e.delete_here(i, &rows)
        })??;
        Ok(n as u32)
    }

    /// As `delete_here`, for group `group`.
    pub fn delete_group_here(&self, group: u32, picked: Vec<Picked>) -> Result<u32> {
        let n = self.with_group(group, |e, g| {
            let rows = picked_rows(&e.data.groups[g].messages, &picked, true);
            e.delete_group_here(g, &rows)
        })??;
        Ok(n as u32)
    }

    /// The group on screen (`None`: none): its messages are read as they come.
    pub fn open_group(&self, group: Option<u32>) {
        let mut e = self.lock();
        let g = group.and_then(|g| usize::try_from(g).ok());
        let id = g.and_then(|g| e.data.groups.get(g)).map(|x| x.id);
        if let Some(g) = g.filter(|_| id.is_some()) {
            e.data.groups[g].unread = 0;
        }
        e.open_group = id;
    }

    /// The chat on screen (`None`: none): its messages are read as they come.
    pub fn open(&self, index: Option<u32>) {
        let mut e = self.lock();
        let i = index.and_then(|i| usize::try_from(i).ok());
        let key = i.and_then(|i| e.data.contacts.get(i)).map(|c| c.key);
        if let Some(i) = i.filter(|_| key.is_some()) {
            e.data.contacts[i].unread = 0;
        }
        e.open = key;
    }

    /// What was typed in chat `index` and not sent yet, kept with the state.
    /// PLAN Q4: send `text` to chat `index` at `at` (Unix seconds).
    pub fn send_later(&self, index: u32, text: String, at: u64) -> Result<()> {
        Ok(self.with(index, |e, i| e.send_later(i, &text, at))??)
    }

    pub fn send_group_later(&self, group: u32, text: String, at: u64) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.send_group_later(g, &text, at))??)
    }

    /// What waits to go later in chat `index`, soonest first.
    pub fn later(&self, index: u32) -> Vec<LaterView> {
        self.with(index, |e, i| views(&e.data.contacts[i].later))
            .unwrap_or_default()
    }

    pub fn group_later(&self, group: u32) -> Vec<LaterView> {
        self.with_group(group, |e, g| views(&e.data.groups[g].later))
            .unwrap_or_default()
    }

    /// Drops message `n` of `later(index)`.
    pub fn cancel_later(&self, index: u32, n: u32) -> Result<()> {
        Ok(self.with(index, |e, i| e.cancel_later(i, n as usize))??)
    }

    pub fn cancel_group_later(&self, group: u32, n: u32) -> Result<()> {
        Ok(self.with_group(group, |e, g| e.cancel_group_later(g, n as usize))??)
    }

    pub fn draft(&self, index: u32) -> String {
        self.with(index, |e, i| e.data.contacts[i].draft.clone())
            .unwrap_or_default()
    }

    /// Kept in memory as it is typed; saved with the state on lock.
    pub fn set_draft(&self, index: u32, text: String) {
        let _ = self.with(index, |e, i| e.data.contacts[i].draft = text);
    }

    pub fn group_draft(&self, group: u32) -> String {
        self.with_group(group, |e, g| e.data.groups[g].draft.clone())
            .unwrap_or_default()
    }

    pub fn set_group_draft(&self, group: u32, text: String) {
        let _ = self.with_group(group, |e, g| e.data.groups[g].draft = text);
    }

    pub fn send(&self, index: u32, text: String) -> Result<()> {
        Ok(self.with(index, |e, i| e.send(i, &text))??)
    }

    /// `text` as an answer to message `msg` (ADR-025).
    pub fn reply(&self, index: u32, msg: u32, text: String) -> Result<()> {
        Ok(self.with(index, |e, i| e.reply(i, msg as usize, &text))??)
    }

    /// New text for our message `msg` (ADR-025).
    pub fn edit_message(&self, index: u32, msg: u32, text: String) -> Result<()> {
        Ok(self.with(index, |e, i| e.edit_message(i, msg as usize, &text))??)
    }

    /// Deletes our message `msg` for both sides (ADR-025).
    pub fn delete_message(&self, index: u32, msg: u32) -> Result<()> {
        Ok(self.with(index, |e, i| e.delete_message(i, msg as usize))??)
    }

    /// Our reaction to message `msg`, an index into `reactions()`, or none (ADR-025).
    pub fn react(&self, index: u32, msg: u32, reaction: Option<u8>) -> Result<()> {
        Ok(self.with(index, |e, i| e.react(i, msg as usize, reaction))??)
    }

    /// Queues a document: up to 1 MiB it goes at once (ADR-017), up to
    /// 25 MiB it is offered and goes when they ask (ADR-043).
    pub fn send_file(&self, index: u32, name: String, bytes: Vec<u8>) -> Result<()> {
        Ok(self.with(index, |e, i| e.send_file(i, &name, bytes))??)
    }

    /// ADR-044: received image `file` (its id), decoded within limits. Call
    /// it off the main thread; the engine stays unlocked while it decodes.
    pub fn view_image(&self, index: u32, file: u32) -> Result<Picture> {
        let bytes = self.with(index, |e, i| {
            e.image_bytes(i, e.file_row(i, file).ok_or_else(|| GONE.to_string())?)
        })??;
        Ok(picture(&bytes)?)
    }

    /// ADR-046 amendment 1: the tags in Notes, most used first.
    pub fn note_tags(&self) -> Vec<TagCount> {
        let _rt = runtime().enter();
        self.lock()
            .note_tags()
            .into_iter()
            .map(|(tag, n)| TagCount {
                tag: engine::clean(&tag),
                count: u32::try_from(n).unwrap_or(u32::MAX),
            })
            .collect()
    }

    /// ADR-046 amendment 1: indices of the notes `filter` shows: "all",
    /// "favorites", "links" or "#tag".
    pub fn notes_shown(&self, filter: String) -> Vec<u32> {
        use engine::NoteFilter;
        let f = match filter.as_str() {
            "favorites" => NoteFilter::Favorites,
            "links" => NoteFilter::Links,
            t if t.starts_with('#') => NoteFilter::Tag(t[1..].to_lowercase()),
            _ => NoteFilter::All,
        };
        let _rt = runtime().enter();
        let e = self.lock();
        let Some(i) = e.notes() else {
            return Vec::new();
        };
        e.data.contacts[i]
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| engine::note_shown(m, &f))
            .map(|(n, _)| n as u32)
            .collect()
    }

    /// ADR-046 amendment 1: star note `msg`, or take the star away.
    pub fn toggle_star(&self, msg: u32) -> Result<()> {
        let _rt = runtime().enter();
        Ok(self.lock().toggle_star(msg as usize)?)
    }

    /// ADR-045: introduce contact `index` to contact `other`.
    pub fn introduce(&self, index: u32, other: u32) -> Result<()> {
        let other = other as usize;
        Ok(self.with(index, |e, i| e.introduce_contacts(i, other))??)
    }

    /// ADR-045: what contact `index`'s chat asks of us, if anything.
    pub fn pending_intro(&self, index: u32) -> Result<Option<IntroView>> {
        use engine::PendingIntro;
        self.with(index, |e, i| {
            e.pending_intro(i).map(|p| match p {
                PendingIntro::Ask {
                    id,
                    nickname,
                    fingerprint,
                } => IntroView {
                    id,
                    ask: true,
                    nickname,
                    fingerprint,
                },
                PendingIntro::Offer {
                    id,
                    nickname,
                    fingerprint,
                } => IntroView {
                    id,
                    ask: false,
                    nickname,
                    fingerprint,
                },
            })
        })
    }

    /// ADR-045: our answer to an introduction: "allow", "incognito" or "decline".
    pub fn answer_intro(&self, index: u32, id: u64, answer: String) -> Result<()> {
        use engine::IntroAnswer;
        let answer = match answer.as_str() {
            "allow" => IntroAnswer::Allow,
            "incognito" => IntroAnswer::AllowIncognito,
            _ => IntroAnswer::Decline,
        };
        Ok(self.with(index, |e, i| e.answer_intro(i, id, answer))??)
    }

    /// ADR-045: add the introduced contact, or not; the new contact's index.
    pub fn answer_offer(&self, index: u32, id: u64, add: bool) -> Result<Option<u32>> {
        let added = self.with(index, |e, i| e.answer_offer(i, id, add))??;
        Ok(added.map(|j| j as u32))
    }

    /// Asks the contact for offered document `msg` (ADR-043).
    pub fn download_file(&self, index: u32, msg: u32) -> Result<()> {
        Ok(self.with(index, |e, i| e.download_file(i, msg as usize))??)
    }

    /// The bytes of received document `file` (its id), for the system
    /// "Save as" dialog.
    pub fn file_bytes(&self, index: u32, file: u32) -> Result<Option<Vec<u8>>> {
        self.with(index, |e, i| {
            let m = e.file_row(i, file)?;
            e.file_bytes(i, m).map(|(_, b)| b)
        })
    }

    /// Forgets the bytes of received document `file` (its id), once saved
    /// or declined.
    pub fn forget_file(&self, index: u32, file: u32) -> Result<()> {
        self.with(index, |e, i| {
            if let Some(m) = e.file_row(i, file) {
                e.discard_file(i, m);
            }
        })
    }

    /// Adds a contact from their invite; returns its index. `incognito`
    /// talks to them from a new identity of its own (ADR-014).
    pub fn add(&self, invite: String, incognito: bool) -> Result<u32> {
        // An incognito identity starts a relay link of its own.
        let _rt = runtime().enter();
        let i = self.lock().add(invite.trim(), incognito)?;
        Ok(i as u32)
    }

    /// The fingerprint of the identity an invite belongs to: ours differ per
    /// incognito identity (ADR-014).
    pub fn invite_fingerprint(&self, invite: String) -> Result<String> {
        let invite = Invite::decode(&invite).map_err(|e| e.to_string())?;
        Ok(termchat_core::fingerprint(&invite.identity.ed25519))
    }

    /// An invite from a new identity used only with whoever takes it; an
    /// empty nickname picks two random words. It comes as `Event::Invite`
    /// once the identity has its invite queue (ADR-023).
    pub fn incognito_invite(&self, nickname: String) -> Result<()> {
        let _rt = runtime().enter();
        Ok(self.lock().incognito_invite(&nickname)?)
    }

    pub fn accept(&self, index: u32) -> Result<()> {
        self.with(index, |e, i| e.accept(i))
    }

    pub fn delete(&self, index: u32) -> Result<()> {
        self.with(index, |e, i| e.delete(i))
    }

    pub fn rename(&self, index: u32, name: String) -> Result<()> {
        let name = name.trim();
        let name = (!name.is_empty()).then(|| name.chars().take(64).collect());
        self.with(index, |e, i| e.edit(i, |c| c.petname = name))
    }

    /// The chat or group as plain text, for an export the person asked for.
    pub fn export(&self, place: Place) -> Result<ExportView> {
        let e = self.lock();
        let at = |n: u32| usize::try_from(n).unwrap_or(usize::MAX);
        let export = match place {
            Place::Chat { index } if at(index) < e.data.people() => e.export_chat(at(index)),
            Place::Group { index } => e.export_group(at(index)),
            Place::Chat { .. } => None,
        };
        let (name, text) = export.ok_or_else(|| String::from("no such chat"))?;
        Ok(ExportView { name, text })
    }

    /// Message `msg` of one chat or group into another, as a new message of ours.
    pub fn forward(&self, from: Place, msg: u32, to: Place) -> Result<()> {
        let _rt = runtime().enter();
        let mut e = self.lock();
        let at = |n: u32| usize::try_from(n).unwrap_or(usize::MAX);
        let msgs = match from {
            Place::Chat { index } => e.data.contacts.get(at(index)).map(|c| &c.messages),
            Place::Group { index } => e.data.groups.get(at(index)).map(|g| &g.messages),
        };
        let what = msgs
            .and_then(|m| m.get(at(msg)))
            .and_then(Msg::forwardable)
            .ok_or_else(|| String::from("nothing here to forward"))?;
        Ok(match to {
            Place::Chat { index } if at(index) < e.data.people() => e.forward(at(index), what),
            Place::Group { index } if at(index) < e.data.groups.len() => {
                e.forward_group(at(index), what)
            }
            _ => Err(String::from("no such chat")),
        }?)
    }

    /// The list is sorted by the app; the order of indices stays.
    pub fn set_pinned(&self, index: u32, on: bool) -> Result<()> {
        self.with(index, |e, i| e.edit(i, |c| c.pinned = on))
    }

    pub fn set_archived(&self, index: u32, on: bool) -> Result<()> {
        self.with(index, |e, i| e.edit(i, |c| c.archived = on))
    }

    /// Cycles the mute like Ctrl+V: off, an hour, a day, always.
    pub fn next_mute(&self, index: u32) -> Result<()> {
        self.with(index, |e, i| {
            e.edit(i, |c| c.muted_until = engine::next_mute(c.muted_until));
        })
    }

    pub fn next_group_mute(&self, group: u32) -> Result<()> {
        self.with_group(group, |e, g| {
            let x = &mut e.data.groups[g];
            x.muted_until = engine::next_mute(x.muted_until);
            e.persist();
        })
    }

    /// Cycles the disappearing timer like Ctrl+T; returns the new one.
    pub fn next_timer(&self, index: u32) -> Result<Option<u32>> {
        self.with(index, |e, i| {
            e.edit(i, |c| c.expires_in = next_timer(c.expires_in));
            e.data.contacts[i].expires_in
        })
    }

    pub fn set_blocked(&self, index: u32, on: bool) -> Result<()> {
        // Blocking deletes our queue for them (ADR-023).
        self.with(index, |e, i| e.block(i, on))
    }

    pub fn set_verified(&self, index: u32, on: bool) -> Result<()> {
        self.with(index, |e, i| e.edit(i, |c| c.verified = on))
    }

    pub fn set_no_history(&self, index: u32, on: bool) -> Result<()> {
        self.with(index, |e, i| e.edit(i, |c| c.no_history = on))
    }

    /// ADR-037: what the safety-number QR code holds.
    pub fn safety_code(&self, index: u32) -> Result<String> {
        self.with(index, |e, i| e.safety_code(i).unwrap_or_default())
    }

    /// ADR-037: a code scanned from their screen. True: it matched and the
    /// contact is verified; false: nothing changed.
    pub fn verify_by_code(&self, index: u32, scanned: String) -> Result<bool> {
        self.with(index, |e, i| e.verify_by_code(i, &scanned))
    }

    pub fn safety_number(&self, index: u32) -> Result<String> {
        self.with(index, |e, i| {
            let c = &e.data.contacts[i];
            termchat_core::safety_number(&e.me_for(c), &c.key)
        })
    }

    /// The exact text a report would disclose (ADR-006); `None`: nothing to report.
    pub fn report_text(&self, index: u32) -> Result<Option<String>> {
        self.with(index, |e, i| e.report_text(i))
    }

    pub fn report(&self, index: u32, text: String) -> Result<()> {
        self.with(index, |e, i| e.report(i, text))
    }

    pub fn backup(&self, path: String, password: String) -> Result<()> {
        Ok(self.lock().backup(Path::new(&path), &password)?)
    }

    pub fn change_password(&self, current: String, new: String) -> Result<()> {
        Ok(self.lock().change_password(&current, &new)?)
    }

    /// `None` turns it off. Returns what to tell the user.
    pub fn set_duress(&self, current: String, duress: Option<String>) -> Result<String> {
        Ok(self.lock().set_duress(&current, duress.as_deref())?)
    }

    /// `WIPE` (`moved: false`) or `MOVED` (ADR-009); `false`: a `WipeAnyway` follows.
    pub fn leave(&self, moved: bool) -> bool {
        self.lock().leave(moved)
    }

    /// "Wipe anyway" after a `WipeAnyway`.
    pub fn wipe_now(&self) {
        self.lock().finish_wipe("");
    }

    /// The Tor bridges: transport and address only (ADR-015).
    pub fn bridges(&self) -> Vec<String> {
        self.lock().bridges()
    }

    /// Replaces the bridge lines, one per line; returns what to tell the user.
    pub fn set_bridges(&self, text: String) -> Result<String> {
        let _rt = runtime().enter();
        Ok(self.lock().set_bridges(&text)?)
    }

    /// Every queue's UnifiedPush instance name: the main invite queue first,
    /// then each incognito identity's and each contact's. One shared
    /// endpoint would let the relay link the queues (ADR-014, ADR-023).
    /// Grows with contacts: ask again after `on_change`.
    pub fn push_instances(&self) -> Vec<String> {
        let e = self.lock();
        let me = e.me();
        let mut all = vec![MAIN_INSTANCE.to_string()];
        all.extend(e.push_owners().iter().filter(|k| **k != me).map(hex));
        // Each group's reader is a queue of its own (ADR-027).
        let readers = e
            .data
            .groups
            .iter()
            .filter(|g| g.reader.is_some() && !g.left);
        all.extend(readers.map(|g| group_instance(&g.id)));
        all
    }

    /// The UnifiedPush endpoint of `instance`, or `None` to stop its wake-ups (ADR-012).
    pub fn set_push(&self, instance: String, endpoint: Option<String>) {
        let mut e = self.lock();
        if let Some(id) = group_of_instance(&e, &instance) {
            return e.set_group_push(&id, endpoint);
        }
        if let Some(who) = identity_of(&e, &instance) {
            e.set_push(&who, endpoint);
        }
    }

    /// A push for `instance`: that queue's link checks itself (ADR-012).
    pub fn woken(&self, instance: String) {
        let e = self.lock();
        if let Some(id) = group_of_instance(&e, &instance) {
            return e.woken_group(&id);
        }
        if let Some(who) = identity_of(&e, &instance) {
            e.woken(&who);
        }
    }

    pub fn push(&self) -> Option<String> {
        self.lock().data.push.clone()
    }

    pub fn news_seen(&self) {
        self.lock().news_seen();
    }
}

/// A group reader's UnifiedPush instance: "g" and the group id in hex,
/// apart from the 64 hex digits of a contact's.
fn group_instance(id: &[u8; 16]) -> String {
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    format!("g{hex}")
}

fn group_of_instance(e: &Engine, instance: &str) -> Option<[u8; 16]> {
    instance.strip_prefix('g')?;
    e.data
        .groups
        .iter()
        .map(|g| g.id)
        .find(|id| group_instance(id) == instance)
}

/// Whose queue a UnifiedPush instance name is: an identity's or a contact's.
fn identity_of(e: &Engine, instance: &str) -> Option<Ed25519Key> {
    match instance {
        MAIN_INSTANCE => Some(e.me()),
        _ => e.push_owners().into_iter().find(|k| hex(k) == instance),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    struct Quiet;
    impl Listener for Quiet {
        fn on_event(&self, _: Event) {}
        fn on_change(&self) {}
    }

    #[test]
    fn setup_then_unlock() {
        let dir = std::env::temp_dir().join(format!("termchat-ffi-{}", std::process::id()));
        let dir = dir.display().to_string();
        let pw = "correct horse battery staple 42".to_string();
        let relay = "ws://localhost:1/".to_string();
        let a = setup(
            dir.clone(),
            relay,
            None,
            false,
            "ffi".into(),
            pw.clone(),
            Arc::new(Quiet),
        )
        .unwrap();
        // No relay here, so no invite queue yet, so no invite.
        assert!(a.invite().is_err());
        // Only Notes (ADR-046) at first.
        assert!(a.contacts().iter().all(|c| c.notes) && a.add("nonsense".into(), false).is_err());
        a.incognito_invite("ghost".into()).unwrap();
        assert_eq!(a.push_instances().len(), 2);
        // ADR-027: a group waits for its queue offline, but is there.
        let g = a.create_group("team".into()).unwrap();
        assert_eq!(a.groups()[0].name, "team");
        assert!(a.groups()[0].admin || a.groups()[0].members == 0);
        assert!(a.group_messages(g).unwrap().is_empty());
        assert!(a.group_messages(9).is_err() && a.create_group(String::new()).is_err());
        assert_eq!(
            a.push_instances().len(),
            2,
            "no reader yet, no push instance"
        );
        let me = a.fingerprint();
        drop(a);
        assert!(state_exists(dir.clone()));
        let wrong = unlock(dir.clone(), "nope".into(), Arc::new(Quiet));
        assert!(matches!(wrong, Err(TermchatError::Failed(_))));
        let a = unlock(dir.clone(), pw, Arc::new(Quiet)).unwrap().unwrap();
        assert_eq!(a.fingerprint(), me);
        // A wipe stays a wipe when the account is dropped (it saves on drop).
        assert!(a.leave(true));
        drop(a);
        assert!(!state_exists(dir.clone()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Kotlin calls in from threads outside the runtime. An action that
    /// starts a relay link (accepting, unblocking) must not panic there:
    /// the panic was swallowed and the contact got no link until the next
    /// unlock (found on a Pixel with 0.8.0).
    #[test]
    fn contact_actions_start_links_from_any_thread() {
        let dir = std::env::temp_dir().join(format!("termchat-ffi-rt-{}", std::process::id()));
        let dir = dir.display().to_string();
        let a = setup(
            dir.clone(),
            "ws://localhost:1/".into(),
            None,
            false,
            "ffi".into(),
            "correct horse battery staple 42".into(),
            Arc::new(Quiet),
        )
        .unwrap();
        a.lock().data.contacts.push(termchat_engine::data::Contact {
            key: [7; 32],
            ..Default::default()
        });
        let run = std::thread::spawn(move || {
            a.set_blocked(0, true).unwrap();
            a.set_blocked(0, false).unwrap();
            assert!(!a.contacts()[0].blocked);
            a.leave(true);
        });
        assert!(run.join().is_ok(), "no panic outside the runtime");
        let _ = std::fs::remove_dir_all(dir);
    }
}
