//! What the Android app sees of `termchat-engine` (ADR-011): an `Account`
//! with plain methods, and a `Listener` the engine calls back.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use termchat_engine::data::{Contact, Status};
use termchat_engine::engine::{self, Setup, next_timer};
use termchat_engine::{Engine, Happened, Unlocked};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

uniffi::setup_scaffolding!();

/// How often the relay's events are taken in.
/// ponytail: polling, 200 ms of latency; await the channel if that shows.
const POLL: Duration = Duration::from_millis(200);

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
}

#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    fn on_event(&self, event: Event);
    /// Something in the account changed: contacts, messages or the link.
    fn on_change(&self);
}

#[derive(uniffi::Record)]
pub struct ContactView {
    pub index: u32,
    pub name: String,
    pub fingerprint: String,
    pub unread: u32,
    pub verified: bool,
    pub blocked: bool,
    pub left: bool,
    /// They asked; Accept or delete.
    pub request: bool,
    pub expires_in: Option<u32>,
    pub no_history: bool,
    pub last_at: u64,
}

#[derive(uniffi::Record)]
pub struct MessageView {
    pub mine: bool,
    pub text: String,
    pub at: u64,
    pub system: bool,
    /// "queued", "sending", "sent", "received", "system" or the failure.
    pub status: String,
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

/// A new identity. Android has no Tor yet (ADR-011).
#[uniffi::export]
pub fn setup(
    dir: String,
    relay_url: String,
    nickname: String,
    password: String,
    listener: Arc<dyn Listener>,
) -> Result<Arc<Account>> {
    let _rt = runtime().enter();
    let setup = Setup {
        relay_url: relay_url.trim(),
        onion_url: None,
        tor: false,
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

    /// Runs `f` on contact `index`, if it exists.
    fn with<T>(&self, index: u32, f: impl FnOnce(&mut Engine, usize) -> T) -> Result<T> {
        let mut e = self.lock();
        let i = usize::try_from(index).unwrap_or(usize::MAX);
        if i >= e.data.contacts.len() {
            return Err(String::from("no such contact").into());
        }
        Ok(f(&mut e, i))
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
    }
}

fn view(e: &Engine, i: usize, c: &Contact) -> ContactView {
    ContactView {
        index: i as u32,
        name: e.name(c),
        fingerprint: termchat_core::fingerprint(&c.key),
        unread: u32::try_from(c.unread).unwrap_or(u32::MAX),
        verified: c.verified,
        blocked: c.blocked,
        left: c.left,
        request: e.core().is_request(&c.key) == Some(true),
        expires_in: c.expires_in,
        no_history: c.no_history,
        last_at: c.last_at(),
    }
}

#[uniffi::export]
impl Account {
    pub fn nickname(&self) -> String {
        self.lock().data.nickname.clone()
    }

    pub fn fingerprint(&self) -> String {
        termchat_core::fingerprint(&self.lock().me())
    }

    pub fn connected(&self) -> bool {
        self.lock().connected
    }

    pub fn backed_up(&self) -> bool {
        self.lock().data.backed_up
    }

    /// A newer termchat the relay runs (ADR-010).
    pub fn update(&self) -> Option<String> {
        self.lock().update.clone()
    }

    pub fn invite(&self) -> String {
        self.lock().invite()
    }

    /// In a stable order: an index stays valid until a contact is deleted.
    /// The app sorts by `last_at` for display.
    pub fn contacts(&self) -> Vec<ContactView> {
        let e = self.lock();
        e.data
            .contacts
            .iter()
            .enumerate()
            .map(|(i, c)| view(&e, i, c))
            .collect()
    }

    pub fn messages(&self, index: u32) -> Result<Vec<MessageView>> {
        self.with(index, |e, i| {
            e.data.contacts[i]
                .messages
                .iter()
                .map(|m| MessageView {
                    mine: m.mine,
                    text: m.text.clone(),
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
                })
                .collect()
        })
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

    pub fn send(&self, index: u32, text: String) -> Result<()> {
        Ok(self.with(index, |e, i| e.send(i, &text))??)
    }

    /// Adds a contact from their invite; returns its index.
    pub fn add(&self, invite: String) -> Result<u32> {
        let i = self.lock().add(invite.trim())?;
        Ok(i as u32)
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

    /// Cycles the disappearing timer like Ctrl+T; returns the new one.
    pub fn next_timer(&self, index: u32) -> Result<Option<u32>> {
        self.with(index, |e, i| {
            e.edit(i, |c| c.expires_in = next_timer(c.expires_in));
            e.data.contacts[i].expires_in
        })
    }

    pub fn set_blocked(&self, index: u32, on: bool) -> Result<()> {
        self.with(index, |e, i| e.edit(i, |c| c.blocked = on))
    }

    pub fn set_verified(&self, index: u32, on: bool) -> Result<()> {
        self.with(index, |e, i| e.edit(i, |c| c.verified = on))
    }

    pub fn set_no_history(&self, index: u32, on: bool) -> Result<()> {
        self.with(index, |e, i| e.edit(i, |c| c.no_history = on))
    }

    pub fn safety_number(&self, index: u32) -> Result<String> {
        self.with(index, |e, i| {
            termchat_core::safety_number(&e.me(), &e.data.contacts[i].key)
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

    /// The UnifiedPush endpoint, or `None` to stop wake-ups (ADR-012).
    pub fn set_push(&self, endpoint: Option<String>) {
        self.lock().set_push(endpoint);
    }

    pub fn push(&self) -> Option<String> {
        self.lock().data.push.clone()
    }

    pub fn news_seen(&self) {
        self.lock().news_seen();
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
            "ffi".into(),
            pw.clone(),
            Arc::new(Quiet),
        )
        .unwrap();
        assert!(a.invite().starts_with("termchat:"));
        assert!(a.contacts().is_empty() && a.add("nonsense".into()).is_err());
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
}
