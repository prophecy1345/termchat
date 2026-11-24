//! An unlocked account: the state, the relay link and everything that changes
//! them. The interface calls methods and drains `happened` afterwards.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use termchat_core::proto::{
    self, Body, ClientFrame, Ed25519Key, ErrorCode, INVITE_VERSION, Invite, InviteCode, QueueId,
    Ref, ServerFrame, check_nickname,
};
use termchat_core::{
    Client, Error as CoreError, FileKey, KdfParams, create_file_key, set_duress, store,
};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_tungstenite::tungstenite::http::Uri;
use zeroize::Zeroize;

use crate::data::{self, Addr, Contact, Data, FileMsg, Msg, Queue, Status};

#[path = "devices.rs"]
mod devices;
#[path = "group.rs"]
mod group;
#[path = "intro.rs"]
mod intro;
pub use intro::{IntroAnswer, PendingIntro};
#[path = "large.rs"]
mod large;
#[path = "receipts.rs"]
mod receipts;
use crate::{Bridges, Dial, Error, Event, Relay, parse_version, password};
pub use group::GroupScriptError;
use subtle::ConstantTimeEq;

/// The `Understands` level that reads answers, edits and reactions (ADR-025).
const RICH_LEVEL: u16 = 1;

/// How long a wipe waits for the relay to take the goodbyes (ADR-009).
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);
const NOT_REMOVED: &str =
    "The relay did not confirm deleting your queues; it forgets unused ones after 90 days.";
/// ADR-023: a contact from before anonymous queues, with no way to reach them.
/// What `take_new` found: a new message, or a change to one (ADR-025).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetched {
    Message,
    Edit,
    Delete,
    React,
    /// Ours reached the contact, or they read it (ADR-041).
    Delivered,
    Read,
}

/// `proto::REACTIONS` by index, as words for scripts and screen readers.
pub const REACTION_NAMES: [&str; proto::REACTIONS as usize] =
    ["like", "love", "laugh", "wow", "sad", "thanks", "dislike"];

/// ADR-025: where answers, edits, deletes and reactions are not possible yet.
pub const OLDER_APP: &str = "their app is older; answers and reactions come after they update";

pub const NEEDS_INVITE: &str =
    "this contact needs a new invite: ask them for one, or send them yours";
/// One-time keys kept on each contact queue (ADR-023, amendment).
const CONTACT_KEYS: u32 = 2;
/// ADR-007: what a not-yet-accepted request may leave behind.
const MAX_HIDDEN: usize = 5;
const MAX_REJECTED: usize = 1000;
/// ADR-017: file chunks go in a burst of this many, then one per `FILE_EVERY`,
/// so the relay's send limit (30, then one per 2 s) never refuses one.
const FILE_BURST: f64 = 10.0;
const FILE_EVERY: f64 = 3.0;
/// ADR-019: a fallback key is replaced at this age, used or not, so a relay
/// that hides its use cannot keep one key in service.
const FALLBACK_MAX_AGE: u64 = 7 * 86_400;
/// ADR-003: a typo in the real password must never hit the duress one.
pub const DURESS_DISTANCE: usize = 4;
/// The project's public relay, prefilled on first run; any other relay can be
/// typed over it (docs/self-host.md).
pub const DEFAULT_RELAY: &str = "wss://relay.termchat.app/";
pub const DEFAULT_ONION: &str =
    "ws://rgisnh6oixs4usvi544poxwzxstbxls74x4mtzlwl4a55uuncrpolpad.onion/";

/// Disappearing timer steps, in the order a button cycles them.
pub const TIMERS: [Option<u32>; 5] = [None, Some(300), Some(3600), Some(86_400), Some(604_800)];

pub fn next_timer(now: Option<u32>) -> Option<u32> {
    let at = TIMERS.iter().position(|t| *t == now).unwrap_or(0);
    TIMERS[(at + 1) % TIMERS.len()]
}

/// One-time codes kept unused; older ones stop working (ADR-013).
const MAX_ONE_TIME_CODES: usize = 20;

fn new_code() -> InviteCode {
    let mut code = [0; 16];
    // getrandom fails only on a broken OS; an all-zero code would still be
    // one shared, guessable code, so refuse to go on.
    if getrandom::fill(&mut code).is_err() {
        std::process::abort();
    }
    code
}

/// This version's notes and the ones before, from CHANGELOG.md.
pub fn news() -> &'static str {
    const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
    &CHANGELOG[CHANGELOG.find("## ").unwrap_or(0)..]
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How long ago `at` was, short: "now", "5m", "3h", "2d".
pub fn age(at: u64) -> String {
    match now().saturating_sub(at) {
        0..60 => "now".into(),
        s => span(s),
    }
}

/// A duration, short and rounded down: "59m", "2h", "3d".
/// ADR-037: the start of a safety-number QR code.
pub const SAFETY_CODE: &str = "TERMCHAT-SAFETY:";

/// `2026-11-21 14:03 UTC` for Unix seconds, with no calendar library: the
/// days-to-civil-date arithmetic of Howard Hinnant's `civil_from_days`.
pub fn utc_minute(at: u64) -> String {
    let days = i64::try_from(at / 86_400).unwrap_or(i64::MAX / 2) + 719_468;
    let (era, doe) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let secs = at % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3600,
        secs / 60 % 60
    )
}

/// Plain text of a chat for an export the person asked for: a title, a
/// warning, then each message with its time and who wrote it.
fn export_text(title: &str, msgs: &[Msg], who: impl Fn(&Msg) -> String) -> String {
    let mut out = format!(
        "{title}, exported {}\nThis file is not encrypted: keep it somewhere safe, or delete it.\n\n",
        utc_minute(now())
    );
    for m in msgs.iter().filter(|m| m.status != Status::System) {
        let body = match (&m.file, m.deleted) {
            (_, true) => "(deleted)".to_string(),
            (Some(f), _) => format!("[document {}]", clean(&f.label())),
            (None, _) => clean_text(&m.text).replace('\n', "\n  "),
        };
        let edited = if m.edited { " (edited)" } else { "" };
        out.push_str(&format!(
            "{}  {}: {body}{edited}\n",
            utc_minute(m.at),
            who(m)
        ));
    }
    out
}

/// Whether a chat muted until `until` (Unix seconds, `u64::MAX` for always) is quiet now.
pub fn muted(until: Option<u64>) -> bool {
    until.is_some_and(|t| t > now())
}

/// The next step of a chat's mute: off, an hour, a day, always, off again.
pub fn next_mute(until: Option<u64>) -> Option<u64> {
    let now = now();
    match until.filter(|&t| t > now) {
        None => Some(now + 3600),
        Some(u64::MAX) => None,
        Some(t) if t - now <= 3600 => Some(now + 86_400),
        Some(_) => Some(u64::MAX),
    }
}

/// How a chat's mute reads: "off", "always", or the time left ("23h").
pub fn mute_label(until: Option<u64>) -> String {
    match until.filter(|_| muted(until)) {
        None => "off".into(),
        Some(u64::MAX) => "always".into(),
        Some(t) => format!("for {}", span(t.saturating_sub(now()).max(60))),
    }
}

pub fn span(s: u64) -> String {
    match s {
        0..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// What the interface should show after a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Happened {
    /// A new message (`request: false`) or contact request to point out.
    Alert { key: Ed25519Key, request: bool },
    /// One line for the status bar: a problem, or the result of an action.
    Note(String),
    /// Worth a window of its own.
    Notice(String),
    /// Everything is erased; show the first-run screen with this note.
    Wiped(String),
    /// A wipe could not tell the relay: the reason, then ask "wipe anyway?".
    WipeAnyway(String),
    /// An invite asked for earlier is ready: its queue exists now (ADR-023).
    Invite(String),
    /// A new message in a group (ADR-027).
    GroupAlert {
        group: termchat_core::proto::GroupId,
    },
}

/// Everything the first-run form asks.
/// The nickname of a device set up only to be linked: the main device's
/// replaces it, so nobody is asked to make one up (ADR-028).
pub const NEW_DEVICE_NICKNAME: &str = "new device";

pub struct Setup<'a> {
    pub relay_url: &'a str,
    pub onion_url: Option<&'a str>,
    pub tor: bool,
    pub nickname: &'a str,
    pub password: &'a str,
}

pub enum Unlocked {
    Open(Box<Engine>),
    /// The duress password was entered and the data is gone.
    Wiped,
}

pub struct Engine {
    /// The main identity.
    pub core: Arc<Mutex<Client>>,
    key: FileKey,
    pub data: Data,
    /// Slot 0 is the main identity, slot `n` is `data.profiles[n - 1]` (ADR-014).
    cores: Vec<Arc<Mutex<Client>>>,
    /// One connection per identity (its invite queue) and per contact
    /// (ADR-023, section 7), and per other relay a contact's queue is on.
    links: BTreeMap<Link, Live>,
    group_writes: BTreeMap<proto::GroupId, Arc<AtomicBool>>,
    group_move_retries: BTreeMap<proto::GroupId, Instant>,
    /// Groups whose reader's backlog came in since the link came up: only
    /// then do we write to them (ADR-042, amendment 3).
    group_read: BTreeSet<proto::GroupId>,
    /// Groups whose reader we asked to read and the relay has not answered.
    group_subscribing: BTreeSet<proto::GroupId>,
    /// Links asked to delete their queue, until the relay confirms.
    /// With the `DeleteQueue` sent on it, to send again if a new link for
    /// the same contact and relay starts before this one is done.
    retiring: Vec<(Link, Live, Instant, ClientFrame)>,
    /// Every link's relay events, tagged with the link.
    pub events: Receiver<RelayEvent>,
    events_tx: Sender<RelayEvent>,
    /// The main identity's invite link is connected.
    pub connected: bool,
    /// A newer version the relay runs (ADR-010).
    pub update: Option<String>,
    /// The chat on screen: its messages do not count as unread.
    pub open: Option<Ed25519Key>,
    /// The group on screen, likewise.
    pub open_group: Option<termchat_core::proto::GroupId>,
    /// ADR-041: send delivery receipts; on unless the settings say no.
    pub receipts: bool,
    /// ADR-041: send read receipts, and show the contacts'; off by default.
    pub read_receipts: bool,
    pub happened: Vec<Happened>,
    path: PathBuf,
    leaving: Option<Leaving>,
    /// Erased: nothing may write the state file again.
    gone: bool,
    /// File chunks we may send now, and when that was counted (ADR-017).
    chunk_budget: (f64, Instant),
    /// The same for parts of documents sent on request (ADR-043).
    large_budget: (f64, Instant),
    /// The last part sent, for relays that pace parts with text.
    slow_part_at: Instant,
    /// Chunks before a file's last one, still unanswered, and whose they are:
    /// no message carries their `client_id` (ADR-020).
    chunks: BTreeMap<u64, Ed25519Key>,
    heartbeat: Option<(Ed25519Key, u64)>,
    /// Why links could not start at the last `start_links`, said once.
    unreachable: BTreeSet<String>,
    /// The relay had no key to give: claim again at this time.
    reclaim: Option<Instant>,
    /// Single devices of contacts whose key we asked for (ADR-028): once each.
    pub(super) claiming: BTreeSet<Ed25519Key>,
}

/// One connection: whose it is (one of our identities, for its invite
/// queue, or a contact) and the relay it goes to (ADR-023).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Link {
    pub who: Ed25519Key,
    pub relay: String,
}

/// A relay event and the link it came on.
pub type RelayEvent = (Link, Event);

/// One link's connection. Dropping it closes the connection.
struct Live {
    relay: Relay,
    connected: bool,
    /// Whose `CreateQueue` frames are out, in order: the relay answers in order.
    creating: Vec<Ed25519Key>,
    /// `Subscribe` frames waiting for their answer: an unknown queue then is ours, gone.
    subscribing: Vec<QueueId>,
    /// An unanswered heartbeat per contact, retained across reconnects of this link.
    heartbeats: BTreeMap<Ed25519Key, u64>,
    /// The relay's version, from its last handshake (ADR-010).
    version: Option<(u32, u32, u32)>,
    forward: tokio::task::AbortHandle,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.forward.abort();
    }
}

/// After `NoKeys`, when to claim again.
const RECLAIM_AFTER: Duration = Duration::from_secs(15);
/// How long a queue being deleted may take (ADR-014, ADR-023).
const RETIRE_TIMEOUT: Duration = Duration::from_secs(60);
/// Incognito identities whose invite nobody used yet.
const MAX_WAITING_PROFILES: usize = 5;
const MISSING_SUFFIX: &str = " message(s) missing so far";
const MISSING_PROFILE: &str = "this contact's incognito identity is missing";

/// Goodbyes sent and not yet accepted, then every queue deleted (ADR-009).
struct Leaving {
    waiting: Vec<u64>,
    until: Instant,
    deleting: bool,
    /// Queues the relay has not yet confirmed deleting.
    queues: Vec<QueueId>,
}

/// Plain `ws://` would show the relay traffic (and so who we talk to) to the
/// network; it is only fine where TLS adds nothing: the same machine or an
/// onion service, which authenticates and encrypts on its own.
pub fn relay_url_ok(url: &str) -> bool {
    let Some(host) = ws_host(url) else {
        return false;
    };
    // The address used without Tor: an onion one goes in its own field (ADR-004).
    !host.ends_with(".onion")
        && (url.starts_with("wss://") || matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1"))
}

fn onion_url_ok(url: &str) -> bool {
    url.starts_with("ws://") && ws_host(url).is_some_and(|h| h.ends_with(".onion"))
}

/// The host the transport will actually connect to. Parsed the same way as in
/// `Relay::connect`, so userinfo, a query or a fragment cannot smuggle a
/// different host past the checks above.
fn ws_host(url: &str) -> Option<String> {
    let uri: Uri = url.parse().ok()?;
    if !matches!(uri.scheme_str(), Some("ws" | "wss")) {
        return None;
    }
    let host = uri.host()?.trim_start_matches('[').trim_end_matches(']');
    Some(host.to_owned())
}

/// A relay address from a peer (invite or `QueueOffer`), fit to connect to.
fn addr_ok(relay_url: &str, onion_url: Option<&str>) -> bool {
    relay_url_ok(relay_url) && onion_url.is_none_or(onion_url_ok)
}

fn loopback(url: &str) -> bool {
    ws_host(url).is_some_and(|h| matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1"))
}

impl Engine {
    /// A relay address from a peer or an invite (review 2026-11c, findings 3
    /// and 7): `wss://`, an onion `ws://` beside it, and this machine only if
    /// our own relay is here too (tests, a relay on the same computer); never
    /// a request from a peer to a service on our own machine.
    pub(crate) fn peer_addr_ok(&self, relay_url: &str, onion_url: Option<&str>) -> bool {
        peer_addr_ok(&self.data.relay_url, relay_url, onion_url)
    }
}

/// A message clearing a chat keeps: not sent yet, or a document still arriving.
fn on_its_way(m: &Msg) -> bool {
    matches!(m.status, Status::Queued | Status::Sending(_))
        || m.file.as_ref().is_some_and(|f| !m.mine && !f.complete())
}

fn peer_addr_ok(ours: &str, relay_url: &str, onion_url: Option<&str>) -> bool {
    addr_ok(relay_url, onion_url) && (!loopback(relay_url) || loopback(ours))
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Text from a peer or the network, safe to show on one line: no control
/// characters and no bidi overrides that could reorder what is around it.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .filter(|&c| !proto::is_unsafe_char(c))
        .collect()
}

/// A message body, safe to show: like [`clean`], but line breaks stay.
pub fn clean_text(s: &str) -> String {
    s.split('\n').map(clean).collect::<Vec<_>>().join("\n")
}

/// A received file's name, safe to create in a folder the user picked
/// (ADR-017). The sender chose it, so it must not come out hidden (`.zshenv`
/// in a home folder runs with the next shell) or, on Windows, as a device
/// (`CON`), a stream of another file (`a.txt:b`) or a name Windows trims.
pub fn safe_name(name: &str) -> String {
    let name: String = clean(name)
        .chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' | '/' | '\\' => '_',
            c => c,
        })
        .collect();
    let name = name.trim().trim_end_matches(['.', ' ']);
    let stem = name.split('.').next().unwrap_or_default().to_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit() && c != '0'));
    match name.is_empty() || name.starts_with('.') || device {
        true => format!("_{name}"),
        false => name.to_string(),
    }
}

fn why(code: ErrorCode) -> String {
    match code {
        ErrorCode::UnknownQueue => "their queue is gone: ask them for a new invite".into(),
        ErrorCode::UnsupportedVersion => "the relay runs another protocol version".into(),
        code => format!("{code:?}"),
    }
}

/// A backup is a state file without sessions or messages: restoring is a copy.
pub fn restore(state: &Path, backup: &Path) -> Result<(), String> {
    let bytes = std::fs::read(backup).map_err(|e| format!("cannot read backup: {e}"))?;
    data::write_atomic(state, &bytes).map_err(|e| e.to_string())?;
    // ADR-003: the copy brought onto this device is erased with it.
    data::remember_backup(state, backup, &bytes).map_err(|e| e.to_string())
}

/// Starts one link: through Tor only if Tor is on (ADR-004), then to the
/// relay's onion address. Its events go to `tx` tagged with the link, after
/// `delay`; every reconnect waits a random 0-30 s more (ADR-014).
fn start(
    link: &Link,
    onion: Option<&str>,
    data: &Data,
    path: &Path,
    tx: &Sender<RelayEvent>,
    delay: Duration,
) -> Result<Live, String> {
    let (url, dial) = match (onion, data.tor) {
        (Some(onion), true) => {
            let bridges = Bridges {
                lines: data.bridges.clone(),
                program: pt_program(data),
            };
            let dir = data::tor_dir_for(path, data.tor_generation);
            (onion, Dial::Tor(dir, bridges))
        }
        (None, true) => return Err("Tor is on but the relay has no onion URL".into()),
        (_, false) => (link.relay.as_str(), Dial::Direct),
    };
    // ADR-038: without Tor the relay ties the links together by IP anyway,
    // so a spread before each reconnect would only delay messages.
    let (relay, mut rx) =
        Relay::connect_after(url, dial, delay, data.tor).map_err(|e| match e {
            Error::NoTor => "this build has no Tor support".to_string(),
            _ => format!("bad relay URL: {url}"),
        })?;
    // Stops with the engine (its receiver) or with this `Live` (abort).
    let tx = tx.clone();
    let me = link.clone();
    let forward = tokio::spawn(async move {
        loop {
            tokio::select! {
                ev = rx.recv() => {
                    let Some(ev) = ev else { return };
                    if tx.send((me.clone(), ev)).await.is_err() {
                        return;
                    }
                }
                _ = tx.closed() => return,
            }
        }
    })
    .abort_handle();
    Ok(Live {
        relay,
        connected: false,
        creating: Vec::new(),
        subscribing: Vec::new(),
        heartbeats: BTreeMap::new(),
        version: None,
        forward,
    })
}

/// Where the bridge transport program is (ADR-015): the setting, an app's
/// own copy (Android: [`set_pt_program`]), next to this program, or `PATH`.
pub fn pt_program(data: &Data) -> Option<PathBuf> {
    if let Some(p) = &data.pt_program {
        return Some(p.into());
    }
    if let Some(p) = PT_PROGRAM.get() {
        return Some(p.clone());
    }
    let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe()
        .ok()?
        .with_file_name(exe("lyrebird"));
    if beside.is_file() {
        return Some(beside);
    }
    let path = std::env::var_os("PATH")?;
    ["lyrebird", "obfs4proxy"].iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(exe(name)))
            .find(|p| p.is_file())
    })
}

static PT_PROGRAM: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The app's own transport program, e.g. in Android's native library folder.
pub fn set_pt_program(path: PathBuf) {
    let _ = PT_PROGRAM.set(path);
}

/// Latin, Cyrillic and Greek letters together: "Аlice" with a Cyrillic А
/// looks like "Alice" (ADR-007, section 5). Such a name always shows its
/// fingerprint digits.
fn mixed_scripts(name: &str) -> bool {
    let script = |c: char| match c {
        'a'..='z' | 'A'..='Z' | '\u{00C0}'..='\u{024F}' => Some(0),
        '\u{0370}'..='\u{03FF}' => Some(1),
        '\u{0400}'..='\u{04FF}' => Some(2),
        _ => None,
    };
    let mut seen = name.chars().filter_map(script);
    seen.next().is_some_and(|first| seen.any(|s| s != first))
}

/// 0-30 s, so a device's identities do not connect in the same second (ADR-014).
fn spread() -> Duration {
    Duration::from_millis(u64::from(getrandom::u32().unwrap_or(0) % 30_000))
}

impl Engine {
    /// A new identity, saved to `path`.
    pub fn setup(path: &Path, s: &Setup) -> Result<Box<Self>, String> {
        if !relay_url_ok(s.relay_url) {
            return Err(
                "relay URL must start with wss:// (ws:// only for localhost or .onion)".into(),
            );
        }
        if s.onion_url.is_some_and(|o| !onion_url_ok(o)) {
            return Err("onion URL looks like ws://xxxx.onion/".into());
        }
        if s.tor && s.onion_url.is_none() {
            return Err("Tor needs the relay's onion URL (or answer n)".into());
        }
        if s.tor && !cfg!(feature = "tor") {
            return Err("this build has no Tor support; answer n".into());
        }
        check_nickname(s.nickname).map_err(|_| "nickname: 1-64 bytes, no control characters")?;
        if let Some(why) = password::weakness(s.password, &[s.nickname]) {
            return Err(why);
        }
        let key = create_file_key(s.password.as_bytes(), KdfParams::DEFAULT)
            .map_err(|e| e.to_string())?;
        let data = Data {
            relay_url: s.relay_url.into(),
            onion_url: s.onion_url.map(Into::into),
            tor: s.tor,
            nickname: s.nickname.into(),
            news_seen: env!("CARGO_PKG_VERSION").into(),
            understands_resent: true,
            ..Data::default()
        };
        let mut engine = Self::open(Client::default(), key, data, path.to_owned())?;
        if !engine.persist() {
            return Err(engine.take_note());
        }
        Ok(engine)
    }

    /// Opens the state file. The duress password erases it first (ADR-003).
    pub fn unlock(path: &Path, password: &[u8]) -> Result<Unlocked, String> {
        // Argon2 blocks the caller for ~1 s.
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read state: {e}"))?;
        match Client::load(password, &bytes) {
            // Erase first, say nothing.
            Err(CoreError::Duress) => {
                // If shredding fails (read-only, locked file), still cut the
                // state to nothing: the screen must look fresh either way
                // (ADR-003), so the data must not survive behind it.
                if data::wipe(path).is_err() {
                    let cut = std::fs::OpenOptions::new().write(true).open(path);
                    let _ = cut.and_then(|f| f.set_len(0).and_then(|()| f.sync_all()));
                    let _ = std::fs::remove_file(path);
                }
                Ok(Unlocked::Wiped)
            }
            Err(CoreError::WrongPassword) => Err("wrong password".into()),
            Err(e) => Err(e.to_string()),
            Ok((core, key, app)) => {
                let data = Data::from_json(&app).map_err(|e| e.to_string())?;
                Self::open(core, key, data, path.to_owned()).map(Unlocked::Open)
            }
        }
    }

    fn open(
        core: Client,
        key: FileKey,
        mut data: Data,
        path: PathBuf,
    ) -> Result<Box<Self>, String> {
        let core = Arc::new(Mutex::new(core));
        let (events_tx, events) = mpsc::channel(256);
        let mut cores = vec![Arc::clone(&core)];
        for p in &mut data.profiles {
            let c = Client::import(&p.state).map_err(|e| e.to_string())?;
            p.state.zeroize();
            cores.push(Arc::new(Mutex::new(c)));
        }
        // Folders of earlier bridges, and of versions before ADR-015.
        data::prune_tor_dirs(&path, data.tor_generation);
        let happened = data::permission_warning(&path)
            .map(Happened::Notice)
            .into_iter()
            .collect();
        let settings = crate::settings::Settings::load(&crate::settings::path(&path));
        let mut engine = Box::new(Self {
            core,
            key,
            data,
            cores,
            links: BTreeMap::new(),
            group_writes: BTreeMap::new(),
            group_move_retries: BTreeMap::new(),
            group_read: BTreeSet::new(),
            group_subscribing: BTreeSet::new(),
            retiring: Vec::new(),
            events,
            events_tx,
            connected: false,
            update: None,
            open: None,
            open_group: None,
            receipts: settings.receipts,
            read_receipts: settings.read_receipts,
            happened,
            path,
            leaving: None,
            gone: false,
            chunk_budget: (FILE_BURST, Instant::now()),
            large_budget: (large::LARGE_BURST, Instant::now()),
            slow_part_at: Instant::now(),
            chunks: BTreeMap::new(),
            heartbeat: None,
            unreachable: BTreeSet::new(),
            reclaim: None,
            claiming: BTreeSet::new(),
        });
        // ADR-030: states from before it held members like peers.
        let peers: BTreeSet<Ed25519Key> = engine.data.contacts.iter().map(|c| c.key).collect();
        for slot in 0..engine.cores.len() {
            engine.core_at(slot).mark_members(|k| peers.contains(k));
        }
        engine.drop_unproven_devices();
        engine.start_links(true)?;
        Ok(engine)
    }

    /// The main identity's state.
    pub fn core(&self) -> MutexGuard<'_, Client> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn core_at(&self, slot: usize) -> MutexGuard<'_, Client> {
        self.cores[slot]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn me(&self) -> Ed25519Key {
        self.core().identity().ed25519
    }

    /// Our key as this contact knows it (ADR-014).
    pub fn me_for(&self, c: &Contact) -> Ed25519Key {
        c.profile.unwrap_or_else(|| self.me())
    }

    /// The incognito identity this contact talks to, if any.
    pub fn profile_of(&self, c: &Contact) -> Option<&data::Profile> {
        let key = c.profile?;
        self.data.profiles.iter().find(|p| p.key == key)
    }

    /// The slot of the identity that talks to `contact`. `None` if its
    /// incognito identity is gone: never fall back to the main one, which
    /// would tie the two together.
    fn slot_for(&self, contact: &Ed25519Key) -> Option<usize> {
        let i = self.index(contact)?;
        match self.data.contacts[i].profile {
            None => Some(0),
            Some(key) => self.slot_of(&key),
        }
    }

    /// The slot of our identity `key`.
    fn slot_of(&self, key: &Ed25519Key) -> Option<usize> {
        if *key == self.me() {
            return Some(0);
        }
        let n = self.data.profiles.iter().position(|p| p.key == *key)?;
        Some(n + 1)
    }

    fn key_of(&self, slot: usize) -> Ed25519Key {
        self.core_at(slot).identity().ed25519
    }

    pub fn is_request(&self, contact: &Ed25519Key) -> bool {
        self.slot_for(contact)
            .is_some_and(|s| self.core_at(s).is_request(contact) == Some(true))
    }

    /// ADR-023: a contact from before anonymous queues, with no address
    /// either way. Only a new invite brings them back.
    pub fn needs_invite(&self, c: &Contact) -> bool {
        c.mine.is_none()
            && c.theirs.is_none()
            && c.their_invite.is_none()
            && !c.left
            && !self.is_request(&c.key)
    }

    fn nickname_at(&self, slot: usize) -> String {
        match slot {
            0 => self.data.nickname.clone(),
            n => self.data.profiles[n - 1].nickname.clone(),
        }
    }

    /// Our home relay: where our own queues are.
    fn home(&self) -> String {
        self.data.relay_url.clone()
    }

    fn invite_queue(&self, slot: usize) -> Option<&Queue> {
        match slot {
            0 => self.data.invite_queue.as_ref(),
            n => self.data.profiles[n - 1].invite_queue.as_ref(),
        }
    }

    fn invite_queue_mut(&mut self, slot: usize) -> &mut Option<Queue> {
        match slot {
            0 => &mut self.data.invite_queue,
            n => &mut self.data.profiles[n - 1].invite_queue,
        }
    }

    /// Where to write to contact `i`: their queue, or until we know it, their invite queue.
    fn out_addr(&self, i: usize) -> Option<&Addr> {
        let c = &self.data.contacts[i];
        c.theirs.as_ref().or(c.their_invite.as_ref())
    }

    /// Contact `i` reads a queue of ours: accepted, and not blocked or gone.
    fn wants_queue(&self, i: usize) -> bool {
        let c = &self.data.contacts[i];
        !c.blocked && !c.left && !self.is_request(&c.key) && self.slot_for(&c.key).is_some()
    }

    /// Whose link `who`'s queue uses: its own, or with `shared_link` its
    /// identity's (ADR-023, section 7). Identities never share one (ADR-014).
    fn link_who(&self, who: &Ed25519Key) -> Ed25519Key {
        if !self.data.shared_link || self.slot_of(who).is_some() {
            return *who;
        }
        self.slot_for(who).map_or(*who, |s| self.key_of(s))
    }

    /// Our identities and contacts whose queues use `link`.
    fn owners(&self, link: &Link) -> Vec<Ed25519Key> {
        let ids = (0..self.cores.len()).map(|s| self.key_of(s));
        let contacts = self.data.contacts.iter().map(|c| c.key);
        ids.chain(contacts)
            .filter(|k| self.link_who(k) == link.who)
            .collect()
    }

    /// Starts every link that should run and stops the rest (ADR-023,
    /// section 7): one per identity for its invite queue, one per contact
    /// on our relay (or the identity's, shared), and one more for a contact
    /// whose queue is elsewhere.
    fn sync_links(&mut self) -> Result<(), String> {
        self.start_links(false)
    }

    /// `spread`: at unlock, links come up over 0-30 s rather than at once
    /// (ADR-014); a link the user just asked for comes at once.
    fn start_links(&mut self, spread_out: bool) -> Result<(), String> {
        let home = self.home();
        let mut want: BTreeMap<Link, Option<String>> = BTreeMap::new();
        for slot in 0..self.cores.len() {
            let link = Link {
                who: self.key_of(slot),
                relay: home.clone(),
            };
            want.insert(link, self.data.onion_url.clone());
        }
        for i in 0..self.data.contacts.len() {
            if !self.wants_queue(i) {
                continue;
            }
            let who = self.link_who(&self.data.contacts[i].key);
            want.insert(
                Link {
                    who,
                    relay: home.clone(),
                },
                self.data.onion_url.clone(),
            );
            let c = &self.data.contacts[i];
            let elsewhere = self.out_addr(i).into_iter().chain(
                // ADR-028: their other devices', and ours, may read elsewhere.
                c.device_addrs.iter().chain(&c.siblings).map(|(_, a)| a),
            );
            for a in elsewhere.filter(|a| !same_url(&a.relay_url, &home)) {
                let link = Link {
                    who,
                    relay: a.relay_url.clone(),
                };
                want.insert(link, a.onion_url.clone());
            }
        }
        want.extend(self.group_links());
        self.links.retain(|l, _| want.contains_key(l));
        // Review 2026-11c, finding 3: an address that cannot be dialled (from a
        // peer, or with Tor on and no onion) skips its link; it must never
        // stop the rest from starting, nor the state from opening.
        let mut failed = BTreeSet::new();
        for (link, onion) in want {
            if self.links.contains_key(&link) {
                continue;
            }
            let (data, path, tx) = (&self.data, &self.path, &self.events_tx);
            // The main identity at once; the others spread out (ADR-014),
            // under Tor only: directly they share an IP anyway (ADR-038).
            let delay = match spread_out && self.data.tor && link.who != self.me() {
                true => spread(),
                false => Duration::ZERO,
            };
            let live = match start(&link, onion.as_deref(), data, path, tx, delay) {
                Ok(live) => live,
                Err(why) => {
                    failed.insert(why);
                    continue;
                }
            };
            // Events carry only the link, so an old link still deleting a
            // queue would take the new one's for its own. The new one sends
            // the deletion instead (sending it twice is harmless).
            if let Some(r) = self.retiring.iter().position(|(l, ..)| *l == link) {
                let (.., frame) = self.retiring.remove(r);
                let _ = live.relay.send(frame);
            }
            self.links.insert(link, live);
        }
        // Said once per reason, not at every new link.
        for why in failed
            .difference(&self.unreachable)
            .cloned()
            .collect::<Vec<_>>()
        {
            self.note(format!("a relay cannot be reached: {why}"));
        }
        self.unreachable = failed;
        Ok(())
    }

    /// The link `who`'s traffic to `relay` goes on.
    fn live(&self, who: &Ed25519Key, relay: &str) -> Option<&Live> {
        self.links.get(&Link {
            who: self.link_who(who),
            relay: relay.to_owned(),
        })
    }

    fn live_mut(&mut self, who: &Ed25519Key, relay: &str) -> Option<&mut Live> {
        let who = self.link_who(who);
        self.links.get_mut(&Link {
            who,
            relay: relay.to_owned(),
        })
    }

    /// Sends on the link of `who` to our relay.
    fn send_home(&self, who: &Ed25519Key, frame: ClientFrame) {
        if let Some(l) = self.live(who, &self.home()) {
            let _ = l.relay.send(frame);
        }
    }

    /// Asks for a queue for `who` (an identity or a contact) if it has none
    /// and none is on the way; the link's `Connected` asks otherwise.
    fn ensure_queue(&mut self, who: Ed25519Key) {
        if self.queue_of(&who).is_some() {
            return;
        }
        let home = self.home();
        if let Some(l) = self.live_mut(&who, &home)
            && l.connected
            && !l.creating.contains(&who)
        {
            l.creating.push(who);
            let _ = l.relay.send(ClientFrame::CreateQueue);
        }
    }

    fn subscribe(&mut self, who: Ed25519Key, q: &Queue) {
        let home = self.home();
        if let Some(l) = self.live_mut(&who, &home) {
            l.subscribing.push(q.recv_id);
            let _ = l.relay.send(ClientFrame::Subscribe {
                recv_id: q.recv_id,
                token: q.token,
            });
        }
        // The relay may have lost it, or this is a restored backup.
        if self.push_mut(&who).is_some_and(|p| p.is_some()) {
            self.tell_push(&who);
        }
    }

    /// A one-time invite: a fresh code that works for one request (ADR-013).
    pub fn invite(&mut self) -> Result<String, String> {
        let code = new_code();
        let invite = self.invite_with(0, code)?;
        let once = &mut self.data.invites.once;
        once.push(code);
        if once.len() > MAX_ONE_TIME_CODES {
            once.remove(0);
        }
        self.persist();
        Ok(invite)
    }

    /// The reusable invite; `renew` replaces it and stops the old one.
    pub fn public_invite(&mut self, renew: bool) -> Result<String, String> {
        let code = match self.data.invites.public {
            Some(code) if !renew => code,
            _ => new_code(),
        };
        let invite = self.invite_with(0, code)?;
        if self.data.invites.public != Some(code) {
            self.data.invites.public = Some(code);
            self.persist();
        }
        Ok(invite)
    }

    /// An invite of a new identity of its own, for one contact (ADR-014).
    /// An empty `nickname` picks two random words. It comes as
    /// `Happened::Invite` once the identity has its invite queue (ADR-023).
    pub fn incognito_invite(&mut self, nickname: &str) -> Result<(), String> {
        let waiting = self.waiting();
        if waiting.len() >= MAX_WAITING_PROFILES {
            self.retire(&waiting[0]);
        }
        self.new_profile(nickname, Some(new_code()))?;
        self.persist();
        Ok(())
    }

    /// Incognito identities whose invite nobody used yet, oldest first.
    fn waiting(&self) -> Vec<Ed25519Key> {
        let p = self.data.profiles.iter();
        p.filter(|p| p.code.is_some()).map(|p| p.key).collect()
    }

    /// The UnifiedPush endpoint of `who`'s queue: an identity's invite queue
    /// or a contact's queue, each its own (ADR-012, ADR-023).
    fn push_mut(&mut self, who: &Ed25519Key) -> Option<&mut Option<String>> {
        if let Some(slot) = self.slot_of(who) {
            return Some(match slot {
                0 => &mut self.data.push,
                n => &mut self.data.profiles[n - 1].push,
            });
        }
        let i = self.index(who)?;
        Some(&mut self.data.contacts[i].push)
    }

    /// The queue `who` reads: an identity's invite queue or our queue for a contact.
    fn queue_of(&self, who: &Ed25519Key) -> Option<&Queue> {
        match self.slot_of(who) {
            Some(slot) => self.invite_queue(slot),
            None => self.data.contacts[self.index(who)?].mine.as_ref(),
        }
    }

    /// Whose queue `recv_id` is.
    fn owner_of(&self, recv_id: &QueueId) -> Option<Ed25519Key> {
        let is = |q: Option<&Queue>| q.is_some_and(|q| q.recv_id == *recv_id);
        if let Some(slot) = (0..self.cores.len()).find(|&s| is(self.invite_queue(s))) {
            return Some(self.key_of(slot));
        }
        let c = self.data.contacts.iter().find(|c| is(c.mine.as_ref()))?;
        Some(c.key)
    }

    /// Identity `slot`'s fallback key state (ADR-002, ADR-019): published at
    /// all, when the previous key was rotated out, when the current one was made.
    fn fallback_mut(&mut self, slot: usize) -> (&mut bool, &mut Option<u64>, &mut Option<u64>) {
        match slot {
            0 => (
                &mut self.data.fallback_published,
                &mut self.data.old_fallback_since,
                &mut self.data.fallback_since,
            ),
            n => {
                let p = &mut self.data.profiles[n - 1];
                (
                    &mut p.fallback_published,
                    &mut p.old_fallback_since,
                    &mut p.fallback_since,
                )
            }
        }
    }

    /// Tops up identity `slot`'s one-time keys on its invite queue and
    /// replaces its fallback key when due, given what the relay says it has
    /// (ADR-002, ADR-019).
    fn publish_keys(&mut self, slot: usize, one_time_left: usize, fallback_used: bool) {
        let Some(q) = self.invite_queue(slot).cloned() else {
            return;
        };
        let (published, since, made) = {
            let (p, s, m) = self.fallback_mut(slot);
            (*p, *s, *m)
        };
        // ADR-002: a used fallback key is replaced, and the one before
        // it kept until nothing sent to it can still be queued.
        // vodozemac keeps only one previous key, so no new rotation
        // while one is waiting: claims alone (the relay reports every
        // one) must not push out a key a new contact may have used.
        // An empty relay (first run, or it lost its database) has no
        // fallback key either.
        // ADR-019: a key past its age counts as used; the relay's word is
        // not needed for that.
        let aged = made.is_none_or(|at| now() >= at + FALLBACK_MAX_AGE);
        let used = fallback_used || one_time_left == 0 || aged;
        let rotate = !published || (used && since.is_none());
        let keys = self.core_at(slot).prekeys(one_time_left, rotate);
        if !keys.one_time.is_empty() || keys.fallback.is_some() {
            let (p, s, m) = self.fallback_mut(slot);
            *p = true;
            if rotate {
                *m = Some(now());
                if published {
                    *s = Some(now());
                }
            }
            self.persist();
            let who = self.key_of(slot);
            self.send_home(
                &who,
                ClientFrame::PublishKeys {
                    recv_id: q.recv_id,
                    token: q.token,
                    one_time: keys.one_time,
                    fallback: keys.fallback,
                },
            );
            // An incognito invite asked for earlier: handed out once its
            // keys are on their way, so whoever takes it finds one.
            if slot > 0
                && !published
                && let Some(code) = self.data.profiles[slot - 1].code
                && let Ok(invite) = self.invite_with(slot, code)
                // ADR-045: made for an introduction, it goes to the introducer.
                && !self.grant_waiting(slot)
            {
                self.happened.push(Happened::Invite(invite));
            }
        }
    }

    /// Keeps `CONTACT_KEYS` one-time keys on our queue for contact `i`, so
    /// they can start a new session (ADR-023, amendment).
    fn top_up_contact_keys(&mut self, i: usize, left: u32) {
        let c = &self.data.contacts[i];
        let (Some(q), Some(slot)) = (c.mine.clone(), self.slot_for(&c.key)) else {
            return;
        };
        if left >= CONTACT_KEYS {
            return;
        }
        let one_time = self
            .core_at(slot)
            .one_time_keys((CONTACT_KEYS - left) as usize);
        self.persist();
        let who = self.data.contacts[i].key;
        self.send_home(
            &who,
            ClientFrame::PublishKeys {
                recv_id: q.recv_id,
                token: q.token,
                one_time,
                fallback: None,
            },
        );
    }

    /// Whether `who`'s link to our relay is up.
    fn link_up(&self, who: &Ed25519Key) -> bool {
        self.live(who, &self.home()).is_some_and(|l| l.connected)
    }

    /// ADR-019: the age check must not wait for a `KeyCount`, which the relay
    /// chooses to send. Only while connected, so the new key reaches the relay.
    fn rotate_aged_fallbacks(&mut self) {
        for slot in 0..self.cores.len() {
            let (published, since, made) = {
                let (p, s, m) = self.fallback_mut(slot);
                (*p, *s, *m)
            };
            let aged = made.is_none_or(|at| now() >= at + FALLBACK_MAX_AGE);
            if published && since.is_none() && aged && self.link_up(&self.key_of(slot)) {
                // "Plenty of one-time keys left": this is about the fallback key only.
                self.publish_keys(slot, usize::MAX, false);
            }
        }
    }

    /// A new incognito identity, connecting; its slot.
    fn new_profile(&mut self, nickname: &str, code: Option<InviteCode>) -> Result<usize, String> {
        let nickname = match nickname.trim() {
            "" => password::words(2).to_string(),
            n => n.to_string(),
        };
        check_nickname(&nickname).map_err(|_| "nickname: 1-64 bytes, no control characters")?;
        let core = Client::default();
        let key = core.identity().ed25519;
        self.data.profiles.push(data::Profile {
            key,
            nickname,
            code,
            ..Default::default()
        });
        self.cores.push(Arc::new(Mutex::new(core)));
        if let Err(e) = self.sync_links() {
            self.data.profiles.pop();
            self.cores.pop();
            return Err(e);
        }
        Ok(self.cores.len() - 1)
    }

    /// Deletes queue `q` of `who` on the relay; the link stays until it
    /// says so, or for `RETIRE_TIMEOUT`.
    fn delete_queue(&mut self, who: &Ed25519Key, q: &Queue) {
        let frame = ClientFrame::DeleteQueue {
            recv_id: q.recv_id,
            token: q.token,
        };
        // A shared link stays: it carries other queues too.
        if self.link_who(who) != *who {
            if let Some(l) = self.live(who, &q.relay_url) {
                let _ = l.relay.send(frame);
            }
            return;
        }
        let link = Link {
            who: *who,
            relay: q.relay_url.clone(),
        };
        if let Some(live) = self.links.remove(&link) {
            let _ = live.relay.send(frame.clone());
            self.retiring
                .push((link, live, Instant::now() + RETIRE_TIMEOUT, frame));
        }
    }

    /// Deletes incognito identity `key`'s invite queue and erases it here.
    fn retire(&mut self, key: &Ed25519Key) {
        let Some(slot) = self.slot_of(key).filter(|&s| s > 0) else {
            return;
        };
        if let Some(q) = self.invite_queue(slot).cloned() {
            self.delete_queue(key, &q);
        }
        self.cores.remove(slot);
        self.data.profiles.remove(slot - 1);
        let _ = self.sync_links();
    }

    /// Every one-time invite handed out so far stops working, incognito ones too.
    pub fn revoke_invites(&mut self) -> usize {
        let mut n = std::mem::take(&mut self.data.invites.once).len();
        let waiting = self.waiting();
        for key in &waiting {
            self.retire(key);
        }
        n += waiting.len();
        self.persist();
        n
    }

    fn invite_with(&self, slot: usize, code: InviteCode) -> Result<String, String> {
        Ok(self.invite_at(slot, code)?.encode())
    }

    fn invite_at(&self, slot: usize, code: InviteCode) -> Result<Invite, String> {
        let q = self
            .invite_queue(slot)
            .ok_or("not connected to the relay yet; try again in a moment")?;
        Ok(Invite {
            version: INVITE_VERSION,
            relay_url: q.relay_url.clone(),
            onion_url: q.onion_url.clone(),
            identity: self.core_at(slot).identity(),
            queue: q.send_id,
            code,
        })
    }

    pub fn index(&self, key: &Ed25519Key) -> Option<usize> {
        self.data.contacts.iter().position(|c| c.key == *key)
    }

    fn note(&mut self, text: impl Into<String>) {
        self.happened.push(Happened::Note(text.into()));
    }

    /// The last note, for callers that report it as their own error.
    fn take_note(&mut self) -> String {
        match self.happened.pop() {
            Some(Happened::Note(n)) => n,
            _ => String::new(),
        }
    }

    /// Must run after every change to the ratchet or the data.
    pub fn persist(&mut self) -> bool {
        if self.gone {
            return false;
        }
        let saved = self
            .cores
            .iter()
            .skip(1)
            .map(|c| c.lock().unwrap_or_else(PoisonError::into_inner).export())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
            .and_then(|states| self.data.to_json(&states).map_err(|e| e.to_string()))
            .and_then(|app| self.core().save(&self.key, &app).map_err(|e| e.to_string()))
            .and_then(|bytes| data::write_atomic(&self.path, &bytes).map_err(|e| e.to_string()));
        if let Err(e) = &saved {
            self.note(format!("SAVE FAILED: {e}"));
        }
        saved.is_ok()
    }

    fn system(&mut self, i: usize, text: impl Into<String>) {
        self.data.contacts[i].messages.push(Msg {
            mine: false,
            text: text.into(),
            at: now(),
            expires_at: None,
            status: Status::System,
            file: None,
            unfetched: false,
            ..Default::default()
        });
    }

    /// Encrypts, saves the advanced ratchet, then hands the envelope to the transport.
    /// An unsaved ratchet must never go on the wire: a restart would replay it.
    fn transmit(&mut self, key: &Ed25519Key, body: Body) -> Result<u64, String> {
        let slot = self.slot_for(key).ok_or(MISSING_PROFILE)?;
        let i = self.index(key).ok_or(MISSING_PROFILE)?;
        let to = self.out_addr(i).cloned().ok_or(NEEDS_INVITE)?;
        let envelope = self
            .core_at(slot)
            .encrypt(key, body)
            .map_err(|e| e.to_string())?;
        let next = &mut self.data.contacts[i].next_client_id;
        let client_id = *next;
        *next += 1;
        if !self.persist() {
            return Err(self.take_note());
        }
        self.live(key, &to.relay_url)
            .ok_or("relay link closed")?
            .relay
            .send(ClientFrame::Send {
                client_id,
                send_id: to.send_id,
                envelope,
            })
            .map_err(|_| "relay link closed")?;
        Ok(client_id)
    }

    /// Asks their relay for a key to start a session with contact `key`.
    fn claim(&mut self, key: Ed25519Key) {
        let Some(to) = self.index(&key).and_then(|i| self.out_addr(i)).cloned() else {
            return;
        };
        if let Some(l) = self.live(&key, &to.relay_url) {
            let _ = l.relay.send(ClientFrame::ClaimKey {
                send_id: to.send_id,
            });
        }
    }

    fn has_session(&self, key: &Ed25519Key) -> bool {
        self.slot_for(key)
            .is_some_and(|s| self.core_at(s).has_session(key))
    }

    /// Sends what we owe contact `i`, in this order (protocol.md, section 9):
    /// our request, our queue's address, our acceptance, then queued messages.
    fn flush(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        let Some(slot) = self.slot_for(&key) else {
            return self.note(MISSING_PROFILE);
        };
        if self.out_addr(i).is_none() || !self.has_session(&key) {
            return;
        }
        if self.data.contacts[i].request_pending {
            let (identity, nickname) = (self.core_at(slot).identity(), self.nickname_at(slot));
            let note = String::new();
            let body = match self.data.contacts[i].invite_code {
                // ADR-028: our answer to a new device's link code.
                Some(code) if self.data.contacts[i].own => Body::DeviceLink { code, identity },
                Some(code) => Body::ContactRequestV2 {
                    identity,
                    nickname,
                    note,
                    code,
                },
                None => Body::ContactRequest {
                    identity,
                    nickname,
                    note,
                },
            };
            match self.transmit(&key, body) {
                Ok(_) => self.data.contacts[i].request_pending = false,
                Err(e) => return self.note(e),
            }
        }
        // A linked device's queue reaches the contact in `Devices` below: a
        // `QueueOffer` would move where they write to the whole identity.
        if self.data.contacts[i].offer_pending && !self.is_primary() && !self.data.contacts[i].own {
            self.data.contacts[i].offer_pending = false;
        }
        if self.data.contacts[i].offer_pending
            && let Some(addr) = self.data.contacts[i].mine.as_ref().map(Queue::addr)
        {
            let body = Body::QueueOffer {
                relay_url: addr.relay_url,
                onion_url: addr.onion_url,
                send_id: addr.send_id,
            };
            match self.transmit(&key, body) {
                Ok(_) => self.data.contacts[i].offer_pending = false,
                Err(e) => return self.note(e),
            }
        }
        // They must know where to answer before they learn they may.
        let c = &self.data.contacts[i];
        if c.mine.is_none() || c.offer_pending {
            self.persist();
            return;
        }
        // ADR-028: this device, to the contact and to our other devices.
        if let Err(e) = self.tell_devices(i) {
            return self.note(e);
        }
        // ADR-028: a contact made after our devices were linked reaches them too.
        self.hand_to_devices(i);
        if self.data.contacts[i].accept_pending {
            let body = Body::ContactAccept {
                nickname: self.nickname_at(slot),
            };
            match self.transmit(&key, body) {
                Ok(_) => self.data.contacts[i].accept_pending = false,
                Err(e) => return self.note(e),
            }
        }
        // ADR-025: once, before anything that could use it.
        if self.owes_understands(i) {
            let body = Body::Understands {
                level: proto::UNDERSTANDS,
            };
            match self.transmit(&key, body) {
                Ok(_) => self.data.contacts[i].told = proto::UNDERSTANDS,
                Err(e) => return self.note(e),
            }
        }
        // ADR-041: what we gathered goes first, so the relay sees one burst.
        self.send_receipts(i);
        let rich = self.data.contacts[i].understands >= RICH_LEVEL;
        for m in 0..self.data.contacts[i].messages.len() {
            let msg = &self.data.contacts[i].messages[m];
            if msg.status != Status::Queued || msg.file.is_some() {
                continue;
            }
            // The timer it was written with, not the chat's now: the two copies
            // disappear after the same time even if it waited in the queue.
            let expires_in = msg
                .expires_at
                .map(|end| u32::try_from(end.saturating_sub(msg.at)).unwrap_or(u32::MAX));
            let body = match msg.id {
                Some(id) if rich => Body::Message {
                    id,
                    sent_at: msg.at,
                    expires_in,
                    reply_to: msg.reply_to,
                    text: msg.text.clone(),
                },
                _ => Body::Text {
                    sent_at: msg.at,
                    expires_in,
                    text: msg.text.clone(),
                },
            };
            self.data.contacts[i].messages[m].status = match self.transmit_all(&key, body) {
                Ok(id) => Status::Sending(id),
                Err(e) => Status::Failed(e),
            };
        }
        // ADR-028: what waits for single devices of theirs.
        self.flush_devices(i);
        // Edits, deletes and reactions, in the order they were made.
        while let Some(body) = self.data.contacts[i].outbox.first().cloned() {
            match self.transmit_all(&key, body) {
                Ok(_) => drop(self.data.contacts[i].outbox.remove(0)),
                Err(e) => return self.note(e),
            }
        }
        self.persist();
        self.send_chunks();
    }

    /// Contact `i` has not heard what we read (ADR-025), and may: never a
    /// stranger whose request is not accepted.
    fn owes_understands(&self, i: usize) -> bool {
        let c = &self.data.contacts[i];
        c.told < proto::UNDERSTANDS && !c.left && !c.blocked && !self.is_request(&c.key)
    }

    /// Something to send to contact `i`: flush with a session, claim a key without.
    fn deliver(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        match self.has_session(&key) {
            true => self.flush(i),
            false => self.claim(key),
        }
    }

    /// One relay event on `link`.
    pub fn on_relay(&mut self, event: RelayEvent) {
        self.on_relay_observing_direct_text(event, &mut |_, _| {});
    }

    /// ADR-032: a saved original text from the authenticated direct sender.
    pub fn on_relay_observing_direct_text(
        &mut self,
        event: RelayEvent,
        observe: &mut dyn FnMut(Ed25519Key, &str),
    ) {
        let before = self.happened.len();
        self.on_relay_event(event, observe);
        self.quiet_muted(before);
    }

    /// A muted chat or group still counts unread, without a sound or a
    /// notification; a stranger's request is never muted. From `before` on.
    fn quiet_muted(&mut self, before: usize) {
        let data = &self.data;
        let mut n = 0;
        self.happened.retain(|h| {
            n += 1;
            n <= before
                || match h {
                    Happened::Alert {
                        key,
                        request: false,
                    } => !data
                        .contacts
                        .iter()
                        .any(|c| c.key == *key && muted(c.muted_until)),
                    Happened::GroupAlert { group } => !data
                        .groups
                        .iter()
                        .any(|g| g.id == *group && muted(g.muted_until)),
                    _ => true,
                }
        });
    }

    fn on_relay_event(
        &mut self,
        (link, ev): RelayEvent,
        observe: &mut dyn FnMut(Ed25519Key, &str),
    ) {
        if let Some(r) = self.retiring.iter().position(|(l, ..)| *l == link) {
            // Gone from the relay, or at least from here.
            if matches!(
                ev,
                Event::Frame(ServerFrame::QueueDeleted { .. }) | Event::Disconnected
            ) {
                self.retiring.remove(r);
            }
            return;
        }
        let Some(live) = self.links.get_mut(&link) else {
            return;
        };
        let who = link.who;
        let home = same_url(&link.relay, &self.data.relay_url);
        match ev {
            Event::RelayVersion(v) => {
                live.version = parse_version(&v);
                let ours = parse_version(env!("CARGO_PKG_VERSION"));
                self.update = (parse_version(&v) > ours).then_some(v);
            }
            Event::Connected => {
                live.connected = true;
                live.creating.clear();
                if who == self.me() {
                    self.connected = true;
                }
                for owner in self.owners(&link) {
                    let contact = self.index(&owner);
                    if home {
                        match self.queue_of(&owner).cloned() {
                            Some(q) => self.subscribe(owner, &q),
                            None if contact.is_none_or(|i| self.wants_queue(i)) => {
                                self.ensure_queue(owner);
                            }
                            None => {}
                        }
                    }
                    // Sends go on the link to their relay, which may be this one.
                    if let Some(i) = contact
                        && self
                            .out_addr(i)
                            .is_some_and(|a| same_url(&a.relay_url, &link.relay))
                    {
                        let c = &self.data.contacts[i];
                        let owed = c.request_pending
                            || c.offer_pending
                            || c.accept_pending
                            || !c.outbox.is_empty()
                            || c.messages.iter().any(|m| m.status == Status::Queued);
                        let owed = owed || self.owes_understands(i);
                        if owed {
                            self.deliver(i);
                        }
                    }
                }
                if let Some(g) = self.group_of_link(&link) {
                    self.group_connected(g);
                }
            }
            Event::Disconnected => {
                live.connected = false;
                live.subscribing.clear();
                if who == self.me() {
                    self.connected = false;
                }
            }
            Event::Frame(frame) => self.on_frame(&link, frame, observe),
        }
    }

    fn on_frame(
        &mut self,
        link: &Link,
        frame: ServerFrame,
        observe: &mut dyn FnMut(Ed25519Key, &str),
    ) {
        if let Some(g) = self.group_of_link(link) {
            return self.on_group_frame(g, frame);
        }
        if let ServerFrame::Accepted { client_id }
        | ServerFrame::Error {
            client_id: Some(client_id),
            ..
        } = &frame
            && let Some(live) = self.links.get_mut(link)
        {
            live.heartbeats.retain(|_, pending| pending != client_id);
        }
        let who = link.who;
        match frame {
            ServerFrame::QueueCreated {
                recv_id,
                send_id,
                token,
            } => {
                let owner = match self.links.get_mut(link) {
                    Some(l) if !l.creating.is_empty() => l.creating.remove(0),
                    _ => who,
                };
                let q = Queue {
                    relay_url: link.relay.clone(),
                    onion_url: self.data.onion_url.clone(),
                    recv_id,
                    send_id,
                    token,
                };
                self.queue_created(owner, q);
            }
            ServerFrame::Subscribed { recv_id } => {
                if let Some(l) = self.links.get_mut(link) {
                    l.subscribing.retain(|q| *q != recv_id);
                }
            }
            ServerFrame::KeyCount {
                recv_id,
                one_time_left,
                fallback_used,
            } => {
                let Some(owner) = self.owner_of(&recv_id) else {
                    return;
                };
                match (self.slot_of(&owner), self.index(&owner)) {
                    (Some(slot), _) => {
                        self.publish_keys(slot, one_time_left as usize, fallback_used);
                    }
                    (None, Some(i)) => self.top_up_contact_keys(i, one_time_left),
                    _ => {}
                }
            }
            ServerFrame::ClaimedKey { send_id, key } => {
                let ours = |i: usize| {
                    self.out_addr(i).is_some_and(|a| a.send_id == send_id)
                        && self.link_who(&self.data.contacts[i].key) == who
                };
                let Some(i) = (0..self.data.contacts.len()).find(|&i| ours(i)) else {
                    return self.claimed_device_key(&who, send_id, &key);
                };
                let who = self.data.contacts[i].key;
                let slot = self.slot_for(&who);
                if let Some(slot) = slot
                    && !self.has_session(&who)
                {
                    let started = self.core_at(slot).start_session(&who, &key);
                    if let Err(e) = started {
                        return self.note(format!("cannot start session: {e}"));
                    }
                    self.persist();
                }
                self.flush(i);
            }
            ServerFrame::Deliver {
                recv_id,
                relay_id,
                envelope,
            } => {
                // Only from a queue this link reads; anything else is the relay's mistake.
                let Some(owner) = self.owner_of(&recv_id) else {
                    return;
                };
                if self.link_who(&owner) != who {
                    return;
                }
                let queue_relay = self
                    .queue_of(&owner)
                    .is_some_and(|q| same_url(&q.relay_url, &link.relay));
                let mut ignore = |_, _: &str| {};
                let observe = if queue_relay { observe } else { &mut ignore };
                // Not saved means not acked: the relay keeps it for the next connection.
                if self.receive(owner, &envelope, observe)
                    && let Some(l) = self.links.get(link)
                {
                    let _ = l.relay.send(ClientFrame::Ack { recv_id, relay_id });
                }
            }
            ServerFrame::Accepted { client_id } => {
                self.chunks.remove(&client_id);
                self.set_status(client_id, Status::Sent);
                self.goodbye_done(Some(client_id));
            }
            ServerFrame::Error {
                client_id: Some(id),
                code,
            } => {
                // Only refusals for good get here; the link retries the others (ADR-020).
                if let Some(key) = self.chunks.remove(&id) {
                    self.file_failed(&key, why(code));
                }
                self.set_status(id, Status::Failed(why(code)));
                self.goodbye_done(Some(id));
            }
            ServerFrame::Error {
                client_id: None,
                code,
            } => {
                // Our own queue is gone (the relay expired it or lost its
                // database): make a new one and tell whoever uses it.
                // The oldest pending subscribe is taken as the one
                // refused; an unknown queue from a ClaimKey in between would
                // make us renew a queue that was fine (harmless: a new address).
                let lost = match self.links.get_mut(link) {
                    Some(l) if code == ErrorCode::UnknownQueue && !l.subscribing.is_empty() => {
                        Some(l.subscribing.remove(0))
                    }
                    _ => None,
                };
                if code == ErrorCode::NoKeys {
                    // Their keys may be on their way (a fresh invite): try again.
                    self.reclaim = Some(Instant::now() + RECLAIM_AFTER);
                } else if let Some(owner) = lost.and_then(|q| self.owner_of(&q)) {
                    self.queue_lost(owner);
                } else {
                    self.note(format!("relay: {}", why(code)));
                }
            }
            ServerFrame::QueueDeleted { recv_id } => {
                let Some(l) = &mut self.leaving else { return };
                l.queues.retain(|q| *q != recv_id);
                if l.queues.is_empty() {
                    self.finish_wipe("");
                }
            }
            ServerFrame::Reported => self.note("reported to the relay operator, and blocked"),
            ServerFrame::PushSet => {}
            // ADR-027: groups are taken in in a later stage.
            ServerFrame::GroupCreated { .. }
            | ServerFrame::ReaderAdded { .. }
            | ServerFrame::ReaderRemoved { .. }
            | ServerFrame::GroupDeleted { .. } => {}
        }
    }

    /// A queue the relay made for `who`.
    fn queue_created(&mut self, who: Ed25519Key, q: Queue) {
        if let Some(slot) = self.slot_of(&who) {
            if self.invite_queue(slot).is_some() {
                return self.send_home(
                    &who,
                    ClientFrame::DeleteQueue {
                        recv_id: q.recv_id,
                        token: q.token,
                    },
                );
            }
            *self.invite_queue_mut(slot) = Some(q.clone());
            self.persist();
            self.subscribe(who, &q);
            return;
        }
        let Some(i) = self.index(&who) else { return };
        if self.data.contacts[i].mine.is_some() || !self.wants_queue(i) {
            return self.send_home(
                &who,
                ClientFrame::DeleteQueue {
                    recv_id: q.recv_id,
                    token: q.token,
                },
            );
        }
        let c = &mut self.data.contacts[i];
        c.mine = Some(q.clone());
        c.offer_pending = true;
        self.persist();
        self.subscribe(who, &q);
        self.deliver(i);
        // ADR-028: a linked device's address for another goes via the primary.
        self.introduce(i);
    }

    /// The relay no longer knows `who`'s queue. An invite queue takes the
    /// invites handed out with it along; a contact gets the new address.
    fn queue_lost(&mut self, who: Ed25519Key) {
        if let Some(slot) = self.slot_of(&who) {
            *self.invite_queue_mut(slot) = None;
            self.note("the relay lost your invite queue; invites given out before stop working");
        } else if let Some(i) = self.index(&who) {
            self.data.contacts[i].mine = None;
        }
        self.persist();
        self.ensure_queue(who);
    }

    /// ADR-009: goodbyes to every contact we talk to, then every queue
    /// deleted, then the wipe; `false` if the relay cannot be told (a
    /// `WipeAnyway` follows). `moved` (section 6) only erases this device, silently.
    pub fn leave(&mut self, moved: bool) -> bool {
        if moved {
            self.finish_wipe("");
            return true;
        }
        if !self.links.values().all(|l| l.connected) {
            self.happened.push(Happened::WipeAnyway(
                "Not every connection to the relay is up yet; wait a minute, or wipe anyway."
                    .into(),
            ));
            return false;
        }
        let told: Vec<_> = self
            .data
            .contacts
            .iter()
            .filter(|c| !c.blocked && !c.left && !c.request_pending)
            .map(|c| c.key)
            .collect();
        let mut waiting = Vec::new();
        for key in told {
            let talking = self.has_session(&key) && !self.is_request(&key);
            if talking && let Ok(id) = self.transmit(&key, Body::Goodbye) {
                waiting.push(id);
            }
        }
        let invites = (0..self.cores.len()).filter_map(|s| self.invite_queue(s));
        let contacts = self.data.contacts.iter().filter_map(|c| c.mine.as_ref());
        let queues = invites.chain(contacts).map(|q| q.recv_id).collect();
        self.leaving = Some(Leaving {
            waiting,
            until: Instant::now() + LEAVE_TIMEOUT,
            deleting: false,
            queues,
        });
        self.goodbye_done(None);
        true
    }

    /// The relay answered `client_id`; once every goodbye is in, delete every queue.
    fn goodbye_done(&mut self, client_id: Option<u64>) {
        let Some(l) = &mut self.leaving else { return };
        l.waiting.retain(|w| Some(*w) != client_id);
        if !l.waiting.is_empty() || l.deleting {
            return;
        }
        l.deleting = true;
        if l.queues.is_empty() {
            return self.finish_wipe("");
        }
        let owners: Vec<(Ed25519Key, Queue)> = (0..self.cores.len())
            .filter_map(|s| Some((self.key_of(s), self.invite_queue(s)?.clone())))
            .chain(
                self.data
                    .contacts
                    .iter()
                    .filter_map(|c| Some((c.key, c.mine.clone()?))),
            )
            .collect();
        for (who, q) in owners {
            self.send_home(
                &who,
                ClientFrame::DeleteQueue {
                    recv_id: q.recv_id,
                    token: q.token,
                },
            );
        }
    }

    fn leave_timeout(&mut self) {
        let Some(l) = &self.leaving else { return };
        if Instant::now() < l.until {
            return;
        }
        match l.deleting {
            true => self.finish_wipe(NOT_REMOVED),
            false => {
                self.leaving = None;
                self.happened.push(Happened::WipeAnyway(
                    "The relay did not answer in time; some contacts may already know.".into(),
                ));
            }
        }
    }

    /// Erases the device; `note` goes on the first-run screen.
    pub fn finish_wipe(&mut self, note: &str) {
        self.leaving = None;
        let happened = match data::wipe(&self.path) {
            Ok(()) => {
                self.gone = true;
                for g in 0..self.data.groups.len() {
                    self.stop_group_writes(g, true);
                }
                Happened::Wiped(note.into())
            }
            Err(e) => Happened::Notice(format!("wipe failed: {e}")),
        };
        self.happened.push(happened);
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

    /// Whether a stranger's request to our identity `slot` may in; spends the
    /// code (ADR-013). An incognito identity takes only its own invite's code,
    /// once: then it belongs to that contact (ADR-014).
    fn admit(&mut self, slot: usize, body: &Body) -> bool {
        // ADR-028: the primary answering this device's link code.
        if let (0, Body::DeviceLink { code, .. }) = (slot, body) {
            let ok = self
                .data
                .link_code
                .is_some_and(|mine| bool::from(mine.ct_eq(code)));
            if ok {
                self.data.link_code = None;
            }
            return ok;
        }
        if slot == 0 {
            return self.data.invites.admit(body);
        }
        let p = &mut self.data.profiles[slot - 1];
        let ok = match (body, p.code) {
            (Body::ContactRequestV2 { code, .. }, Some(mine)) => bool::from(mine.ct_eq(code)),
            _ => false,
        };
        if ok {
            p.code = None;
        }
        ok
    }

    /// Decrypts and stores one delivery from the queue of `who`: one of our
    /// identities (its invite queue) or a contact (our queue for them).
    /// `true` once it is safe to ack: the message is on disk, or there was
    /// nothing worth keeping.
    fn receive(
        &mut self,
        who: Ed25519Key,
        envelope: &proto::Envelope,
        observe: &mut dyn FnMut(Ed25519Key, &str),
    ) -> bool {
        let invite_slot = self.slot_of(&who);
        let (slot, from) = match invite_slot {
            Some(slot) => (slot, None),
            None => match self.slot_for(&who) {
                Some(slot) => (slot, Some(who)),
                None => return true,
            },
        };
        // Blocked: not even decrypted (its queue is normally gone already).
        if from.is_some_and(|k| {
            self.index(&k)
                .is_some_and(|i| self.data.contacts[i].blocked)
        }) {
            return true;
        }
        let had_session = from.is_some_and(|k| self.core_at(slot).has_session(&k));
        let mut result = self.core_at(slot).decrypt(from.as_ref(), envelope);
        // ADR-028: the contact's other devices write here too, and so do ours
        // with copies of what we sent them.
        let mut via = from;
        if result.is_err() && !matches!(result, Err(CoreError::Duplicate)) {
            for k in from.map(|k| self.writers_of(&k)).unwrap_or_default() {
                let r = self.core_at(slot).decrypt(Some(&k), envelope);
                if r.is_ok() || matches!(r, Err(CoreError::Duplicate)) {
                    (result, via) = (r, Some(k));
                    break;
                }
            }
        }
        let mut got = match result {
            Ok(got) => got,
            Err(CoreError::Duplicate) => return true,
            // The status bar, not the chat: a relay must not be able to plant
            // doubts in any chat.
            Err(e) => {
                if from.is_some() {
                    self.note(format!("a message could not be decrypted ({e})"));
                }
                return true;
            }
        };
        let original_direct_text = from == Some(got.sender)
            && via == from
            && !got.request
            && matches!(got.body, Body::Text { .. } | Body::Message { .. });
        if let (Some(k), Some(v)) = (from, via)
            && v != k
        {
            // A group key from another device: its session is that device's.
            if matches!(got.body, Body::GroupKey { .. }) {
                self.group_key_from(v, got.body);
                return self.persist();
            }
            if self.index(&v).is_some_and(|o| self.data.contacts[o].own) {
                // ADR-030: only a copy that names the chat whose queue it came
                // in; a bare one, or one for another chat, a relay moved here.
                if let Body::Copy { contact, body } = got.body
                    && contact == k
                    && let Some(i) = self.index(&k)
                {
                    self.own_copy(i, *body);
                }
                return self.persist();
            }
            // Queue addresses and reader grants are not part of the signed roster.
            // Only the admin's primary device may send them or revoke a reader.
            if matches!(
                got.body,
                Body::GroupInvite { .. } | Body::GroupMove { .. } | Body::GroupLeave { .. }
            ) {
                return self.persist();
            }
            // A device of theirs speaks for them.
            got.sender = k;
        }
        let sender = got.sender;
        let known = self.index(&sender);
        // Turned down, blocked, or a contact of another of our identities
        // writing to this one's invite queue: they must stay apart.
        let refused = self.data.rejected.contains(&sender)
            || known.is_some_and(|i| self.data.contacts[i].blocked)
            || (known.is_some() && self.slot_for(&sender) != Some(slot));
        if refused {
            if known.is_none() || self.slot_for(&sender) != Some(slot) {
                self.core_at(slot).remove_contact(&sender);
            }
            self.persist();
            return true;
        }
        let i = match known {
            Some(i) => i,
            // Silently: they cannot tell a refusal from a request not yet seen.
            None if !self.admit(slot, &got.body) => {
                self.core_at(slot).remove_contact(&sender);
                self.persist();
                return true;
            }
            None => self.data.push_person(Contact {
                key: sender,
                added_at: now(),
                profile: (slot > 0).then(|| self.data.profiles[slot - 1].key),
                next_client_id: getrandom::u64().unwrap_or(0) >> 2,
                ..Default::default()
            }),
        };
        // A first session with another device of theirs is no restart (ADR-028).
        if got.new_session && had_session && via == from {
            self.system(
                i,
                "⚠ they restarted the conversation (restored a backup?); messages may be lost",
            );
        }
        show_missed(&mut self.data.contacts[i], got.missed);
        let alert = |request| Happened::Alert {
            key: sender,
            request,
        };
        let intro_code = match &got.body {
            Body::ContactRequestV2 { code, .. } => Some(*code),
            _ => None,
        };
        match got.body {
            // A stranger's texts wait out of sight until the request is accepted.
            Body::Text { text, .. } | Body::Message { text, .. } if got.request => {
                let c = &mut self.data.contacts[i];
                match c.hidden.len() < MAX_HIDDEN {
                    true => c.hidden.push(Msg {
                        mine: false,
                        text,
                        at: now(),
                        expires_at: None,
                        status: Status::Received,
                        file: None,
                        unfetched: true,
                        ..Default::default()
                    }),
                    false => c.held_back = c.held_back.saturating_add(1),
                }
            }
            Body::ContactAccept { .. } if got.request => {}
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
                    file: None,
                    unfetched: true,
                    ..Default::default()
                });
                self.happened.push(alert(false));
            }
            // The note is never shown: our clients send none, and a stranger's
            // text inside a system line would pass for the app speaking.
            Body::ContactRequest { .. } | Body::ContactRequestV2 { .. } => {
                let request = self.core_at(slot).is_request(&sender);
                // ADR-045: someone an introduction let in is accepted at once.
                let introduced = request == Some(true)
                    && intro_code.is_some_and(|code| self.accept_introduced(i, &sender, &code));
                if introduced {
                    self.happened.push(alert(false));
                } else if request == Some(true) {
                    self.happened.push(alert(true));
                    self.system(i, "contact request: accept it, or delete to reject");
                } else {
                    // A contact we have, asking again: from a new device, or
                    // after the move to queues (ADR-023). They get our queue.
                    // They may have lost us (deleted, new device): our name
                    // travels only in `ContactAccept`, after our queue's address.
                    let c = &mut self.data.contacts[i];
                    c.request_pending = false;
                    c.offer_pending = c.mine.is_some();
                    c.accept_pending = true;
                    // Their old queue may be gone with them; the new one's
                    // address comes right after the request (protocol, 7).
                    c.theirs = None;
                    self.system(i, "connected");
                    let _ = self.sync_links();
                    self.ensure_queue(sender);
                    self.deliver(i);
                }
            }
            Body::ContactAccept { .. } => self.system(i, "accepted your request"),
            Body::QueueOffer {
                relay_url,
                onion_url,
                send_id,
            } => {
                if !self.peer_addr_ok(&relay_url, onion_url.as_deref()) {
                    self.note("a contact sent an address that cannot be used");
                    return self.persist();
                }
                self.data.contacts[i].theirs = Some(Addr {
                    relay_url,
                    onion_url,
                    send_id,
                });
                self.data.contacts[i].their_invite = None;
                if !got.request {
                    let _ = self.sync_links();
                    self.deliver(i);
                }
                return self.persist();
            }
            // ADR-017: no files before the request is accepted.
            Body::File { .. } if got.request => return self.persist(),
            body @ Body::File { .. } => {
                if !self.data.take_chunk(i, body, now()) {
                    return self.persist();
                }
                let whole = self.data.contacts[i].messages.iter().rev();
                if let Some(id) = whole
                    .filter(|m| !m.mine)
                    .find_map(|m| m.file.as_ref())
                    .map(|f| f.id)
                {
                    self.got(i, data::Seen::File(id));
                }
                self.happened.push(alert(false));
            }
            // ADR-045: introductions, between accepted contacts only.
            Body::IntroAsk { .. }
            | Body::IntroGrant { .. }
            | Body::IntroDecline { .. }
            | Body::IntroOffer { .. }
                if got.request =>
            {
                return self.persist();
            }
            body @ (Body::IntroAsk { .. }
            | Body::IntroGrant { .. }
            | Body::IntroDecline { .. }
            | Body::IntroOffer { .. }) => {
                self.take_intro(i, body);
                return self.persist();
            }
            // ADR-043: documents on request, between accepted contacts only.
            Body::FileOffer { .. }
            | Body::FileWant { .. }
            | Body::FileAck { .. }
            | Body::FileStop { .. }
            | Body::FilePart { .. }
                if got.request =>
            {
                return self.persist();
            }
            body @ (Body::FileOffer { .. }
            | Body::FileWant { .. }
            | Body::FileAck { .. }
            | Body::FileStop { .. }
            | Body::FilePart { .. }) => {
                self.take_large(i, body);
                return self.persist();
            }
            Body::Receipt { .. } if got.request => return self.persist(),
            Body::Receipt {
                messages,
                files,
                read,
            } => {
                self.take_receipt(i, &messages, &files, read);
                return self.persist();
            }
            // ADR-025: a kind from a newer client; its `seq` counted.
            Body::Unknown => return self.persist(),
            Body::Heartbeat { sent_at } => {
                let saved = self.persist();
                if saved
                    && from == Some(sender)
                    && via == from
                    && !got.request
                    && self.verified_contact(&sender).is_ok()
                {
                    self.heartbeat = Some((sender, sent_at));
                }
                return saved;
            }
            // ADR-030: copies come only from our own devices, handled above.
            Body::Copy { .. } => return self.persist(),
            // ADR-028: devices of one identity.
            body @ (Body::DeviceLink { .. }
            | Body::DeviceLinked { .. }
            | Body::LinkedContact { .. }
            | Body::OwnDevices { .. }
            | Body::Devices { .. }
            | Body::OwnQueue { .. }
            | Body::SiblingQueue { .. }
            | Body::GroupDevices { .. }
            | Body::OwnGroupJoin { .. }) => {
                self.device_body(i, got.request, body);
                return self.persist();
            }
            Body::Understands { level } => {
                let c = &mut self.data.contacts[i];
                let news = level > c.understands;
                c.understands = level;
                if news {
                    c.told = 0;
                    if !got.request {
                        self.deliver(i);
                        self.invite_awaiting(i);
                    }
                }
                // A member who now reads device lists may be the last one a
                // group waited for: as admin, welcome the devices it kept out.
                if news && level >= proto::DEVICES_LEVEL && !got.request {
                    let mut owners: Vec<Ed25519Key> = vec![self.identity_key()];
                    owners.extend(
                        self.data
                            .contacts
                            .iter()
                            .filter(|c| c.their_devices.is_some())
                            .map(|c| c.key),
                    );
                    for who in owners {
                        self.group_devices_changed(who, &[]);
                    }
                }
                return self.persist();
            }
            // ADR-027: invites, joins and keys come over a chat; a stranger's
            // wait for acceptance like their texts.
            Body::GroupInvite { .. }
            | Body::GroupJoin { .. }
            | Body::GroupKey { .. }
            | Body::GroupRoster { .. }
            | Body::GroupLeave { .. }
            | Body::GroupMove { .. }
            | Body::GroupAdmins { .. }
            | Body::GroupAdminKeys { .. }
                if got.request =>
            {
                return self.persist();
            }
            body @ (Body::GroupInvite { .. }
            | Body::GroupJoin { .. }
            | Body::GroupKey { .. }
            | Body::GroupMove { .. }
            | Body::GroupLeave { .. }
            | Body::GroupAdmins { .. }
            | Body::GroupAdminKeys { .. }) => {
                self.group_from_contact(i, body);
                return self.persist();
            }
            Body::GroupRoster { .. } => return self.persist(),
            // ADR-042: a resign is said inside the group only.
            Body::GroupResign { .. } => return self.persist(),
            // Pointing at messages waits for the request to be accepted.
            Body::Edit { .. } | Body::Delete { .. } | Body::React { .. } if got.request => {
                return self.persist();
            }
            Body::Message {
                id,
                expires_in,
                reply_to,
                text,
                ..
            } => {
                let at = now();
                self.data.contacts[i].messages.push(Msg {
                    mine: false,
                    text,
                    at,
                    expires_at: expires_in.map(|s| at + u64::from(s)),
                    status: Status::Received,
                    file: None,
                    unfetched: true,
                    id: Some(id),
                    // Seen from us: their `theirs` is ours.
                    reply_to: reply_to.map(|r| Ref {
                        id: r.id,
                        theirs: !r.theirs,
                    }),
                    ..Default::default()
                });
                self.got(i, data::Seen::Message(id));
                self.happened.push(alert(false));
            }
            body @ (Body::Edit { .. } | Body::Delete { .. } | Body::React { .. }) => {
                if !self.data.contacts[i].change(body) {
                    return self.persist();
                }
                self.happened.push(alert(false));
            }
            Body::Goodbye if got.request => {}
            Body::Goodbye => {
                self.system(i, "deleted their account; this chat is read-only now");
                self.data.contacts[i].left = true;
                self.happened.push(alert(false));
            }
        }
        if self.open != Some(sender) {
            self.data.contacts[i].unread += 1;
        }
        let saved = self.persist();
        if saved
            && original_direct_text
            && let Some(message) = self.data.contacts[i].messages.last()
        {
            observe(sender, &message.text);
        }
        saved
    }

    /// The app came on screen: every link checks itself now (ADR-012).
    pub fn reconnect(&self) {
        self.links.values().for_each(|l| l.relay.reconnect());
    }

    /// A push for `who`'s queue: the relay heard nothing from that link,
    /// which checks itself (ADR-012). The others stay as they are (ADR-014).
    pub fn woken(&self, who: &Ed25519Key) {
        if let Some(l) = self.live(who, &self.home()) {
            l.relay.check();
            if self.data.hurry_open {
                l.relay.hurry();
            }
        }
    }

    /// ADR-038: the link of chat `key` has not reached the relay yet, so
    /// what it carries waits (the spread after unlock, or a reconnect).
    pub fn chat_connecting(&self, key: &Ed25519Key) -> bool {
        self.live(key, &self.home()).is_some_and(|l| !l.connected)
    }

    /// ADR-038, a setting: the chat and group on screen skip the rest of
    /// their spread. Called from `tick`.
    fn hurry_open_links(&self) {
        if !self.data.hurry_open {
            return;
        }
        if let Some(l) = self.open.and_then(|k| self.live(&k, &self.home())) {
            l.relay.hurry();
        }
        let open_group = self
            .open_group
            .and_then(|id| self.data.groups.iter().position(|g| g.id == id));
        if let Some(l) = open_group.and_then(|g| self.group_live(g)) {
            l.relay.hurry();
        }
    }

    /// ADR-038: whether the open chat's link may skip its spread.
    pub fn set_hurry_open(&mut self, on: bool) {
        self.data.hurry_open = on;
        self.persist();
        self.hurry_open_links();
    }

    /// Who has a queue, and so a UnifiedPush instance of its own: our
    /// identities (invite queues) and contacts (ADR-012, ADR-023).
    pub fn push_owners(&self) -> Vec<Ed25519Key> {
        let ids = (0..self.cores.len()).map(|s| self.key_of(s));
        let contacts = self.data.contacts.iter().filter(|c| c.mine.is_some());
        ids.chain(contacts.map(|c| c.key)).collect()
    }

    /// Timers: disappearing messages, the rotated-out fallback key, a wipe
    /// waiting for the relay. Call about once a second.
    pub fn tick(&mut self) {
        self.retry_group_moves();
        self.retire_old_queues();
        self.hurry_open_links();
        if self.reclaim.is_some_and(|at| Instant::now() >= at) {
            self.reclaim = None;
            self.claiming.clear();
            for g in 0..self.data.groups.len() {
                if !self.data.groups[g].left {
                    self.share_key(g);
                }
            }
            for i in 0..self.data.contacts.len() {
                let c = &self.data.contacts[i];
                let owed = c.request_pending || c.offer_pending || c.accept_pending;
                if owed && !self.has_session(&c.key) {
                    self.claim(c.key);
                }
            }
        }
        self.expire();
        self.receipts_due();
        self.rotate_aged_fallbacks();
        self.send_chunks();
        self.leave_timeout();
        let t = Instant::now();
        self.retiring.retain(|(_, _, until, _)| *until > t);
    }

    /// Drops messages whose disappearing timer ran out, and the rotated-out
    /// fallback keys once nothing sent to them can still be queued (ADR-002).
    fn expire(&mut self) {
        let t = now();
        let mut changed = false;
        let keep = (proto::QUEUE_TTL_DAYS + 1) * 86_400;
        let due = |since: Option<u64>| since.is_some_and(|since| t >= since + keep);
        if due(self.data.old_fallback_since) {
            self.core().forget_old_fallback();
            self.data.old_fallback_since = None;
            changed = true;
        }
        for n in 0..self.data.profiles.len() {
            if due(self.data.profiles[n].old_fallback_since) {
                self.core_at(n + 1).forget_old_fallback();
                self.data.profiles[n].old_fallback_since = None;
                changed = true;
            }
        }
        for c in &mut self.data.contacts {
            let before = c.messages.len();
            c.messages.retain(|m| m.expires_at.is_none_or(|e| e > t));
            changed |= c.messages.len() != before;
        }
        for g in &mut self.data.groups {
            let before = g.messages.len();
            g.messages.retain(|m| m.expires_at.is_none_or(|e| e > t));
            let ids: Vec<u64> = g.messages.iter().filter_map(|m| m.id).collect();
            g.reactions.retain(|(id, _, _)| ids.contains(id));
            changed |= g.messages.len() != before;
        }
        changed |= self.expire_offers();
        changed |= self.expire_intros();
        if changed {
            self.persist();
        }
        // ADR-043: also after a chat was cleared or deleted.
        self.sweep_spool();
    }

    /// Accepts contact `i`'s request: their held texts appear. They are told
    /// once our queue for them exists, after its address (ADR-023).
    pub fn accept(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        let Some(slot) = self.slot_for(&key) else {
            return;
        };
        if !self.core_at(slot).accept(&key) {
            return;
        }
        let c = &mut self.data.contacts[i];
        c.accept_pending = true;
        c.messages.push(Msg {
            mine: false,
            text: "you accepted the request".into(),
            at: now(),
            expires_at: None,
            status: Status::System,
            file: None,
            unfetched: false,
            ..Default::default()
        });
        let hidden = std::mem::take(&mut c.hidden);
        let held = std::mem::take(&mut c.held_back);
        c.messages.extend(hidden);
        if held > 0 {
            self.system(
                i,
                format!("{held} more message(s) from before you accepted were not kept"),
            );
        }
        self.persist();
        if let Err(e) = self.sync_links() {
            return self.note(e);
        }
        self.ensure_queue(key);
    }

    /// Changes one contact's settings and saves.
    pub fn edit(&mut self, i: usize, change: impl FnOnce(&mut Contact)) {
        if let Some(c) = self.data.contacts.get_mut(i) {
            change(c);
            self.persist();
        }
    }

    /// Their last few messages, joined, within the protocol limit (ADR-006).
    pub fn report_text(&self, i: usize) -> Option<String> {
        const LAST: usize = 5;
        let c = self.data.contacts.get(i)?;
        let mut theirs: Vec<&str> = c
            .messages
            .iter()
            .rev()
            .filter(|m| m.status == Status::Received)
            .take(LAST)
            .map(|m| m.text.as_str())
            .collect();
        theirs.reverse();
        let mut text = theirs.join("\n---\n");
        if text.len() > proto::MAX_TEXT_BYTES {
            let mut end = proto::MAX_TEXT_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        Some(text).filter(|t| !t.is_empty())
    }

    /// Sends `text` (shown to the user first) to the relay operator and blocks
    /// contact `i`. The relay learns only the text, not who wrote it (ADR-023).
    pub fn report(&mut self, i: usize, text: String) {
        let Some(c) = self.data.contacts.get(i) else {
            return;
        };
        if let Some(q) = c.mine.clone() {
            self.send_home(
                &c.key,
                ClientFrame::Report {
                    recv_id: q.recv_id,
                    token: q.token,
                    text,
                },
            );
        }
        self.block(i, true);
    }

    /// Blocks or unblocks contact `i`. Blocking deletes our queue for them,
    /// so they cannot put a byte in it; unblocking makes a new one and tells
    /// them its address (ADR-023).
    pub fn block(&mut self, i: usize, on: bool) {
        let Some(c) = self.data.contacts.get_mut(i) else {
            return;
        };
        c.blocked = on;
        let key = c.key;
        if on {
            c.offer_pending = false;
            if let Some(q) = c.mine.take() {
                self.delete_queue(&key, &q);
            }
        }
        self.persist();
        if let Err(e) = self.sync_links() {
            self.note(e);
        }
        if !on {
            self.ensure_queue(key);
        }
    }

    /// Removes contact `i` and deletes our queue for them. Turning down a
    /// request is remembered, so it cannot simply ask again. An incognito
    /// contact takes its identity with it: a goodbye first (ADR-014).
    pub fn delete(&mut self, i: usize) {
        let Some(c) = self.data.contacts.get(i) else {
            return;
        };
        let (key, profile, quiet) = (c.key, c.profile, c.blocked || c.left);
        let slot = self.slot_for(&key);
        if profile.is_some() && !quiet && self.has_session(&key) && !self.is_request(&key) {
            let _ = self.transmit(&key, Body::Goodbye);
        }
        let was_request = self.is_request(&key);
        if let Some(q) = self.data.contacts[i].mine.clone() {
            self.delete_queue(&key, &q);
        }
        self.data.contacts.remove(i);
        if let Some(slot) = slot {
            self.core_at(slot).remove_contact(&key);
        }
        match profile {
            Some(p) => self.retire(&p),
            None if was_request && !self.data.rejected.contains(&key) => {
                if self.data.rejected.len() >= MAX_REJECTED {
                    self.data.rejected.remove(0);
                }
                self.data.rejected.push(key);
            }
            None => {}
        }
        self.persist();
        let _ = self.sync_links();
    }

    /// Contact `i`, if we may write to them.
    fn writable(&self, i: usize) -> Result<&Contact, String> {
        let c = self.data.contacts.get(i).ok_or("add a contact first")?;
        // Our own devices (ADR-028) are no one to write to.
        if c.own {
            return Err("add a contact first".into());
        }
        if c.blocked {
            return Err("contact is blocked; unblock them to write".into());
        }
        if c.left {
            return Err("this contact left".into());
        }
        if self.needs_invite(c) {
            return Err(NEEDS_INVITE.into());
        }
        if self.removed() {
            return Err("this device was removed from your account".into());
        }
        // ADR-028: an app before devices hears only from the primary.
        if !self.is_primary() && !c.own && c.understands < proto::DEVICES_LEVEL {
            return Err("their app knows no second devices yet: write from your main one, or ask them to update".into());
        }
        Ok(c)
    }

    /// ADR-031 and ADR-032: pin a verified contact even if names change.
    pub fn verified_contact(&self, key: &Ed25519Key) -> Result<usize, String> {
        let i = self.index(key).ok_or("contact was removed")?;
        let c = self.writable(i)?;
        let accepted = self
            .slot_for(key)
            .is_some_and(|slot| self.core_at(slot).is_request(key) == Some(false));
        if !self.is_primary() || !c.verified || c.request_pending || !accepted {
            return Err(
                "this command needs the main device and an accepted, verified contact".into(),
            );
        }
        Ok(i)
    }

    /// Sends at most one unanswered pulse per contact without a chat entry.
    pub fn send_heartbeat(&mut self, key: &Ed25519Key) -> Result<(), String> {
        let i = self.verified_contact(key)?;
        let Some(to) = &self.data.contacts[i].theirs else {
            return Ok(());
        };
        let relay = to.relay_url.clone();
        if !self.live(key, &relay).is_some_and(|l| l.connected)
            || self.links.values().any(|l| l.heartbeats.contains_key(key))
        {
            return Ok(());
        }
        if !self.has_session(key) {
            self.claim(*key);
            return Ok(());
        }
        let id = self.transmit(key, Body::Heartbeat { sent_at: now() })?;
        if let Some(live) = self.live_mut(key, &relay) {
            live.heartbeats.insert(*key, id);
        }
        Ok(())
    }

    /// The latest direct pulse is private to the daemon, outside chat history.
    pub fn take_heartbeat(&mut self) -> Option<(Ed25519Key, u64)> {
        self.heartbeat.take()
    }

    /// Queues `text` to contact `i` and sends it as soon as there is a session.
    pub fn send(&mut self, i: usize, text: &str) -> Result<(), String> {
        let timer = self.writable(i)?.expires_in;
        self.send_text(i, text, None, timer)
    }

    /// Like `send`, with a timer of its own instead of the chat's: the
    /// scriptable `send --expires` (`expires_in` travels in every message).
    pub fn send_expiring(&mut self, i: usize, text: &str, expires_in: u32) -> Result<(), String> {
        self.send_text(i, text, None, Some(expires_in))
    }

    /// `text` as an answer to message `m` of chat `i` (ADR-025).
    pub fn reply(&mut self, i: usize, m: usize, text: &str) -> Result<(), String> {
        let target = self.target(i, m)?;
        let timer = self.writable(i)?.expires_in;
        self.send_text(i, text, Some(target), timer)
    }

    fn send_text(
        &mut self,
        i: usize,
        text: &str,
        reply_to: Option<Ref>,
        expires_in: Option<u32>,
    ) -> Result<(), String> {
        if text.len() > proto::MAX_TEXT_BYTES {
            return Err("message too long".into());
        }
        self.writable(i)?;
        let id = getrandom::u64().map_err(|e| e.to_string())?;
        let at = now();
        self.data.contacts[i].messages.push(Msg {
            mine: true,
            text: text.to_string(),
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Queued,
            file: None,
            id: Some(id),
            reply_to,
            ..Default::default()
        });
        self.persist();
        self.deliver(i);
        Ok(())
    }

    /// Whether chat `i` can answer, edit, delete and react (ADR-025): their
    /// app has said it reads these.
    pub fn rich(&self, i: usize) -> bool {
        self.data.contacts[i].understands >= RICH_LEVEL
    }

    /// Message `m` of chat `i` as something to point at (ADR-025).
    fn target(&self, i: usize, m: usize) -> Result<Ref, String> {
        self.writable(i)?;
        if !self.rich(i) {
            return Err(OLDER_APP.into());
        }
        let msg = self.data.contacts[i]
            .messages
            .get(m)
            .ok_or("no such message")?;
        match msg.id {
            Some(id) if !msg.deleted => Ok(Ref {
                id,
                theirs: !msg.mine,
            }),
            _ => Err("this message cannot be pointed at".into()),
        }
    }

    /// Queues `body` about our message `m` of chat `i`, unless the message
    /// itself has not gone yet.
    fn own_change(&mut self, i: usize, m: usize, body: Body) -> Result<(), String> {
        if self.target(i, m)?.theirs {
            return Err("only your own messages can be changed".into());
        }
        let c = &mut self.data.contacts[i];
        if c.messages[m].status != Status::Queued {
            c.outbox.push(body);
        }
        Ok(())
    }

    /// New text for our message `m` of chat `i` (ADR-025).
    pub fn edit_message(&mut self, i: usize, m: usize, text: &str) -> Result<(), String> {
        if text.len() > proto::MAX_TEXT_BYTES {
            return Err("message too long".into());
        }
        if text.trim().is_empty() {
            return Err("an edit cannot be empty; delete the message instead".into());
        }
        let id = self.target(i, m)?.id;
        let body = Body::Edit {
            id,
            text: text.to_string(),
        };
        self.own_change(i, m, body)?;
        let msg = &mut self.data.contacts[i].messages[m];
        // Still queued: it goes out with the new text, as a first version.
        msg.edited = msg.status != Status::Queued;
        msg.replace_text(text.to_string());
        self.persist();
        self.deliver(i);
        Ok(())
    }

    /// Deletes our message `m` of chat `i` for both sides (ADR-025).
    pub fn delete_message(&mut self, i: usize, m: usize) -> Result<(), String> {
        let id = self.target(i, m)?.id;
        self.own_change(i, m, Body::Delete { id })?;
        let messages = &mut self.data.contacts[i].messages;
        match messages[m].status {
            // Never sent: as if it never was.
            Status::Queued => drop(messages.remove(m)),
            _ => messages[m].wipe(),
        }
        self.persist();
        self.deliver(i);
        Ok(())
    }

    /// Clears chat `i`'s history on this device: the contact and our other
    /// devices keep theirs. What is on its way stays: a message not sent
    /// yet, a document still coming in. Dropped messages are wiped.
    pub fn clear_chat(&mut self, i: usize) -> Result<(), String> {
        let c = self.data.contacts.get_mut(i).ok_or("no such chat")?;
        c.messages.retain(on_its_way);
        c.unread = 0;
        self.persist();
        Ok(())
    }

    /// As `clear_chat`, for group `g`, with its reactions.
    pub fn clear_group_history(&mut self, g: usize) -> Result<(), String> {
        let group = self.data.groups.get_mut(g).ok_or("no such group")?;
        group.messages.retain(on_its_way);
        let kept: Vec<u64> = group.messages.iter().filter_map(|m| m.id).collect();
        group.reactions.retain(|(id, _, _)| kept.contains(id));
        group.unread = 0;
        self.persist();
        Ok(())
    }

    /// Our reaction to message `m` of chat `i`, or none (ADR-025).
    pub fn react(&mut self, i: usize, m: usize, reaction: Option<u8>) -> Result<(), String> {
        if reaction.is_some_and(|r| r >= proto::REACTIONS) {
            return Err("no such reaction".into());
        }
        let target = self.target(i, m)?;
        let c = &mut self.data.contacts[i];
        c.messages[m].my_reaction = reaction;
        c.outbox.push(Body::React { target, reaction });
        self.persist();
        self.deliver(i);
        Ok(())
    }

    /// Queues a document to contact `i` (ADR-017); its chunks go out at the
    /// pace `send_chunks` allows.
    /// ADR-037: what the safety-number QR code of contact `i` holds. Capitals
    /// and digits only, so the code stays small (QR alphanumeric mode).
    pub fn safety_code(&self, i: usize) -> Option<String> {
        let c = self.data.contacts.get(i)?;
        let number = termchat_core::safety_number(&self.me_for(c), &c.key);
        Some(format!("{SAFETY_CODE}{}", number.replace(' ', "")))
    }

    /// ADR-037: a code scanned from the other screen. Equal: contact `i` is
    /// verified here and true. Anything else: false and nothing changes.
    pub fn verify_by_code(&mut self, i: usize, scanned: &str) -> bool {
        let scanned = scanned.trim();
        // Scanned text comes from outside: `get`, never a slice that could
        // split a character.
        let same = self.safety_code(i).is_some_and(|code| {
            let at = SAFETY_CODE.len();
            scanned
                .get(..at)
                .is_some_and(|start| start.eq_ignore_ascii_case(SAFETY_CODE))
                && scanned.get(at..) == code.get(at..)
        });
        if same {
            self.edit(i, |c| c.verified = true);
        }
        same
    }

    /// File name and plain text of chat `i`, for an export the person asked for.
    pub fn export_chat(&self, i: usize) -> Option<(String, String)> {
        let c = self.data.contacts.get(i)?;
        let them = clean(&self.name(c));
        let who = |m: &Msg| if m.mine { "you".into() } else { them.clone() };
        let title = format!("termchat.app chat with {them}");
        let name = safe_name(&format!("termchat-{them}-{}.txt", &utc_minute(now())[..10]));
        Some((name, export_text(&title, &c.messages, who)))
    }

    /// The same for group `g`, with each author's name.
    pub fn export_group(&self, g: usize) -> Option<(String, String)> {
        let group = self.data.groups.get(g)?;
        let who = |m: &Msg| match (m.mine, m.author) {
            (true, _) => "you".into(),
            (false, Some(k)) => clean(group.name_of(&k).unwrap_or("someone")),
            (false, None) => "someone".into(),
        };
        let name = clean(group.name());
        let title = format!("termchat.app group {name}");
        let file = safe_name(&format!("termchat-{name}-{}.txt", &utc_minute(now())[..10]));
        Some((file, export_text(&title, &group.messages, who)))
    }

    /// Sends what `forwardable` gave into chat `i`, as a new message of ours:
    /// no mark of where it came from.
    pub fn forward(&mut self, i: usize, what: data::Forward) -> Result<(), String> {
        match what {
            data::Forward::Text(text) => self.send(i, &text),
            data::Forward::File(name, bytes) => self.send_file(i, &name, bytes),
        }
    }

    /// The same into group `g`.
    pub fn forward_group(&mut self, g: usize, what: data::Forward) -> Result<(), String> {
        match what {
            data::Forward::Text(text) => self.send_group(g, &text),
            data::Forward::File(name, bytes) => self.send_group_file(g, &name, bytes),
        }
    }

    pub fn send_file(&mut self, i: usize, name: &str, bytes: Vec<u8>) -> Result<(), String> {
        let c = self.writable(i)?;
        let name = name.trim();
        proto::check_file_name(name).map_err(|_| "this file name cannot be sent")?;
        if bytes.is_empty() {
            return Err("the file is empty".into());
        }
        // ADR-043: an offer nobody has asked for yet does not hold the line.
        let busy = |m: &Msg| {
            m.mine
                && m.file
                    .as_ref()
                    .is_some_and(|f| f.held() && f.large.as_ref().is_none_or(|l| l.wanted))
        };
        if c.messages.iter().any(busy) {
            return Err("wait until the previous file is sent".into());
        }
        let size = match u32::try_from(bytes.len()) {
            Ok(n) if n <= proto::MAX_FILE_BYTES => n,
            // ADR-043: offered first, sent when they ask.
            _ => {
                let key = c.key;
                self.offer_file(i, name, &bytes)?;
                self.persist();
                match self.has_session(&key) {
                    true => self.send_chunks(),
                    false => self.claim(key),
                }
                return Ok(());
            }
        };
        let mut id = [0; 4];
        getrandom::fill(&mut id).map_err(|e| e.to_string())?;
        let (key, expires_in) = (c.key, c.expires_in);
        let at = now();
        self.data.contacts[i].messages.push(Msg {
            mine: true,
            text: name.to_string(),
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Queued,
            file: Some(FileMsg {
                id: u32::from_le_bytes(id),
                name: name.to_string(),
                size,
                done: 0,
                picture: false,
                data: Some(bytes),
                large: None,
            }),
            unfetched: false,
            ..Default::default()
        });
        self.persist();
        match self.has_session(&key) {
            true => self.send_chunks(),
            false => self.claim(key),
        }
        Ok(())
    }

    /// Sends queued file chunks as the budget allows (ADR-017), one file per
    /// contact at a time. The last chunk's relay answer marks the file sent.
    fn send_chunks(&mut self) {
        let (tokens, at) = &mut self.chunk_budget;
        *tokens = (*tokens + at.elapsed().as_secs_f64() / FILE_EVERY).min(FILE_BURST);
        *at = Instant::now();
        let mut sent = false;
        for i in 0..self.data.contacts.len() {
            let c = &self.data.contacts[i];
            let (key, expires_in) = (c.key, c.expires_in);
            let queued = |m: &Msg| {
                m.status == Status::Queued && m.file.as_ref().is_some_and(|f| f.large.is_none())
            };
            let Some(m) = c.messages.iter().position(queued) else {
                continue;
            };
            if c.blocked || c.left || !self.has_session(&key) {
                continue;
            }
            while self.chunk_budget.0 >= 1.0 {
                let Some(f) = self.data.contacts[i].messages[m].file.as_mut() else {
                    break;
                };
                let Some(body) = f.next_chunk(expires_in) else {
                    break;
                };
                // Counted before the send, so a restart never repeats a chunk.
                f.done += 1;
                let last = f.complete();
                self.chunk_budget.0 -= 1.0;
                sent = true;
                // ADR-028: every device of theirs; ours hear of the first chunk.
                let first = matches!(body, Body::File { index: 0, .. });
                let result = self.transmit_each(&key, body, first);
                let msg = &mut self.data.contacts[i].messages[m];
                match result {
                    Ok(id) if last => msg.status = Status::Sending(id),
                    Ok(id) => {
                        self.chunks.insert(id, key);
                        continue;
                    }
                    Err(e) => msg.status = Status::Failed(e),
                }
                if let Some(f) = msg.file.as_mut() {
                    f.data = None;
                }
                break;
            }
        }
        // ADR-027 amendment: documents in groups, from the same budget.
        sent |= self.send_group_chunks();
        sent |= self.send_parts();
        if sent {
            self.persist();
        }
    }

    /// A chunk of our file to `key` was refused for good: the rest cannot
    /// make it whole, so it stops (ADR-020).
    fn file_failed(&mut self, key: &Ed25519Key, why: String) {
        let Some(i) = self.index(key) else { return };
        let sending = |m: &Msg| m.mine && m.file.as_ref().is_some_and(FileMsg::held);
        let msgs = &self.data.contacts[i].messages;
        // ADR-043: the receiver hears too, or it waits for parts that never come.
        if let Some(m) = msgs.iter().position(sending)
            && msgs[m].file.as_ref().is_some_and(|f| f.large.is_some())
        {
            return self.stop_large(i, m, true, Some(why));
        }
        let sending = |m: &&mut Msg| m.mine && m.file.as_ref().is_some_and(FileMsg::held);
        if let Some(m) = self.data.contacts[i].messages.iter_mut().find(sending) {
            m.status = Status::Failed(why);
            for f in m.file.iter_mut() {
                f.data = None;
                f.large
                    .iter_mut()
                    .for_each(|l| (l.held, l.stopped) = (false, true));
            }
            self.persist();
            self.sweep_spool();
        }
    }

    /// Name and bytes of a received file `m` of contact `i` that is whole and
    /// not yet saved or discarded.
    pub fn file_bytes(&self, i: usize, m: usize) -> Option<(String, Vec<u8>)> {
        let msg = self.data.contacts.get(i)?.messages.get(m)?;
        let f = msg.file.as_ref().filter(|f| !msg.mine && f.complete())?;
        match f.large {
            Some(_) => Some((f.name.clone(), self.large_bytes(i, m)?)),
            None => Some((f.name.clone(), f.data.clone()?)),
        }
    }

    /// ADR-044: the bytes of received image `m` of contact `i`, to decode
    /// with `picture::decode` off the interface thread.
    pub fn image_bytes(&self, i: usize, m: usize) -> Result<Vec<u8>, String> {
        let msg = self.data.contacts.get(i).and_then(|c| c.messages.get(m));
        if !msg.is_some_and(|msg| !msg.mine && msg.file.as_ref().is_some_and(FileMsg::viewable)) {
            return Err("no image to show".into());
        }
        Ok(self.file_bytes(i, m).ok_or("no image to show")?.1)
    }

    /// The newest received file of contact `i` that waits to be saved.
    pub fn unsaved_file(&self, i: usize) -> Option<usize> {
        let c = self.data.contacts.get(i)?;
        c.messages.iter().rposition(|m| {
            !m.mine
                && m.file
                    .as_ref()
                    .is_some_and(|f| (f.complete() && f.held()) || f.offered())
        })
    }

    /// Forgets the bytes of received file `m`; the line stays in the chat.
    pub fn discard_file(&mut self, i: usize, m: usize) {
        let msg = self
            .data
            .contacts
            .get_mut(i)
            .and_then(|c| c.messages.get_mut(m));
        if let Some(f) = msg.filter(|m| !m.mine).and_then(|m| m.file.as_mut()) {
            if f.large.is_some() {
                return self.drop_large(i, m);
            }
            f.data = None;
            self.persist();
        }
    }

    /// Writes received file `m` into `dir` under a name not taken yet, mode
    /// 0600, then forgets its bytes (ADR-017). Never overwrites a file.
    pub fn save_file(&mut self, i: usize, m: usize, dir: &Path) -> Result<PathBuf, String> {
        let (name, bytes) = self.file_bytes(i, m).ok_or("no file to save")?;
        let path = data::save_new(dir, &safe_name(&name), &bytes)
            .map_err(|e| format!("cannot save: {e}"))?;
        self.discard_file(i, m);
        Ok(path)
    }

    /// Switches Tor on or off and reconnects every identity; returns what to tell the user.
    pub fn toggle_tor(&mut self) -> String {
        if !self.data.tor && self.data.onion_url.is_none() {
            return "This relay has no onion URL, so Tor is not available.".into();
        }
        self.data.tor = !self.data.tor;
        if let Err(e) = self.relink() {
            self.data.tor = !self.data.tor;
            return e;
        }
        self.persist();
        match self.data.tor {
            true => {
                "Tor on: connecting only through Tor. The first connection takes 10-30 s.".into()
            }
            false => "Tor off: the relay and your provider now see your IP address.".into(),
        }
    }

    /// One connection per identity for all its contacts, or one per contact
    /// (ADR-023, section 7). Queues stay; only the connections change.
    pub fn set_shared_link(&mut self, on: bool) -> Result<String, String> {
        self.data.shared_link = on;
        if let Err(e) = self.relink() {
            self.data.shared_link = !on;
            self.relink()?;
            return Err(e);
        }
        self.persist();
        Ok(match on {
            true => "One connection for all contacts: less battery, but the relay sees which queues are yours.".into(),
            false => "A connection per contact: the relay cannot link your queues by connection.".into(),
        })
    }

    /// Every encrypted frame handed to a link was accepted or refused by its
    /// relay. A one-shot command (ADR-024) waits for this before it exits:
    /// a frame lost then leaves a hole in the peer's sequence, or a contact
    /// who never hears our queue.
    pub fn all_answered(&self) -> bool {
        // A dropped link answers nothing soon: waiting on it only stalls the exit.
        self.links
            .values()
            .all(|l| !l.connected || l.relay.unanswered() == 0)
    }

    /// Starts every link now instead of spread over 0-30 s (ADR-014): for a
    /// one-shot command (ADR-024), which is gone before a spread link comes.
    pub fn connect_now(&mut self) -> Result<(), String> {
        self.relink()
    }

    /// Every link is connected, not only the main identity's.
    pub fn all_connected(&self) -> bool {
        self.links.values().all(|l| l.connected)
    }

    /// Reconnects every link with the current settings.
    fn relink(&mut self) -> Result<(), String> {
        self.links.clear();
        self.connected = false;
        self.sync_links()
    }

    /// Replaces the bridge lines, one per line; empty turns bridges off
    /// (ADR-015). `Ok` says what happens now.
    pub fn set_bridges(&mut self, text: &str) -> Result<String, String> {
        let mut lines = Vec::new();
        let mut kinds: Vec<String> = Vec::new();
        let all = text.split(['\r', '\n']).map(str::trim);
        for (n, line) in all.filter(|l| !l.is_empty()).enumerate() {
            let line = line.trim_start_matches("Bridge ").trim();
            let Some(kind) = crate::bridge_transport(line) else {
                return Err(format!(
                    "line {}: not a bridge line termchat.app can use ({})",
                    n + 1,
                    crate::TRANSPORTS.join(", ")
                ));
            };
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
            // Pasting the same list twice must not double it.
            if !lines.iter().any(|l| l == line) {
                lines.push(line.to_string());
            }
        }
        if !lines.is_empty() && !cfg!(feature = "tor") {
            return Err("this build has no Tor support".into());
        }
        if lines != self.data.bridges {
            // Arti keeps the bridge lines in its folder in the clear. Stop
            // every link and move to a new folder; the old one is erased
            // below, once its client lets go.
            self.links.clear();
            crate::forget_tor();
            self.data.tor_generation += 1;
            let old = std::mem::replace(&mut self.data.bridges, lines);
            if let Err(e) = self.relink() {
                // Back to the old bridges, which worked: never off Tor.
                self.data.bridges = old;
                self.relink()?;
                return Err(e);
            }
        }
        self.persist();
        data::prune_tor_dirs(&self.path, self.data.tor_generation);
        let n = self.data.bridges.len();
        Ok(match (n, pt_program(&self.data)) {
            (0, _) => "No bridges: Tor connects directly to the Tor network.".into(),
            (_, None) => format!(
                "{n} bridge(s) saved, but lyrebird was not found: install it, or set its path."
            ),
            _ if !self.data.tor => format!("{n} bridge(s) saved; they are used once Tor is on."),
            _ => format!(
                "{n} bridge(s) ({}): Tor now enters only through them. The first connection can take a few minutes.",
                kinds.join(", ")
            ),
        })
    }

    /// The bridges, without their secrets: transport and address.
    pub fn bridges(&self) -> Vec<String> {
        let shown = |l: &String| l.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        self.data.bridges.iter().map(shown).collect()
    }

    /// The main device has no backup yet. A linked device never needs one:
    /// a lost one is removed and another is linked (ADR-028).
    pub fn needs_backup(&self) -> bool {
        self.is_primary() && !self.data.backed_up
    }

    /// Writes a backup of the identities and contacts (no messages) to `to`.
    pub fn backup(&mut self, to: &Path, password: &str) -> Result<(), String> {
        if !self.is_primary() {
            return Err(
                "back up on your main device; a lost linked device is simply linked again".into(),
            );
        }
        if let Some(why) = password::weakness(password, &[&self.data.nickname]) {
            return Err(why);
        }
        let key =
            create_file_key(password.as_bytes(), KdfParams::DEFAULT).map_err(|e| e.to_string())?;
        let mut copy = self.data.clone();
        copy.backed_up = true;
        // No messages at all, a pending request's held texts included.
        copy.contacts.iter_mut().for_each(|c| {
            c.messages.clear();
            c.hidden.clear();
            c.held_back = 0;
        });
        let states = (1..self.cores.len())
            .map(|s| self.core_at(s).without_sessions().export())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let app = copy.to_json(&states).map_err(|e| e.to_string())?;
        let bytes = self
            .core()
            .without_sessions()
            .save(&key, &app)
            .map_err(|e| e.to_string())?;
        data::write_atomic(to, &bytes).map_err(|e| e.to_string())?;
        self.data.backed_up = true;
        self.persist();
        data::remember_backup(&self.path, to, &bytes)
            .map_err(|e| format!("backup written, but a wipe will not know it: {e}"))
    }

    fn check_current(&self, current: &str) -> Result<(), String> {
        let file = std::fs::read(&self.path).map_err(|e| e.to_string())?;
        store::open(current.as_bytes(), &file)
            .map(|_| ())
            .map_err(|_| "current password is wrong".into())
    }

    /// Sets the duress password, or turns it off with `None`. `Ok` says what happened.
    pub fn set_duress(&mut self, current: &str, duress: Option<&str>) -> Result<String, String> {
        self.check_current(current)?;
        let Some(duress) = duress else {
            set_duress(&mut self.key, None).map_err(|e| e.to_string())?;
            self.persist();
            return Ok("Duress password turned off.".into());
        };
        if password::distance(current, duress) < DURESS_DISTANCE {
            return Err(format!(
                "must differ from your password by at least {DURESS_DISTANCE} characters"
            ));
        }
        if let Some(why) = password::weakness(duress, &[current, &self.data.nickname]) {
            return Err(why);
        }
        set_duress(&mut self.key, Some(duress.as_bytes())).map_err(|e| e.to_string())?;
        self.persist();
        Ok("Duress password set. Entering it at unlock erases everything.".into())
    }

    pub fn change_password(&mut self, current: &str, new: &str) -> Result<(), String> {
        self.check_current(current)?;
        if let Some(why) = password::weakness(new, &[current, &self.data.nickname]) {
            return Err(why);
        }
        if new == current {
            return Err("that is the current password".into());
        }
        // The duress password itself is unknown here, only its slot.
        if termchat_core::opens_duress(&self.key, new.as_bytes()).map_err(|e| e.to_string())? {
            return Err("that is your duress password".into());
        }
        termchat_core::rekey(&mut self.key, new.as_bytes()).map_err(|e| e.to_string())?;
        if !self.persist() {
            return Err(self.take_note());
        }
        Ok(())
    }

    /// Adds a contact from an invite string and starts the handshake; its
    /// index. `incognito` talks to them from a new identity of its own
    /// (ADR-014). Their relay may be another one (ADR-023, decision 4).
    pub fn add(&mut self, invite: &str, incognito: bool) -> Result<usize, String> {
        let invite = Invite::decode(invite).map_err(|e| match e {
            proto::Error::UnsupportedVersion => {
                "this invite is from an older version; ask them for a new one".to_string()
            }
            e => format!("invalid invite: {e}"),
        })?;
        if !self.peer_addr_ok(&invite.relay_url, invite.onion_url.as_deref()) {
            return Err("invalid invite: its relay address cannot be used".into());
        }
        if self.data.tor && invite.onion_url.is_none() {
            return Err("their relay has no onion address, and Tor is on".into());
        }
        let key = invite.identity.ed25519;
        if self.slot_of(&key).is_some() {
            return Err("that is your own invite".into());
        }
        let existing = self.index(&key);
        if incognito && existing.is_some() {
            return Err("already a contact: incognito works only for someone new".into());
        }
        if let Some(i) = existing
            && self.data.contacts[i].theirs.is_some()
            && self.has_session(&key)
        {
            return Err("already a contact".into());
        }
        let slot = match (incognito, existing) {
            (true, _) => self.new_profile("", None)?,
            (false, Some(i)) => self
                .slot_for(&self.data.contacts[i].key)
                .ok_or(MISSING_PROFILE)?,
            (false, None) => 0,
        };
        let added = self.core_at(slot).add_contact(&invite.identity);
        if let Err(e) = added {
            if incognito {
                let p = self.data.profiles[slot - 1].key;
                self.retire(&p);
            }
            return Err(format!("invalid invite: {e}"));
        }
        // A contact asking again (a lost request, or the move to queues,
        // ADR-023): old sessions cannot write to an invite queue, which reads
        // only pre-key messages. Their name and history stay.
        self.core_at(slot).forget_sessions(&key);
        // Adding their invite is how a rejection is taken back.
        self.data.rejected.retain(|k| *k != key);
        let i = match existing {
            Some(i) => i,
            None => self.data.push_person(Contact {
                key,
                added_at: now(),
                profile: (slot > 0).then(|| self.data.profiles[slot - 1].key),
                next_client_id: getrandom::u64().unwrap_or(0) >> 2,
                ..Default::default()
            }),
        };
        let c = &mut self.data.contacts[i];
        c.request_pending = true;
        c.invite_code = Some(invite.code);
        c.theirs = None;
        c.their_invite = Some(Addr {
            relay_url: invite.relay_url,
            onion_url: invite.onion_url,
            send_id: invite.queue,
        });
        // They learn our queue right after the request.
        c.offer_pending = c.mine.is_some();
        self.persist();
        self.sync_links()?;
        self.ensure_queue(key);
        self.claim(key);
        Ok(i)
    }

    /// The local name, or else the nickname they chose; empty if neither.
    fn base_name(&self, c: &Contact) -> String {
        let nick = self
            .slot_for(&c.key)
            .and_then(|s| self.core_at(s).nickname(&c.key).map(str::to_owned));
        clean(&c.petname.clone().or(nick).unwrap_or_default())
    }

    /// The one contact `query` names, for the command line (ADR-024): its
    /// name as shown here, the name without the fingerprint part, or the
    /// start of the fingerprint (at least 5 digits, spaces ignored). More
    /// than one match is an error, so a message never goes to the wrong one.
    pub fn find(&self, query: &str) -> Result<usize, String> {
        let q = query.trim();
        let digits: String = q.chars().filter(|c| !c.is_whitespace()).collect();
        let by_fingerprint = digits.len() >= 5 && digits.chars().all(|c| c.is_ascii_digit());
        let lower = q.to_lowercase();
        let hits: Vec<usize> = (0..self.data.people())
            .filter(|&i| {
                let c = &self.data.contacts[i];
                match by_fingerprint {
                    true => termchat_core::fingerprint(&c.key)
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>()
                        .starts_with(&digits),
                    false => {
                        !lower.is_empty()
                            && (self.name(c).to_lowercase() == lower
                                || self.base_name(c).to_lowercase() == lower)
                    }
                }
            })
            .collect();
        match hits.as_slice() {
            [i] => Ok(*i),
            [] => Err(format!("no contact called \"{}\"", clean(q))),
            many => Err(format!(
                "\"{}\" matches {} contacts ({}); use more of the fingerprint",
                clean(q),
                many.len(),
                many.iter()
                    .map(|&i| self.name(&self.data.contacts[i]))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// Messages that arrived since the last call, and what the contact did
    /// to messages since (ADR-025), for `termchat recv` (ADR-024): oldest
    /// first, each with its contact's index, and marked as taken. A file
    /// counts once it is whole. A request not yet accepted shows nothing, as
    /// in the apps (ADR-007).
    pub fn take_new(&mut self, from: Option<usize>) -> Vec<(usize, Msg, Fetched)> {
        let mut out = Vec::new();
        for i in 0..self.data.people() {
            if from.is_some_and(|f| f != i) || self.is_request(&self.data.contacts[i].key) {
                continue;
            }
            for m in &mut self.data.contacts[i].messages {
                let whole = m.file.as_ref().is_none_or(|f| f.complete());
                if m.unfetched && whole {
                    m.unfetched = false;
                    out.push((i, m.clone(), Fetched::Message));
                }
                let changes = std::mem::take(&mut m.changes);
                for (happened, what) in [
                    (changes.edit, Fetched::Edit),
                    (changes.delete, Fetched::Delete),
                    (changes.react, Fetched::React),
                    (changes.delivered, Fetched::Delivered),
                    (changes.read, Fetched::Read),
                ] {
                    if happened {
                        out.push((i, m.clone(), what));
                    }
                }
            }
        }
        if !out.is_empty() {
            self.persist();
        }
        out.sort_by_key(|(_, m, _)| m.at);
        out
    }

    /// What the contact is called here. Names are chosen by the peer, so an
    /// unverified contact, or one whose name collides with another contact or with
    /// ours, also shows the start of its fingerprint: "Alice·48213" (ADR-007).
    pub fn name(&self, c: &Contact) -> String {
        let base = |c: &Contact| self.base_name(c);
        let fp = termchat_core::fingerprint(&c.key);
        let name = base(c);
        if name.is_empty() {
            return fp[..5].to_string();
        }
        // O(contacts) per name, O(n²) per frame; cache if lists grow large.
        let collides = name == clean(&self.data.nickname)
            || self
                .data
                .contacts
                .iter()
                .any(|o| o.key != c.key && base(o) == name);
        match !c.verified || collides || mixed_scripts(&name) {
            true => format!("{name}·{}", &fp[..5]),
            false => name,
        }
    }

    /// Where the relay should wake us for `who`'s queue (one of our
    /// identities, or a contact); `None` stops it. Every queue has its own
    /// endpoint, or the relay could link them by it (ADR-012, ADR-023).
    pub fn set_push(&mut self, who: &Ed25519Key, endpoint: Option<String>) {
        let Some(push) = self.push_mut(who) else {
            return;
        };
        *push = endpoint;
        self.persist();
        self.tell_push(who);
    }

    fn tell_push(&mut self, who: &Ed25519Key) {
        let (Some(q), Some(endpoint)) = (
            self.queue_of(who).cloned(),
            self.push_mut(who).map(|p| p.clone()),
        ) else {
            return;
        };
        self.send_home(
            who,
            ClientFrame::SetPush {
                recv_id: q.recv_id,
                token: q.token,
                endpoint,
            },
        );
    }

    /// This version's notes were not opened yet, or a newer one is out.
    pub fn news_unread(&self) -> bool {
        self.update.is_some() || self.data.news_seen != env!("CARGO_PKG_VERSION")
    }

    /// Marks this version's notes as read.
    pub fn news_seen(&mut self) {
        self.data.news_seen = env!("CARGO_PKG_VERSION").into();
        self.persist();
    }

    /// Archived first (owner, 2026-11-22), then pinned, then most recent.
    pub fn resort(&mut self) {
        self.data.contacts.sort_by_key(|c| {
            (
                c.own,
                !c.archived,
                !c.pinned,
                std::cmp::Reverse(c.last_at()),
            )
        });
    }
}

/// One notice per chat, redrawn only when the count changes: the tracker reports
/// the same gap with every later message, and a file is many messages.
fn show_missed(c: &mut Contact, missed: u64) {
    show_missed_in(&mut c.messages, &mut c.missed, missed);
}

/// The same for a group (ADR-030, section 6) or a chat.
pub(crate) fn show_missed_in(messages: &mut Vec<Msg>, shown: &mut u64, missed: u64) {
    if missed == *shown {
        return;
    }
    *shown = missed;
    messages.retain(|m| !(m.status == Status::System && m.text.ends_with(MISSING_SUFFIX)));
    if missed > 0 {
        // The number comes from the peer's `seq`; do not let it look alarming on purpose.
        let n = match missed {
            0..=9999 => missed.to_string(),
            _ => "over 9999".to_string(),
        };
        messages.push(Msg {
            mine: false,
            text: format!("⚠ {n}{MISSING_SUFFIX}"),
            at: now(),
            expires_at: None,
            status: Status::System,
            file: None,
            unfetched: false,
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 2026-11c, findings 3 and 7: what a peer may name as a relay.
    #[test]
    fn a_peer_cannot_point_us_at_this_machine_or_a_plain_socket() {
        let ours = "wss://relay.termchat.app/";
        assert!(peer_addr_ok(ours, "wss://relay.example/", None));
        assert!(peer_addr_ok(
            ours,
            "wss://relay.example/",
            Some("ws://abc.onion/")
        ));
        assert!(!peer_addr_ok(ours, "ws://relay.example/", None));
        assert!(!peer_addr_ok(ours, "ws://127.0.0.1:6379/", None));
        assert!(!peer_addr_ok(ours, "ws://localhost/admin?x=1", None));
        assert!(!peer_addr_ok(
            ours,
            "wss://relay.example/",
            Some("ws://relay.example/")
        ));
        // Our own relay on this machine (tests, a relay beside us): fine.
        assert!(peer_addr_ok(
            "ws://127.0.0.1:8090/",
            "ws://127.0.0.1:8091/",
            None
        ));
    }

    #[test]
    fn a_gap_shows_one_notice_that_follows_the_count() {
        let mut c = Contact::default();
        let notices =
            |c: &Contact| -> Vec<String> { c.messages.iter().map(|m| m.text.clone()).collect() };
        show_missed(&mut c, 1);
        for _ in 0..10 {
            show_missed(&mut c, 1);
        }
        assert_eq!(notices(&c), ["⚠ 1 message(s) missing so far"]);
        show_missed(&mut c, 2);
        assert_eq!(notices(&c), ["⚠ 2 message(s) missing so far"]);
        show_missed(&mut c, 0);
        assert!(c.messages.is_empty());
    }

    #[test]
    fn spans_round_down() {
        assert_eq!(span(59 * 60), "59m");
        assert_eq!(span(2 * 3600), "2h");
        assert_eq!(span(3 * 86_400 + 5), "3d");
        assert_eq!(age(now()), "now");
    }

    #[test]
    fn names_mixing_scripts_are_flagged() {
        assert!(mixed_scripts("\u{0410}lice")); // Cyrillic А, then Latin
        assert!(mixed_scripts("b\u{03BF}b")); // Greek ο between Latin b
        assert!(!mixed_scripts("Alice 42"));
        assert!(!mixed_scripts("Ёжик 🦔"));
        assert!(!mixed_scripts("Ζωή"));
    }

    #[test]
    fn message_text_keeps_lines_but_not_bidi_overrides() {
        assert_eq!(clean_text("a\u{202E}b\nc\u{0007}d"), "ab\ncd");
    }

    #[tokio::test]
    async fn fallback_key_rotates_by_age_whatever_the_relay_says() {
        let dir = std::env::temp_dir().join(format!("termchat-fallback-{}", std::process::id()));
        let setup = Setup {
            relay_url: "ws://127.0.0.1:9/",
            onion_url: None,
            tor: false,
            nickname: "alice",
            password: "correct horse battery staple violin",
        };
        let mut e = Engine::setup(&dir.join("state"), &setup).unwrap();
        let me = e.me();
        e.data.invite_queue = Some(Queue {
            relay_url: setup.relay_url.into(),
            onion_url: None,
            recv_id: [1; 16],
            send_id: [2; 16],
            token: [3; 32],
        });
        let link = Link {
            who: me,
            relay: setup.relay_url.into(),
        };
        // The relay never admits the fallback key was used.
        let count = |e: &mut Engine| {
            let frame = ServerFrame::KeyCount {
                recv_id: [1; 16],
                one_time_left: 50,
                fallback_used: false,
            };
            e.on_relay((link.clone(), Event::Frame(frame)));
        };
        count(&mut e);
        let made = e.data.fallback_since;
        assert!(e.data.fallback_published && made.is_some());
        // Young and unused: it stays.
        count(&mut e);
        assert_eq!(
            (e.data.fallback_since, e.data.old_fallback_since),
            (made, None)
        );

        // Eight days old: replaced, and the one before it is kept for now.
        let old = Some(now() - 8 * 86_400);
        e.data.fallback_since = old;
        count(&mut e);
        assert!(e.data.fallback_since >= made && e.data.old_fallback_since.is_some());
        // Old again while the previous key is still kept: no second rotation.
        e.data.fallback_since = old;
        count(&mut e);
        e.tick();
        assert_eq!(e.data.fallback_since, old);

        // The previous key is forgotten, and the relay says nothing at all:
        // the timer replaces the key, but only while connected.
        e.data.old_fallback_since = None;
        e.tick();
        assert_eq!(e.data.fallback_since, old);
        e.links.get_mut(&link).unwrap().connected = true;
        e.tick();
        assert!(e.data.fallback_since >= made && e.data.old_fallback_since.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn offline(name: &str) -> (Box<Engine>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("termchat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let setup = Setup {
            relay_url: "ws://127.0.0.1:9/",
            onion_url: None,
            tor: false,
            nickname: "me",
            password: "correct horse battery staple violin",
        };
        (Engine::setup(&dir.join("state"), &setup).unwrap(), dir)
    }

    fn contact(key: u8, petname: &str) -> Contact {
        Contact {
            key: [key; 32],
            petname: Some(petname.into()),
            verified: true,
            ..Contact::default()
        }
    }

    #[tokio::test]
    async fn clearing_a_chat_keeps_only_what_is_on_its_way() {
        use data::{FileMsg, Msg};
        let (mut e, dir) = offline("clear");
        e.data.contacts.push(contact(9, "bob"));
        let i = e.data.contacts.len() - 1;
        let msg = |text: &str, status| {
            let mut m = Msg::default();
            m.text = text.into();
            m.status = status;
            m
        };
        let mut coming = msg("half.bin", Status::Received);
        coming.file = Some(FileMsg {
            id: 1,
            name: "half.bin".into(),
            size: 40_000,
            done: 1,
            picture: false,
            data: Some(vec![1; 16_000]),
            large: None,
        });
        let c = &mut e.data.contacts[i];
        c.messages = vec![
            msg("read long ago", Status::Received),
            msg("sent", Status::Sent),
            msg("not sent yet", Status::Queued),
            coming,
        ];
        c.unread = 3;
        e.clear_chat(i).unwrap();
        let c = &e.data.contacts[i];
        let left: Vec<&str> = c.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(left, ["not sent yet", "half.bin"]);
        assert_eq!(c.unread, 0);
        assert!(e.clear_chat(99).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn group_script_requires_the_exact_ack_and_saves_before_handoff() {
        use data::Msg;

        let (mut e, dir) = offline("group-script");
        let g = e.create_group("ops").unwrap();
        let group = e.data.groups[g].id;
        let link = Link {
            who: group::link_who(&group),
            relay: e.data.relay_url.clone(),
        };
        let admin_id = [1; 16];
        e.on_group_frame(
            g,
            ServerFrame::GroupCreated {
                group_id: admin_id,
                send_id: [2; 16],
                token: [3; 32],
            },
        );
        e.on_group_frame(
            g,
            ServerFrame::ReaderAdded {
                group_id: admin_id,
                recv_id: [4; 16],
                send_id: [5; 16],
                token: [6; 32],
            },
        );
        assert!(e.group_access(g));

        let disk = dir.join("state.tmp");
        std::fs::create_dir(&disk).unwrap();
        assert!(matches!(
            e.send_group_text(g, "disk failure"),
            Err(GroupScriptError::State(_))
        ));
        assert!(e.data.groups[g].messages.iter().all(|m| !m.mine));
        std::fs::remove_dir(&disk).unwrap();

        let id = e.send_group_text(g, "queued while offline").unwrap();
        e.links.get_mut(&link).unwrap().connected = true;
        // ADR-042, amendment 3: the backlog is read before anything is written.
        e.group_read.insert(group);
        std::fs::create_dir(&disk).unwrap();
        let client_id = e.data.groups[g].next_client_id;
        e.flush_group(g);
        assert!(matches!(
            e.group_text_accepted(&group, id),
            Err(GroupScriptError::State(_))
        ));
        assert_eq!(e.links[&link].relay.unanswered(), 0);
        e.on_group_frame(g, ServerFrame::Accepted { client_id });
        assert!(matches!(
            e.group_text_accepted(&group, id),
            Err(GroupScriptError::State(_))
        ));
        std::fs::remove_dir(&disk).unwrap();

        let client_id = e.data.groups[g].next_client_id;
        let id = e.send_group_text(g, "only this frame").unwrap();
        e.on_group_frame(
            g,
            ServerFrame::Accepted {
                client_id: client_id + 1,
            },
        );
        assert!(e.group_text_accepted(&group, id).unwrap().is_none());
        e.on_group_frame(g, ServerFrame::Accepted { client_id });
        assert!(e.group_text_accepted(&group, id).unwrap().is_some());
        e.data.groups[g].messages.retain(|m| m.id != Some(id));
        assert!(matches!(
            e.group_text_accepted(&group, id),
            Err(GroupScriptError::State(_))
        ));

        let from = [7; 32];
        let incoming = Msg {
            at: now(),
            text: "current edit".into(),
            author: Some(from),
            id: Some(1),
            status: Status::Received,
            file: None,
            group_unfetched: true,
            ..Msg::default()
        };
        let mut deleted = incoming.clone();
        deleted.id = Some(2);
        deleted.wipe();
        let mut expired = incoming.clone();
        expired.id = Some(3);
        expired.expires_at = Some(now());
        let mut own = incoming.clone();
        own.id = Some(4);
        own.mine = true;
        let mut legacy = incoming.clone();
        legacy.id = Some(5);
        legacy.group_unfetched = false;
        legacy.unfetched = true;
        let mut next = incoming.clone();
        next.id = Some(6);
        next.text = "next batch".into();
        e.data.groups[g]
            .messages
            .extend([incoming, deleted, expired, own, legacy, next]);
        std::fs::create_dir(&disk).unwrap();
        assert!(matches!(
            e.take_group_texts(g, 1),
            Err(GroupScriptError::State(_))
        ));
        std::fs::remove_dir(&disk).unwrap();
        let (got, more) = e.take_group_texts(g, 1).unwrap();
        assert!(more);
        assert_eq!(
            got.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            ["current edit"]
        );
        let (got, more) = e.take_group_texts(g, 1).unwrap();
        assert!(!more);
        assert_eq!(got[0].text, "next batch");
        assert!(e.take_group_texts(g, 1).unwrap().0.is_empty());
        drop(e);
        let Unlocked::Open(mut e) =
            Engine::unlock(&dir.join("state"), b"correct horse battery staple violin").unwrap()
        else {
            panic!("state wiped");
        };
        assert!(e.take_group_texts(g, 1).unwrap().0.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_scanned_safety_code_verifies_only_when_it_matches() {
        let (mut e, dir) = offline("safety-qr");
        let mut c = contact(1, "bob");
        c.verified = false;
        e.data.contacts.push(c);
        e.data.contacts.push(contact(2, "carol"));
        let code = e.safety_code(0).unwrap();
        assert!(
            code.starts_with("TERMCHAT-SAFETY:") && code.len() == 16 + 60,
            "{code}"
        );
        assert!(code[16..].bytes().all(|b| b.is_ascii_digit()));
        let carol = e.safety_code(1).unwrap();
        for wrong in [
            carol.as_str(),
            "TERMCHAT-SAFETY:123",
            "termchat:invite",
            "",
            "TERMCHAT-SAFETY:\u{1f600}",
        ] {
            assert!(!e.verify_by_code(0, wrong), "{wrong}");
            assert!(!e.data.contacts[0].verified);
        }
        assert!(!e.verify_by_code(1, &code), "a mismatch never unverifies");
        assert!(e.data.contacts[1].verified);
        let lower = format!("termchat-safety:{}\n", &code[16..]);
        assert!(e.verify_by_code(0, &lower));
        assert!(e.data.contacts[0].verified);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_dates_without_a_calendar_library() {
        assert_eq!(utc_minute(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_minute(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc_minute(1_794_913_363), "2026-11-17 11:02 UTC");
    }

    #[tokio::test]
    async fn exports_are_plain_text_with_a_warning() {
        let (mut e, dir) = offline("export");
        let mut c = contact(1, "bob");
        let line = |mine, text: &str, at| {
            let mut m = Msg::default();
            m.mine = mine;
            m.text = text.into();
            m.at = at;
            m
        };
        c.messages.push(line(false, "hello\nsecond line", 0));
        c.messages.push(line(true, "hi \u{1b}[31mthere", 60));
        let mut gone = line(false, "secret", 120);
        gone.wipe();
        c.messages.push(gone);
        let mut notice = line(false, "⚠ 1 message missing", 180);
        notice.status = Status::System;
        c.messages.push(notice);
        e.data.contacts.push(c);
        let (name, text) = e.export_chat(0).unwrap();
        assert!(
            name.starts_with("termchat-bob-") && name.ends_with(".txt"),
            "{name}"
        );
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("termchat.app chat with bob, exported "));
        assert!(lines[1].contains("not encrypted"));
        assert_eq!(lines[3], "1970-01-01 00:00 UTC  bob: hello");
        assert_eq!(lines[4], "  second line");
        assert!(lines[5].starts_with("1970-01-01 00:01 UTC  you: hi "));
        assert!(!text.contains('\u{1b}'), "no control characters");
        assert_eq!(lines[6], "1970-01-01 00:02 UTC  bob: (deleted)");
        assert_eq!(lines.len(), 7, "notices stay out");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn archived_chats_lead_then_pinned_ones() {
        let (mut e, dir) = offline("pin");
        let said = |at| {
            let mut m = Msg::default();
            m.at = at;
            m
        };
        for (key, name, at, pinned, archived) in [
            (1, "old", 10, false, false),
            (2, "new", 30, false, false),
            (3, "pinned", 5, true, false),
            (4, "archived", 40, false, true),
        ] {
            let mut c = contact(key, name);
            c.messages.push(said(at));
            c.pinned = pinned;
            c.archived = archived;
            e.data.contacts.push(c);
        }
        e.resort();
        let order: Vec<u8> = e.data.contacts.iter().map(|c| c.key[0]).collect();
        assert_eq!(order, [4, 3, 2, 1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn muted_chats_and_groups_stay_quiet() {
        let (mut e, dir) = offline("mute");
        e.data.contacts.push(contact(1, "loud"));
        e.data.contacts.push(contact(2, "quiet"));
        let g = e.create_group("ops").unwrap();
        e.data.contacts[1].muted_until = Some(u64::MAX);
        e.data.groups[g].muted_until = next_mute(None);
        let group = e.data.groups[g].id;
        let loud = Happened::Alert {
            key: [1; 32],
            request: false,
        };
        e.happened = vec![
            loud.clone(),
            Happened::Alert {
                key: [2; 32],
                request: false,
            },
            Happened::Alert {
                key: [2; 32],
                request: true,
            },
            Happened::GroupAlert { group },
        ];
        e.quiet_muted(0);
        assert_eq!(
            e.happened,
            [
                loud,
                Happened::Alert {
                    key: [2; 32],
                    request: true
                }
            ],
            "a request still alerts"
        );
        e.data.contacts[1].muted_until = Some(now() - 1);
        e.happened = vec![Happened::Alert {
            key: [2; 32],
            request: false,
        }];
        e.quiet_muted(0);
        assert_eq!(e.happened.len(), 1, "an expired mute is off");

        let hour = next_mute(None).unwrap() - now();
        assert!((3599..=3600).contains(&hour));
        let day = next_mute(next_mute(None)).unwrap() - now();
        assert!((86_399..=86_400).contains(&day));
        assert_eq!(next_mute(next_mute(next_mute(None))), Some(u64::MAX));
        assert_eq!(next_mute(Some(u64::MAX)), None);
        assert_eq!(mute_label(Some(u64::MAX)), "always");
        assert_eq!(mute_label(None), "off");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn direct_receipts_keep_authorship_and_heartbeats_stay_out_of_chat() {
        let (mut e, dir) = offline("heartbeat");
        let me = e.core().identity();
        let key = e.core().prekeys(0, true).fallback.unwrap();
        let mut server = Client::default();
        let mut linked = Client::default();
        for peer in [&mut server, &mut linked] {
            e.core().add_contact(&peer.identity()).unwrap();
            peer.add_contact(&me).unwrap();
            peer.start_session(&me.ed25519, &key).unwrap();
        }
        let server_key = server.identity().ed25519;
        let other = [9; 32];
        let address = Addr {
            relay_url: e.home(),
            onion_url: None,
            send_id: [7; 16],
        };
        let device = |client: &Client| {
            let identity = client.identity();
            proto::Device {
                ed25519: identity.ed25519,
                curve25519: identity.curve25519,
                proof: client.device_proof(&server_key),
            }
        };
        let mut devices = proto::DeviceList {
            identity: server_key,
            no: 1,
            devices: vec![device(&server), device(&linked)],
            signature: [0; 64],
        };
        devices.signature = server.sign_devices(&devices).unwrap();
        e.data.contacts.push(Contact {
            key: server_key,
            verified: true,
            theirs: Some(address.clone()),
            their_devices: Some(devices),
            ..Contact::default()
        });
        e.data.contacts.push(contact(9, "other"));
        let pulse = |client: &mut Client, sent_at| {
            client
                .encrypt(&me.ed25519, Body::Heartbeat { sent_at })
                .unwrap()
        };

        let mut ignore = |_, _: &str| {};
        assert!(e.receive(me.ed25519, &pulse(&mut server, 1), &mut ignore));
        assert!(e.receive(other, &pulse(&mut server, 2), &mut ignore));
        assert!(e.receive(server_key, &pulse(&mut linked, 3), &mut ignore));
        assert_eq!(e.take_heartbeat(), None, "wrong queue or device");
        let live = pulse(&mut server, 4);
        assert!(e.receive(server_key, &live, &mut ignore));
        assert_eq!(e.take_heartbeat(), Some((server_key, 4)));
        assert!(e.receive(server_key, &live, &mut ignore));
        assert_eq!(e.take_heartbeat(), None, "repeated delivery");
        assert_eq!(e.data.contacts[0].unread, 0);
        assert!(e.take_new(None).is_empty());
        assert!(
            !e.happened
                .iter()
                .any(|h| matches!(h, Happened::Alert { .. }))
        );

        e.sync_links().unwrap();
        e.live_mut(&server_key, &address.relay_url)
            .unwrap()
            .connected = true;
        let before = e.data.contacts[0].next_client_id;
        e.send_heartbeat(&server_key).unwrap();
        e.send_heartbeat(&server_key).unwrap();
        assert_eq!(e.data.contacts[0].next_client_id, before + 1);

        e.edit(0, |c| c.verified = false);
        assert!(e.send_heartbeat(&server_key).is_err());
        assert!(e.receive(server_key, &pulse(&mut server, 5), &mut ignore));
        assert_eq!(e.take_heartbeat(), None, "verification was removed");

        let reply = "yes 0123456789abcdef";
        let text = |text: &str| Body::Text {
            text: text.into(),
            sent_at: now(),
            expires_in: None,
        };
        let mut observed = Vec::new();
        let mut receive = |queue, client: &mut Client, body| {
            let envelope = client.encrypt(&me.ed25519, body).unwrap();
            assert!(e.receive(queue, &envelope, &mut |key, text| {
                observed.push((key, text.to_owned()));
            }));
            envelope
        };
        receive(me.ed25519, &mut server, text(reply));
        receive(other, &mut server, text(reply));
        receive(server_key, &mut linked, text(reply));
        receive(
            server_key,
            &mut server,
            Body::Copy {
                contact: server_key,
                body: Box::new(text(reply)),
            },
        );
        receive(
            server_key,
            &mut server,
            Body::Message {
                id: 42,
                sent_at: now(),
                expires_in: None,
                reply_to: None,
                text: "question".into(),
            },
        );
        receive(
            server_key,
            &mut server,
            Body::Edit {
                id: 42,
                text: reply.into(),
            },
        );
        let replay = receive(server_key, &mut server, text(reply));
        assert!(e.receive(server_key, &replay, &mut |_, _| panic!(
            "duplicate did not stay suppressed"
        )));
        assert_eq!(
            observed,
            [(server_key, "question".into()), (server_key, reply.into())]
        );

        let recv_id = [8; 16];
        e.data.contacts[0].mine = Some(Queue {
            relay_url: e.home(),
            recv_id,
            send_id: [9; 16],
            token: [10; 32],
            onion_url: None,
        });
        let wrong_link = Link {
            who: server_key,
            relay: "ws://127.0.0.1:10/".into(),
        };
        let envelope = server.encrypt(&me.ed25519, text(reply)).unwrap();
        e.on_frame(
            &wrong_link,
            ServerFrame::Deliver {
                recv_id,
                relay_id: 1,
                envelope,
            },
            &mut |_, _| panic!("another relay is not the contact's own queue"),
        );

        // Replacing the state path with a directory prevents saving after decryption.
        e.path = dir.join("not-a-directory");
        std::fs::create_dir(&e.path).unwrap();
        let unsaved = server.encrypt(&me.ed25519, text(reply)).unwrap();
        assert!(!e.receive(server_key, &unsaved, &mut |_, _| panic!(
            "unsaved reply observed"
        )));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_command_line_finds_exactly_one_contact() {
        let (mut e, dir) = offline("find");
        for (k, n) in [(1, "Alice"), (2, "Bob"), (3, "Bob"), (4, "server-01")] {
            e.data.contacts.push(contact(k, n));
        }
        assert_eq!(e.find("alice"), Ok(0), "case does not matter");
        assert_eq!(e.find("  server-01 "), Ok(3));
        // Shown in groups of five; typed with or without the spaces.
        let fp = termchat_core::fingerprint(&[1; 32]);
        assert_eq!(e.find(&fp[..5]), Ok(0), "by fingerprint");
        assert_eq!(e.find(&fp[..11]), Ok(0), "with the space");
        assert_eq!(e.find(&fp[..11].replace(' ', "")), Ok(0), "without it");
        // Two Bobs: the shown names carry their fingerprints, either works.
        let many = e.find("Bob").unwrap_err();
        assert!(many.contains("2 contacts"), "{many}");
        let bob2 = e.name(&e.data.contacts[2].clone());
        assert_eq!(e.find(&bob2), Ok(2));
        assert!(e.find("Carol").is_err());
        assert!(e.find("").is_err(), "nothing is not everything");
        assert!(
            e.find("1234").is_err(),
            "four digits are a name, not a fingerprint"
        );
        assert!(
            !e.find("evil\u{1b}[2J").unwrap_err().contains('\u{1b}'),
            "the query is cleaned before it is shown"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn recv_takes_each_new_message_once_and_files_only_whole() {
        let (mut e, dir) = offline("take");
        e.data.contacts.push(contact(1, "Alice"));
        e.data.contacts.push(contact(2, "Bob"));
        let msg = |text: &str, at, unfetched| Msg {
            mine: false,
            text: text.into(),
            at,
            expires_at: None,
            status: Status::Received,
            file: None,
            unfetched,
            ..Default::default()
        };
        e.data.contacts[0].messages.push(msg("old", 1, false));
        e.data.contacts[0].messages.push(msg("second", 3, true));
        e.data.contacts[1].messages.push(msg("first", 2, true));
        let mut file = msg("half.pdf", 4, true);
        file.file = Some(FileMsg {
            id: 1,
            name: "half.pdf".into(),
            size: 32_000,
            done: 1,
            picture: false,
            data: Some(vec![0; 16_000]),
            large: None,
        });
        e.data.contacts[1].messages.push(file);

        let texts = |got: &[(usize, Msg, Fetched)]| -> Vec<(usize, String)> {
            got.iter().map(|(i, m, _)| (*i, m.text.clone())).collect()
        };
        assert_eq!(texts(&e.take_new(Some(1))), [(1, "first".into())]);
        assert_eq!(texts(&e.take_new(None)), [(0, "second".into())]);
        assert!(e.take_new(None).is_empty(), "once each");
        // The file comes once its last chunk is in.
        if let Some(f) = e.data.contacts[1].messages[1].file.as_mut() {
            f.done = 2;
        }
        assert_eq!(texts(&e.take_new(None)), [(1, "half.pdf".into())]);

        // A state file from before ADR-024 has no field: all of it counts as taken.
        let json = r#"{"mine":false,"text":"x","at":1,"expires_at":null,"status":"Received"}"#;
        let old: Msg = serde_json::from_str(json).unwrap();
        assert!(!old.unfetched);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn received_file_names_are_made_safe_to_save() {
        for (from, to) in [
            ("report.pdf", "report.pdf"),
            ("Отчёт 2026.pdf", "Отчёт 2026.pdf"),
            (".zshenv", "_.zshenv"),
            (".bash_profile", "_.bash_profile"),
            ("a.txt:evil", "a.txt_evil"),
            ("what?*.txt", "what__.txt"),
            ("CON", "_CON"),
            ("nul.txt", "_nul.txt"),
            ("com1.log", "_com1.log"),
            ("common.log", "common.log"),
            ("name. . ", "name"),
            (" . ", "_"),
            ("a\u{202E}fdp.exe", "afdp.exe"),
        ] {
            assert_eq!(safe_name(from), to, "{from:?}");
        }
    }

    #[test]
    fn plain_ws_only_for_local_or_onion() {
        for ok in [
            "wss://relay.example.org/",
            "ws://localhost:8080/",
            "ws://127.0.0.1:8080",
            "ws://[::1]:8080/",
        ] {
            assert!(relay_url_ok(ok), "{ok}");
        }
        for bad in [
            "ws://relay.example.org/",
            "ws://10.0.0.5:8080/",
            "ws://localhost.evil.org/",
            "http://localhost/",
            "",
            // The host after `@`, `?` or `#` is what the transport connects to.
            "ws://localhost:8080@evil.org/",
            "ws://[::1]@evil.org/",
            "ws://evil.org#.onion",
            "ws://evil.org?x=.onion",
            "ws://evil.org/.onion",
            // Onion addresses go in their own field, used only through Tor.
            "ws://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuv.onion/",
            "wss://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuv.onion/",
        ] {
            assert!(!relay_url_ok(bad), "{bad}");
        }
        assert_eq!(ws_host("ws://x.onion/.onion/").as_deref(), Some("x.onion"));
        assert_eq!(
            ws_host("ws://evil.org/x.onion").as_deref(),
            Some("evil.org")
        );
    }
}
