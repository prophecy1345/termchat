//! TUI state and event handling, without any drawing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termchat_core::proto::{
    self, Body, ClientFrame, Ed25519Key, Invite, ServerFrame, check_nickname,
};
use termchat_core::{Client, Error as CoreError, FileKey, KdfParams, create_file_key};
use tokio::sync::mpsc::Receiver;
use zeroize::Zeroizing;

use crate::data::{self, Contact, Data, Msg, Status};
use crate::{Event, Relay};

/// PLAN.md, M3.
pub const AUTOLOCK: Duration = Duration::from_secs(5 * 60);
const MIN_PASSWORD: usize = 8;
/// Disappearing timer steps for Ctrl+T.
const TIMERS: [Option<u32>; 5] = [None, Some(300), Some(3600), Some(86_400), Some(604_800)];

pub struct App {
    pub screen: Screen,
    pub path: PathBuf,
    pub quit: bool,
}

pub enum Screen {
    Setup(Setup),
    Unlock(Unlock),
    Main(Box<Main>),
}

#[derive(Default)]
pub struct Setup {
    /// relay URL, nickname, password, confirmation.
    pub fields: [Zeroizing<String>; 4],
    pub field: usize,
    pub error: String,
}

#[derive(Default)]
pub struct Unlock {
    pub input: Zeroizing<String>,
    pub error: String,
}

pub enum Popup {
    Help,
    Add(String),
    Invite(String),
    Verify,
    Rename(String),
    Delete,
}

pub struct Main {
    pub core: Arc<Mutex<Client>>,
    key: FileKey,
    pub data: Data,
    relay: Relay,
    pub events: Receiver<Event>,
    pub connected: bool,
    pub selected: usize,
    pub input: String,
    pub popup: Option<Popup>,
    pub status: String,
    pub last_input: Instant,
    path: PathBuf,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl App {
    pub fn new(path: PathBuf) -> Self {
        let screen = match path.exists() {
            true => Screen::Unlock(Unlock::default()),
            false => Screen::Setup(Setup::default()),
        };
        Self {
            screen,
            path,
            quit: false,
        }
    }

    /// Next relay event while unlocked; never resolves otherwise.
    pub async fn relay_event(&mut self) -> Event {
        if let Screen::Main(m) = &mut self.screen
            && let Some(ev) = m.events.recv().await
        {
            return ev;
        }
        std::future::pending().await
    }

    pub fn on_relay(&mut self, ev: Event) {
        if let Screen::Main(m) = &mut self.screen {
            m.on_relay(ev);
        }
    }

    pub fn on_tick(&mut self) {
        if let Screen::Main(m) = &mut self.screen {
            if m.last_input.elapsed() >= AUTOLOCK {
                self.lock();
            } else {
                m.expire();
            }
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c' | 'q')) {
            if let Screen::Main(m) = &mut self.screen {
                m.persist();
            }
            self.quit = true;
            return;
        }
        match &mut self.screen {
            Screen::Setup(s) => {
                if let Some(main) = s.on_key(k, &self.path) {
                    self.screen = Screen::Main(main);
                }
            }
            Screen::Unlock(u) => {
                if let Some(main) = u.on_key(k, &self.path) {
                    self.screen = Screen::Main(main);
                }
            }
            Screen::Main(m) => {
                m.last_input = Instant::now();
                if ctrl && k.code == KeyCode::Char('k') {
                    self.lock();
                } else {
                    m.on_key(k, ctrl);
                }
            }
        }
    }

    /// Saves and drops every key and message from memory (PLAN.md, M3).
    pub fn lock(&mut self) {
        if let Screen::Main(m) = &mut self.screen {
            m.persist();
        }
        self.screen = Screen::Unlock(Unlock::default());
    }
}

/// Text input shared by forms and popups. `true` on Enter.
fn edit(s: &mut String, k: KeyEvent) -> bool {
    match k.code {
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => s.push(c),
        KeyCode::Backspace => {
            s.pop();
        }
        KeyCode::Enter => return true,
        _ => {}
    }
    false
}

impl Setup {
    fn on_key(&mut self, k: KeyEvent, path: &Path) -> Option<Box<Main>> {
        match k.code {
            KeyCode::Tab | KeyCode::Down => self.field = (self.field + 1) % 4,
            KeyCode::BackTab | KeyCode::Up => self.field = (self.field + 3) % 4,
            _ if edit(&mut self.fields[self.field], k) => {
                if self.field < 3 {
                    self.field += 1;
                } else {
                    match self.finish(path) {
                        Ok(main) => return Some(main),
                        Err(e) => self.error = e,
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn finish(&mut self, path: &Path) -> Result<Box<Main>, String> {
        let [url, nickname, password, confirm] = &self.fields;
        let url = url.trim();
        if !(url.starts_with("wss://") || url.starts_with("ws://")) {
            return Err("relay URL must start with wss://".into());
        }
        let nickname = nickname.trim();
        check_nickname(nickname).map_err(|_| "nickname: 1-64 bytes, no control characters")?;
        if password.chars().count() < MIN_PASSWORD {
            return Err(format!("password: at least {MIN_PASSWORD} characters"));
        }
        if password != confirm {
            return Err("passwords do not match".into());
        }
        let key =
            create_file_key(password.as_bytes(), KdfParams::DEFAULT).map_err(|e| e.to_string())?;
        let data = Data {
            relay_url: url.into(),
            nickname: nickname.into(),
            ..Data::default()
        };
        let mut main = Main::open(Client::default(), key, data, path.to_owned())?;
        if !main.persist() {
            return Err(main.status);
        }
        Ok(main)
    }
}

impl Unlock {
    fn on_key(&mut self, k: KeyEvent, path: &Path) -> Option<Box<Main>> {
        if !edit(&mut self.input, k) {
            return None;
        }
        // Argon2 blocks the UI thread for ~1 s; move to spawn_blocking if it bothers.
        let result = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| match Client::load(self.input.as_bytes(), &bytes) {
                Err(CoreError::WrongPassword) => Err("wrong password".into()),
                other => other.map_err(|e| e.to_string()),
            })
            .and_then(|(core, key, app)| {
                let data = Data::from_json(&app).map_err(|e| e.to_string())?;
                Main::open(core, key, data, path.to_owned())
            });
        self.input = Zeroizing::default();
        match result {
            Ok(main) => Some(main),
            Err(e) => {
                self.error = e;
                None
            }
        }
    }
}

impl Main {
    fn open(core: Client, key: FileKey, data: Data, path: PathBuf) -> Result<Box<Self>, String> {
        let identity = core.identity();
        let core = Arc::new(Mutex::new(core));
        let signer = Arc::clone(&core);
        let sign = Box::new(move |host: &str, nonce: &[u8; 32]| {
            signer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .auth_signature(host, nonce)
        });
        let (relay, events) =
            Relay::connect(&data.relay_url, identity, sign).map_err(|_| "bad relay URL")?;
        Ok(Box::new(Self {
            core,
            key,
            data,
            relay,
            events,
            connected: false,
            selected: 0,
            input: String::new(),
            popup: None,
            status: String::new(),
            last_input: Instant::now(),
            path,
        }))
    }

    pub fn core(&self) -> MutexGuard<'_, Client> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn me(&self) -> Ed25519Key {
        self.core().identity().ed25519
    }

    pub fn invite(&self) -> String {
        Invite {
            version: proto::VERSION,
            relay_url: self.data.relay_url.clone(),
            identity: self.core().identity(),
        }
        .encode()
    }

    /// Must run after every change to the ratchet or the data.
    fn persist(&mut self) -> bool {
        let saved = self
            .core()
            .save(&self.key, &self.data.to_json())
            .map_err(|e| e.to_string())
            .and_then(|bytes| data::write_atomic(&self.path, &bytes).map_err(|e| e.to_string()));
        if let Err(e) = &saved {
            self.status = format!("SAVE FAILED: {e}");
        }
        saved.is_ok()
    }

    fn index(&self, key: &Ed25519Key) -> Option<usize> {
        self.data.contacts.iter().position(|c| c.key == *key)
    }

    fn system(&mut self, i: usize, text: impl Into<String>) {
        self.data.contacts[i].messages.push(Msg {
            mine: false,
            text: text.into(),
            at: now(),
            expires_at: None,
            status: Status::System,
        });
    }

    /// Encrypts, saves the advanced ratchet, then hands the envelope to the transport.
    fn transmit(&mut self, key: &Ed25519Key, body: Body) -> Result<u64, String> {
        let envelope = self.core().encrypt(key, body).map_err(|e| e.to_string())?;
        let client_id = self.data.next_client_id;
        self.data.next_client_id += 1;
        self.persist();
        self.relay
            .send(ClientFrame::Send {
                client_id,
                envelope,
            })
            .map_err(|_| "relay link closed")?;
        Ok(client_id)
    }

    fn claim(&mut self, key: Ed25519Key) {
        let _ = self.relay.send(ClientFrame::ClaimKey { target: key });
    }

    /// Sends our contact request (if still owed) and every queued message.
    fn flush(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        if self.data.contacts[i].request_pending {
            let body = Body::ContactRequest {
                identity: self.core().identity(),
                nickname: self.data.nickname.clone(),
                note: String::new(),
            };
            match self.transmit(&key, body) {
                Ok(_) => self.data.contacts[i].request_pending = false,
                Err(e) => return self.status = e,
            }
        }
        let expires_in = self.data.contacts[i].expires_in;
        for m in 0..self.data.contacts[i].messages.len() {
            let msg = &self.data.contacts[i].messages[m];
            if msg.status != Status::Queued {
                continue;
            }
            let body = Body::Text {
                sent_at: msg.at,
                expires_in,
                text: msg.text.clone(),
            };
            self.data.contacts[i].messages[m].status = match self.transmit(&key, body) {
                Ok(id) => Status::Sending(id),
                Err(e) => Status::Failed(e),
            };
        }
        self.persist();
    }

    fn on_relay(&mut self, ev: Event) {
        match ev {
            Event::Connected => {
                self.connected = true;
                let waiting: Vec<_> = self
                    .data
                    .contacts
                    .iter()
                    .filter(|c| {
                        c.request_pending || c.messages.iter().any(|m| m.status == Status::Queued)
                    })
                    .map(|c| c.key)
                    .collect();
                for key in waiting {
                    let ready = self.core().has_session(&key);
                    match ready {
                        true => {
                            if let Some(i) = self.index(&key) {
                                self.flush(i);
                            }
                        }
                        false => self.claim(key),
                    }
                }
            }
            Event::Disconnected => self.connected = false,
            Event::Frame(frame) => self.on_frame(frame),
        }
    }

    fn on_frame(&mut self, frame: ServerFrame) {
        match frame {
            ServerFrame::KeyCount {
                one_time_left,
                fallback_used,
            } => {
                let rotate = fallback_used || !self.data.fallback_published;
                let keys = self.core().prekeys(one_time_left as usize, rotate);
                if !keys.one_time.is_empty() || keys.fallback.is_some() {
                    self.data.fallback_published = true;
                    self.persist();
                    let _ = self.relay.send(ClientFrame::PublishKeys {
                        one_time: keys.one_time,
                        fallback: keys.fallback,
                    });
                }
            }
            ServerFrame::ClaimedKey { target, key } => {
                let Some(i) = self.index(&target) else { return };
                if !self.core().has_session(&target) {
                    let started = self.core().start_session(&target, &key);
                    if let Err(e) = started {
                        return self.status = format!("cannot start session: {e}");
                    }
                    self.persist();
                }
                self.flush(i);
            }
            ServerFrame::Deliver {
                relay_id,
                sender,
                envelope,
            } => {
                self.receive(sender, &envelope);
                let _ = self.relay.send(ClientFrame::Ack { relay_id });
            }
            ServerFrame::Accepted { client_id } => self.set_status(client_id, Status::Sent),
            ServerFrame::Error {
                client_id: Some(id),
                code,
            } => self.set_status(id, Status::Failed(format!("{code:?}"))),
            ServerFrame::Error {
                client_id: None,
                code,
            } => self.status = format!("relay: {code:?}"),
            ServerFrame::Challenge { .. } | ServerFrame::AuthOk => {}
        }
    }

    fn set_status(&mut self, client_id: u64, status: Status) {
        for m in self
            .data
            .contacts
            .iter_mut()
            .flat_map(|c| c.messages.iter_mut())
        {
            if m.status == Status::Sending(client_id) {
                m.status = status;
                self.persist();
                return;
            }
        }
    }

    /// Decrypts and stores one delivery. Always saved before the caller acks it.
    fn receive(&mut self, sender: Ed25519Key, envelope: &proto::Envelope) {
        let had_session = self.core().has_session(&sender);
        let result = self.core().decrypt(&sender, envelope);
        let got = match result {
            Ok(got) => got,
            Err(CoreError::Duplicate) => return,
            Err(e) => {
                if let Some(i) = self.index(&sender) {
                    self.system(i, format!("a message could not be decrypted ({e})"));
                    self.persist();
                }
                return;
            }
        };
        let i = match self.index(&sender) {
            Some(i) => i,
            None => {
                self.data.contacts.push(Contact {
                    key: sender,
                    ..Default::default()
                });
                self.data.contacts.len() - 1
            }
        };
        if got.new_session && had_session {
            self.system(
                i,
                "⚠ they restarted the conversation (restored a backup?); messages may be lost",
            );
        }
        if got.missed > 0 {
            self.system(i, format!("⚠ {} message(s) missing so far", got.missed));
        }
        match got.body {
            Body::Text {
                expires_in, text, ..
            } => {
                let at = now();
                self.data.contacts[i].messages.push(Msg {
                    mine: false,
                    text,
                    at,
                    expires_at: expires_in.map(|s| at + u64::from(s)),
                    status: Status::Received,
                });
            }
            Body::ContactRequest { note, .. } => {
                let text = match self.core().is_request(&sender) {
                    Some(true) => format!(
                        "contact request{}  —  Ctrl+Y accept, Ctrl+D reject",
                        match note.is_empty() {
                            true => String::new(),
                            false => format!(": {note}"),
                        }
                    ),
                    _ => "connected".into(),
                };
                self.system(i, text);
            }
            Body::ContactAccept { .. } => self.system(i, "accepted your request"),
        }
        if i != self.selected {
            self.data.contacts[i].unread += 1;
        }
        self.persist();
    }

    /// Drops messages whose disappearing timer ran out.
    fn expire(&mut self) {
        let t = now();
        let mut changed = false;
        for c in &mut self.data.contacts {
            let before = c.messages.len();
            c.messages.retain(|m| m.expires_at.is_none_or(|e| e > t));
            changed |= c.messages.len() != before;
        }
        if changed {
            self.persist();
        }
    }

    pub fn current(&self) -> Option<&Contact> {
        self.data.contacts.get(self.selected)
    }

    fn on_key(&mut self, k: KeyEvent, ctrl: bool) {
        if let Some(popup) = self.popup.take() {
            return self.on_popup_key(popup, k);
        }
        let key = self.current().map(|c| c.key);
        match (ctrl, k.code) {
            (false, KeyCode::F(1)) => self.popup = Some(Popup::Help),
            (false, KeyCode::Char('?')) if self.input.is_empty() => self.popup = Some(Popup::Help),
            (false, KeyCode::Up) => self.select(self.selected.saturating_sub(1)),
            (false, KeyCode::Down) => self.select(self.selected + 1),
            (true, KeyCode::Char('a')) => self.popup = Some(Popup::Add(String::new())),
            (true, KeyCode::Char('o')) => self.popup = Some(Popup::Invite(self.invite())),
            (true, KeyCode::Char('f')) if key.is_some() => self.popup = Some(Popup::Verify),
            (true, KeyCode::Char('r')) if key.is_some() => {
                self.popup = Some(Popup::Rename(String::new()))
            }
            (true, KeyCode::Char('d')) if key.is_some() => self.popup = Some(Popup::Delete),
            (true, KeyCode::Char('y')) => {
                if let Some(key) = key
                    && self.core().accept(&key)
                {
                    let body = Body::ContactAccept {
                        nickname: self.data.nickname.clone(),
                    };
                    match self.transmit(&key, body) {
                        Ok(_) => self.system(self.selected, "you accepted the request"),
                        Err(e) => self.status = e,
                    }
                }
            }
            (true, KeyCode::Char('t')) => {
                if let Some(c) = self.data.contacts.get_mut(self.selected) {
                    let at = TIMERS.iter().position(|t| *t == c.expires_in).unwrap_or(0);
                    c.expires_in = TIMERS[(at + 1) % TIMERS.len()];
                    self.persist();
                }
            }
            (true, KeyCode::Char('e')) => {
                if let Some(c) = self.data.contacts.get_mut(self.selected) {
                    c.no_history = !c.no_history;
                    self.persist();
                }
            }
            (false, _) if edit(&mut self.input, k) => self.send(),
            _ => {}
        }
    }

    fn select(&mut self, i: usize) {
        if i < self.data.contacts.len() {
            self.selected = i;
            self.data.contacts[i].unread = 0;
        }
    }

    fn send(&mut self) {
        let text = std::mem::take(&mut self.input);
        let Some(c) = self.current() else {
            return self.status = "add a contact first (Ctrl+A)".into();
        };
        if text.trim().is_empty() {
            return;
        }
        if text.len() > proto::MAX_TEXT_BYTES {
            self.input = text;
            return self.status = "message too long".into();
        }
        let (key, expires_in, i) = (c.key, c.expires_in, self.selected);
        let at = now();
        self.data.contacts[i].messages.push(Msg {
            mine: true,
            text,
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Queued,
        });
        let ready = self.core().has_session(&key);
        match ready {
            true => self.flush(i),
            false => {
                self.persist();
                self.claim(key);
            }
        }
    }

    fn on_popup_key(&mut self, popup: Popup, k: KeyEvent) {
        let keep = match popup {
            Popup::Help => None,
            Popup::Invite(invite) => {
                if k.code == KeyCode::Char('c') {
                    copy_to_clipboard(&invite);
                    self.status = "invite copied (if your terminal supports OSC 52)".into();
                }
                None
            }
            Popup::Verify => {
                if k.code == KeyCode::Char('v')
                    && let Some(c) = self.data.contacts.get_mut(self.selected)
                {
                    c.verified = !c.verified;
                    self.persist();
                }
                None
            }
            Popup::Delete => {
                if k.code == KeyCode::Char('y') && self.selected < self.data.contacts.len() {
                    let c = self.data.contacts.remove(self.selected);
                    self.core().remove_contact(&c.key);
                    self.selected = self.selected.saturating_sub(1);
                    self.persist();
                }
                None
            }
            Popup::Add(mut s) if k.code != KeyCode::Esc => match edit(&mut s, k) {
                true => {
                    if let Err(e) = self.add(&s) {
                        self.status = e;
                    }
                    None
                }
                false => Some(Popup::Add(s)),
            },
            Popup::Rename(mut s) if k.code != KeyCode::Esc => match edit(&mut s, k) {
                true => {
                    let name = s.trim();
                    if let Some(c) = self.data.contacts.get_mut(self.selected) {
                        c.petname = (!name.is_empty()).then(|| name.chars().take(64).collect());
                    }
                    self.persist();
                    None
                }
                false => Some(Popup::Rename(s)),
            },
            Popup::Add(_) | Popup::Rename(_) => None,
        };
        self.popup = keep;
    }

    /// Adds a contact from an invite string and starts the handshake.
    fn add(&mut self, invite: &str) -> Result<(), String> {
        let invite = Invite::decode(invite).map_err(|e| format!("invalid invite: {e}"))?;
        if invite.relay_url.trim_end_matches('/') != self.data.relay_url.trim_end_matches('/') {
            return Err(format!(
                "they use another relay ({}); only one relay is supported for now",
                invite.relay_url
            ));
        }
        let key = invite.identity.ed25519;
        if key == self.me() {
            return Err("that is your own invite".into());
        }
        self.core()
            .add_contact(&invite.identity)
            .map_err(|e| format!("invalid invite: {e}"))?;
        let i = match self.index(&key) {
            Some(i) => i,
            None => {
                self.data.contacts.push(Contact {
                    key,
                    request_pending: true,
                    ..Default::default()
                });
                self.data.contacts.len() - 1
            }
        };
        self.select(i);
        self.persist();
        self.claim(key);
        Ok(())
    }
}

/// OSC 52: the terminal puts the text on the system clipboard.
fn copy_to_clipboard(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
}
