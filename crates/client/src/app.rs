//! TUI state and event handling, without any drawing.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use termchat_core::proto::Ed25519Key;

use crate::data::{self, Contact};
use crate::engine::{self, Engine, Happened, Setup, Unlocked, next_timer};
use crate::form::{self, Action, Form, Kind};
use crate::input::Input;
use crate::notify;
use crate::settings::{self, Settings};
use crate::ui::Hits;
use crate::{RelayEvent, theme};

/// Idle time before the client locks itself.
pub const AUTOLOCK: Duration = Duration::from_secs(5 * 60);
const RELEASES: &str = "https://github.com/prophecy1345/termchat/releases";

/// How this copy was installed decides how to update it (ADR-010).
fn update_command() -> Option<&'static str> {
    // Homebrew runs it through a symlink into its Cellar.
    let exe = std::env::current_exe().and_then(std::fs::canonicalize);
    let exe = exe.unwrap_or_default();
    let exe = exe.to_string_lossy();
    if exe.contains("/Cellar/") {
        Some("brew update && brew upgrade termchat")
    } else if cfg!(windows) {
        Some(
            "irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex",
        )
    } else if exe.ends_with(".local/bin/termchat") {
        Some(
            "curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | sh",
        )
    } else {
        None
    }
}

/// What Ctrl+T does, e.g. "off → 5m" or "7d → off".
fn timer_step(now: Option<u32>) -> String {
    let name = |t: Option<u32>| t.map_or("off".into(), |s| crate::ui::span(s.into()));
    format!("{} → {}", name(now), name(next_timer(now)))
}

pub struct App {
    pub screen: Screen,
    pub path: PathBuf,
    pub settings: Settings,
    /// Where things were drawn last, for the mouse.
    pub hits: RefCell<Hits>,
    /// The contact list edge is being dragged.
    drag: bool,
    /// Whether the terminal window has focus; `None` until it says.
    pub focused: Option<bool>,
    /// New-message cards, oldest first.
    pub toasts: Vec<Toast>,
    last_sound: Option<Instant>,
    pub quit: bool,
}

pub enum Screen {
    /// First run or restore from backup.
    Setup(Form),
    Unlock(Unlock),
    Main(Box<Main>),
}

#[derive(Default)]
pub struct Unlock {
    pub input: Input,
    pub error: String,
}

pub enum Popup {
    /// A long text with a filter: the keys or the manual.
    Pager {
        doc: Doc,
        filter: Input,
        /// Rows scrolled down from the top.
        scroll: usize,
    },
    Settings {
        tab: usize,
        at: usize,
    },
    Notice(String),
    /// A wipe is waiting for the relay (ADR-009).
    Leaving,
    /// The relay cannot be told; the reason, then "wipe anyway?".
    WipeAnyway(String),
    /// Ctrl+P, with the highlighted row.
    Menu(usize),
    /// Right click on a contact, with the highlighted row.
    Actions(usize),
    Form(Form),
    Add(Input),
    /// The invite shown and its kind (ADR-013, ADR-014).
    Invite {
        text: String,
        kind: Share,
    },
    /// The nickname for a new incognito identity.
    Incognito(Input),
    Verify,
    Rename(Input),
    Delete,
    /// The exact text that would be disclosed to the relay operator.
    Report(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    /// Works once.
    Once,
    /// Reusable until renewed.
    Public,
    /// Works once, from a new identity of its own.
    Incognito,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doc {
    Keys,
    Manual,
    News,
}

/// Tabs of the settings window.
pub const TABS: [&str; 4] = ["theme", "notifications", "interface", "security"];
/// Mouse-only keys: a click on settings row `i` is `F(ROW_KEY + i)`, on tab
/// `i` it is `F(TAB_KEY + i)`. No keyboard has these.
pub const ROW_KEY: u8 = 100;
pub const TAB_KEY: u8 = 200;

/// A new-message card in the corner. Shows a name, never the text (ADR-008).
pub struct Toast {
    pub key: Ed25519Key,
    pub text: String,
    pub until: Instant,
}

const MAX_TOASTS: usize = 4;
const TOAST_FOR: Duration = Duration::from_secs(5);
/// A burst of messages makes one sound, not many.
const SOUND_GAP: Duration = Duration::from_secs(2);
/// A click on card `i` is `F(TOAST_KEY + i)`.
pub const TOAST_KEY: u8 = 150;

pub struct Main {
    pub engine: Box<Engine>,
    pub selected: usize,
    /// History rows scrolled up from the bottom.
    pub scroll: usize,
    pub input: Input,
    pub popup: Option<Popup>,
    /// New messages (`false`) and requests (`true`) to tell the user about.
    pub alerts: Vec<(Ed25519Key, bool)>,
    pub status: String,
    pub last_input: Instant,
    path: PathBuf,
    /// Everything was erased; the app returns to the first-run screen with this note.
    wiped: Option<String>,
}

pub use crate::engine::now;

impl App {
    pub fn new(path: PathBuf) -> Self {
        let screen = match path.exists() {
            true => Screen::Unlock(Unlock::default()),
            false => {
                let _ = data::remove_tor_dir(&path);
                Screen::Setup(Form::new(Kind::Setup))
            }
        };
        Self {
            screen,
            settings: Settings::load(&settings::path(&path)),
            hits: RefCell::default(),
            drag: false,
            focused: None,
            toasts: Vec::new(),
            last_sound: None,
            path,
            quit: false,
        }
    }

    /// The terminal window title: an unread count, never a name (ADR-008).
    pub fn title(&self) -> String {
        let unread: usize = match &self.screen {
            Screen::Main(m) => m.engine.data.contacts.iter().map(|c| c.unread).sum(),
            _ => 0,
        };
        match unread {
            0 => "termchat".into(),
            n => format!("termchat ({n})"),
        }
    }

    /// After a wipe the device must look freshly installed, theme included.
    fn wiped(&mut self, note: String) {
        self.settings = Settings::default();
        self.toasts.clear();
        let mut form = Form::new(Kind::Setup);
        form.error = note;
        self.screen = Screen::Setup(form);
    }

    /// Next relay event while unlocked; never resolves otherwise.
    pub async fn relay_event(&mut self) -> RelayEvent {
        if let Screen::Main(m) = &mut self.screen
            && let Some(ev) = m.engine.events.recv().await
        {
            return ev;
        }
        std::future::pending().await
    }

    pub fn on_relay(&mut self, ev: RelayEvent) {
        let Screen::Main(m) = &mut self.screen else {
            return;
        };
        m.on_relay(ev);
        if let Some(note) = m.wiped.take() {
            return self.wiped(note);
        }
        m.resort();
        for (key, request) in std::mem::take(&mut m.alerts) {
            // Already in front of the user's eyes.
            let open = m.current().is_some_and(|c| c.key == key);
            if open && self.focused != Some(false) {
                continue;
            }
            let Some(c) = m.engine.index(&key).map(|i| &m.engine.data.contacts[i]) else {
                continue;
            };
            let what = if request {
                "contact request"
            } else {
                "new message"
            };
            if self.toasts.len() >= MAX_TOASTS {
                self.toasts.remove(0);
            }
            self.toasts.push(Toast {
                key,
                text: format!("{what} · {}", crate::ui::name(m, c)),
                until: Instant::now() + TOAST_FOR,
            });
            let quiet = self.last_sound.is_some_and(|t| t.elapsed() < SOUND_GAP);
            if self.settings.sound && !quiet {
                self.last_sound = Some(Instant::now());
                notify::sound();
            }
            if self.settings.notify && self.focused != Some(true) {
                notify::system();
            }
        }
    }

    pub fn on_tick(&mut self) {
        self.toasts.retain(|t| t.until > Instant::now());
        if let Screen::Main(m) = &mut self.screen {
            if m.last_input.elapsed() >= AUTOLOCK {
                self.lock();
            } else {
                m.engine.tick();
                m.absorb();
                if let Some(note) = m.wiped.take() {
                    self.wiped(note);
                }
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
            Screen::Setup(f) if ctrl && k.code == KeyCode::Char('b') && f.kind == Kind::Setup => {
                self.screen = Screen::Setup(Form::new(Kind::Restore));
            }
            Screen::Setup(f) => match f.on_key(k) {
                Action::Cancel if f.kind == Kind::Restore => {
                    self.screen = Screen::Setup(Form::new(Kind::Setup))
                }
                Action::Submit if f.kind == Kind::Restore => {
                    match engine::restore(&self.path, &form::expand(&f.fields[0])) {
                        Ok(()) => {
                            let hint = "restored: enter the backup's password".to_string();
                            self.screen = Screen::Unlock(Unlock {
                                error: hint,
                                ..Unlock::default()
                            });
                        }
                        Err(e) => f.error = e,
                    }
                }
                Action::Submit => match setup(f, &self.path) {
                    Ok(main) => self.screen = Screen::Main(main),
                    Err(e) => f.error = e,
                },
                _ => {}
            },
            Screen::Unlock(u) => match u.on_key(k, &self.path) {
                Unlocking::Stay => {}
                Unlocking::Open(main) => self.screen = Screen::Main(main),
                Unlocking::Wiped => self.wiped(String::new()),
            },
            Screen::Main(m) => {
                m.last_input = Instant::now();
                let hits = self.hits.borrow().clone();
                let pager = matches!(m.popup, Some(Popup::Pager { .. }));
                let view = if pager { hits.pager } else { hits.history };
                let page = isize::try_from(view.height.max(2) - 1).unwrap_or(1);
                let by = match k.code {
                    KeyCode::PageUp => Some(page),
                    KeyCode::PageDown => Some(-page),
                    KeyCode::Up if pager => Some(1),
                    KeyCode::Down if pager => Some(-1),
                    _ => None,
                };
                if ctrl && k.code == KeyCode::Char('k') {
                    self.lock();
                } else if let KeyCode::F(f) = k.code
                    && (TOAST_KEY..TOAST_KEY + 10).contains(&f)
                {
                    // A click on a card opens that chat.
                    let i = usize::from(f - TOAST_KEY);
                    if i < self.toasts.len() {
                        let toast = self.toasts.remove(i);
                        if let Some(at) = m.engine.index(&toast.key) {
                            m.select(at);
                        }
                    }
                } else if let Some(by) = by.filter(|_| pager || m.popup.is_none()) {
                    scroll(m, by, &hits);
                } else {
                    m.on_key(k, ctrl, &mut self.settings);
                    m.resort();
                    if let Some(note) = m.wiped.take() {
                        self.wiped(note);
                    }
                }
            }
        }
    }

    /// Clicks act like the key a button shows, so the mouse adds no new paths.
    pub fn on_mouse(&mut self, e: MouseEvent) {
        let at = Position::new(e.column, e.row);
        let hits = self.hits.borrow().clone();
        if let Screen::Main(m) = &mut self.screen {
            m.last_input = Instant::now();
        }
        let left = MouseEventKind::Down(MouseButton::Left);
        if e.kind == left
            && let Some((_, k)) = hits.keys.iter().find(|(r, _)| r.contains(at))
        {
            return self.on_key(*k);
        }
        let Screen::Main(m) = &mut self.screen else {
            if hits
                .window
                .is_some_and(|w| e.kind == left && !w.contains(at))
            {
                self.on_key(KeyEvent::from(KeyCode::Esc));
            }
            return;
        };
        if let Some(w) = hits.window {
            match e.kind {
                MouseEventKind::ScrollUp => scroll(m, 3, &hits),
                MouseEventKind::ScrollDown => scroll(m, -3, &hits),
                _ if e.kind == left && !w.contains(at) => self.on_key(KeyEvent::from(KeyCode::Esc)),
                _ => {}
            }
            return;
        }
        let row = |r: ratatui::layout::Rect| hits.list_offset + usize::from(at.y - r.y);
        match e.kind {
            MouseEventKind::Down(MouseButton::Left) if hits.list.contains(at) => {
                m.select(row(hits.list))
            }
            MouseEventKind::Down(MouseButton::Right) if hits.list.contains(at) => {
                m.select(row(hits.list));
                if m.current().is_some() {
                    m.popup = Some(Popup::Actions(0));
                }
            }
            MouseEventKind::Down(MouseButton::Left) if hits.edge == Some(at.x) => self.drag = true,
            MouseEventKind::Drag(MouseButton::Left) if self.drag => {
                self.settings.sidebar = (e.column + 1).clamp(18, 40);
            }
            MouseEventKind::Up(MouseButton::Left) if self.drag => {
                self.drag = false;
                if let Err(e) = self.settings.save(&settings::path(&self.path)) {
                    m.status = format!("cannot save settings: {e}");
                }
            }
            MouseEventKind::ScrollUp if hits.list.contains(at) => {
                m.select(m.selected.saturating_sub(1))
            }
            MouseEventKind::ScrollDown if hits.list.contains(at) => m.select(m.selected + 1),
            MouseEventKind::ScrollUp => scroll(m, 3, &hits),
            MouseEventKind::ScrollDown => scroll(m, -3, &hits),
            _ => {}
        }
    }

    /// Bracketed paste: the text goes into the active field as text only.
    /// Control characters are dropped so a pasted string cannot act as keys.
    pub fn on_paste(&mut self, text: &str) {
        let plain: String = text.chars().filter(|c| !c.is_control()).collect();
        match &mut self.screen {
            Screen::Setup(f) => f.paste(&plain),
            Screen::Unlock(u) => u.input.insert(&plain),
            Screen::Main(m) => {
                m.last_input = Instant::now();
                match &mut m.popup {
                    Some(Popup::Add(s) | Popup::Rename(s) | Popup::Incognito(s)) => {
                        s.insert(&plain)
                    }
                    Some(Popup::Form(f)) => f.paste(&plain),
                    Some(_) => {}
                    None => m.input.insert(&crate::ui::clean(text)),
                }
            }
        }
    }

    /// Saves and drops every key and message from memory.
    pub fn lock(&mut self) {
        if let Screen::Main(m) = &mut self.screen {
            m.persist();
        }
        // They carry names.
        self.toasts.clear();
        self.screen = Screen::Unlock(Unlock::default());
    }
}

fn setup(f: &Form, path: &Path) -> Result<Box<Main>, String> {
    let tor = match f.fields[2].trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => return Err("Use Tor: answer y or n".into()),
    };
    let onion = f.fields[1].trim();
    let setup = Setup {
        relay_url: f.fields[0].trim(),
        onion_url: (!onion.is_empty()).then_some(onion),
        tor,
        nickname: f.fields[3].trim(),
        password: f.new_password(4)?,
    };
    let mut main = Main::new(Engine::setup(path, &setup)?, path);
    main.popup = Some(Popup::Notice(
        "Welcome. Click a contact to open the chat, right-click it for actions;\nCtrl+P opens the menu (settings, keys, manual), ? lists the keys.\n\nFirst, back up your identity: Ctrl+P, then b.\nWithout a backup, a lost device means a lost identity.".into(),
    ));
    Ok(main)
}

pub enum Unlocking {
    Stay,
    Open(Box<Main>),
    /// The duress password was entered and the data is gone.
    Wiped,
}

impl Unlock {
    fn on_key(&mut self, k: KeyEvent, path: &Path) -> Unlocking {
        if !self.input.on_key(k) {
            return Unlocking::Stay;
        }
        let result = Engine::unlock(path, self.input.as_bytes());
        self.input = Input::default();
        match result {
            Ok(Unlocked::Open(engine)) => Unlocking::Open(Main::new(engine, path)),
            Ok(Unlocked::Wiped) => Unlocking::Wiped,
            Err(e) => {
                self.error = e;
                Unlocking::Stay
            }
        }
    }
}

impl Main {
    fn new(engine: Box<Engine>, path: &Path) -> Box<Self> {
        let mut main = Box::new(Self {
            engine,
            selected: 0,
            scroll: 0,
            input: Input::default(),
            popup: None,
            alerts: Vec::new(),
            status: String::new(),
            last_input: Instant::now(),
            path: path.to_owned(),
            wiped: None,
        });
        main.absorb();
        main.resort();
        main
    }

    /// Takes in what the engine reported since the last call.
    fn absorb(&mut self) {
        for h in std::mem::take(&mut self.engine.happened) {
            match h {
                Happened::Alert { key, request } => self.alerts.push((key, request)),
                Happened::Note(n) => self.status = n,
                Happened::Notice(n) => self.popup = Some(Popup::Notice(n)),
                Happened::Wiped(n) => self.wiped = Some(n),
                Happened::WipeAnyway(why) => self.popup = Some(Popup::WipeAnyway(why)),
            }
        }
    }

    /// Most recent first; the open chat stays open while rows move.
    fn resort(&mut self) {
        let open = self.current().map(|c| c.key);
        self.engine.resort();
        if let Some(i) = open.and_then(|k| self.engine.index(&k)) {
            self.selected = i;
        }
    }

    fn persist(&mut self) {
        self.engine.persist();
        self.absorb();
    }

    fn on_relay(&mut self, ev: RelayEvent) {
        self.engine.open = self.current().map(|c| c.key);
        self.engine.on_relay(ev);
        // A first contact shows up already open.
        if let Some(c) = self.engine.data.contacts.get_mut(self.selected) {
            c.unread = 0;
        }
        self.absorb();
    }

    pub fn current(&self) -> Option<&Contact> {
        self.engine.data.contacts.get(self.selected)
    }

    fn on_key(&mut self, k: KeyEvent, ctrl: bool, settings: &mut Settings) {
        // Seen by now; a new problem sets it again.
        self.status.clear();
        if let Some(popup) = self.popup.take() {
            self.on_popup_key(popup, k, settings);
            return self.absorb();
        }
        let key = self.current().map(|c| c.key);
        let i = self.selected;
        match (ctrl, k.code) {
            (false, KeyCode::F(1)) => self.popup = Some(pager(Doc::Keys)),
            (false, KeyCode::Char('?')) if self.input.is_empty() => {
                self.popup = Some(pager(Doc::Keys))
            }
            (false, KeyCode::Up) => self.select(self.selected.saturating_sub(1)),
            (false, KeyCode::Down) => self.select(self.selected + 1),
            (true, KeyCode::Char('a')) => self.popup = Some(Popup::Add(Input::default())),
            (true, KeyCode::Char('p')) => self.popup = Some(Popup::Menu(0)),
            (true, KeyCode::Char('o')) => {
                self.popup = Some(Popup::Invite {
                    text: self.engine.invite(),
                    kind: Share::Once,
                });
            }
            (true, KeyCode::Char('f')) if key.is_some() => self.popup = Some(Popup::Verify),
            (true, KeyCode::Char('r')) if key.is_some() => {
                self.popup = Some(Popup::Rename(Input::default()))
            }
            (true, KeyCode::Char('d')) if key.is_some() => self.popup = Some(Popup::Delete),
            (true, KeyCode::Char('y')) => {
                if key.is_some() {
                    self.engine.accept(i);
                }
            }
            (true, KeyCode::Char('t')) => self
                .engine
                .edit(i, |c| c.expires_in = next_timer(c.expires_in)),
            (true, KeyCode::Char('x')) => {
                if let Some(text) = self.engine.report_text(i) {
                    self.popup = Some(Popup::Report(text));
                }
            }
            (true, KeyCode::Char('b')) => {
                self.engine.edit(i, |c| c.blocked = !c.blocked);
                if let Some(c) = self.current() {
                    self.status = match c.blocked {
                        true => "blocked: their messages are dropped unread",
                        false => "unblocked",
                    }
                    .into();
                }
            }
            (true, KeyCode::Char('e')) => self.engine.edit(i, |c| c.no_history = !c.no_history),
            _ if self.input.on_key(k) => self.send(),
            _ => {}
        }
        self.absorb();
    }

    fn select(&mut self, i: usize) {
        if i < self.engine.data.contacts.len() {
            self.selected = i;
            self.scroll = 0;
            self.engine.data.contacts[i].unread = 0;
        }
    }

    /// This version's notes were not opened yet, or a newer one is out.
    pub fn news_unread(&self) -> bool {
        self.engine.update.is_some() || self.engine.data.news_seen != env!("CARGO_PKG_VERSION")
    }

    /// Top of "what's new" when the relay runs a newer version (ADR-010).
    pub fn update_note(&self) -> Option<String> {
        let v = self.engine.update.as_ref()?;
        let how = match update_command() {
            Some(cmd) => format!("To update, quit termchat and run:\n\n```\n{cmd}\n```"),
            None => format!("Download it from {RELEASES}"),
        };
        Some(format!(
            "## termchat {v} is out\n\n{how}\n\nWhat changed: {RELEASES}\n\n"
        ))
    }

    /// Ctrl+P menu: hotkey and label.
    pub fn menu(&self) -> Vec<(char, String)> {
        let nag = if self.engine.data.backed_up {
            ""
        } else {
            "  ●"
        };
        vec![
            ('s', "settings".into()),
            ('k', "keys".into()),
            ('m', "manual".into()),
            (
                'n',
                format!("what's new{}", if self.news_unread() { "  ●" } else { "" }),
            ),
            ('o', "your invite".into()),
            ('a', "add contact".into()),
            ('b', format!("back up your identity{nag}")),
        ]
    }

    /// Rows of a settings tab: label and whether it is ticked.
    pub fn settings_rows(&self, s: &Settings, tab: usize) -> Vec<(String, bool)> {
        let row = |label: &str, on: bool| (label.to_string(), on);
        match tab {
            0 => theme::THEMES
                .iter()
                .map(|(name, _)| row(name, *name == s.theme))
                .collect(),
            1 => vec![
                row("sound for new messages", s.sound),
                row("system notification: \"termchat: new message\"", s.notify),
            ],
            2 => vec![row("mouse (off: the terminal selects text)", s.mouse)],
            _ => vec![
                row(
                    &format!(
                        "back up your identity{}",
                        if self.engine.data.backed_up {
                            ""
                        } else {
                            "  ●"
                        }
                    ),
                    false,
                ),
                row("change password", false),
                row("duress password", false),
                row("Tor", self.engine.data.tor),
                row(
                    "accept requests without an invite code (old invites)",
                    !self.engine.data.invites.codes_only,
                ),
                row("wipe everything from this device", false),
            ],
        }
    }

    /// Settings window keys: Tab/arrows move, Enter or a click acts.
    fn on_settings_key(
        &mut self,
        tab: usize,
        at: usize,
        k: KeyEvent,
        s: &mut Settings,
    ) -> Option<Popup> {
        let n = self.settings_rows(s, tab).len().max(1);
        let theme_at = theme::index(&s.theme);
        let goto = |tab: usize| {
            let tab = tab % TABS.len();
            (tab, if tab == 0 { theme_at } else { 0 })
        };
        let (tab, at) = match k.code {
            KeyCode::Esc => return None,
            KeyCode::Tab | KeyCode::Right => goto(tab + 1),
            KeyCode::BackTab | KeyCode::Left => goto(tab + TABS.len() - 1),
            KeyCode::F(f) if f >= TAB_KEY => goto(usize::from(f - TAB_KEY)),
            KeyCode::F(f) if f >= ROW_KEY => {
                return self.settings_act(tab, usize::from(f - ROW_KEY), s);
            }
            KeyCode::Enter => return self.settings_act(tab, at, s),
            // Moving through themes shows each one at once.
            KeyCode::Up if tab == 0 => return self.settings_act(0, (at + n - 1) % n, s),
            KeyCode::Down if tab == 0 => return self.settings_act(0, (at + 1) % n, s),
            KeyCode::Up => (tab, (at + n - 1) % n),
            KeyCode::Down => (tab, (at + 1) % n),
            _ => (tab, at),
        };
        Some(Popup::Settings { tab, at })
    }

    fn settings_act(&mut self, tab: usize, at: usize, s: &mut Settings) -> Option<Popup> {
        match (tab, at) {
            (3, 0) => return Some(Popup::Form(Form::new(Kind::Backup))),
            (3, 1) => return Some(Popup::Form(Form::new(Kind::Password))),
            (3, 2) => return Some(Popup::Form(Form::new(Kind::Duress))),
            (3, 3) => return Some(Popup::Notice(self.engine.toggle_tor())),
            (3, 4) => {
                let on = self.engine.data.invites.codes_only;
                self.engine.set_codes_only(!on);
                return Some(Popup::Settings { tab, at });
            }
            (3, _) => return Some(Popup::Form(Form::new(Kind::Wipe))),
            (0, _) => {
                s.theme = theme::THEMES
                    .get(at)
                    .map_or(s.theme.clone(), |t| t.0.into())
            }
            (1, 0) => s.sound = !s.sound,
            (1, _) => s.notify = !s.notify,
            _ => s.mouse = !s.mouse,
        }
        if let Err(e) = s.save(&settings::path(&self.path)) {
            self.status = format!("cannot save settings: {e}");
        }
        Some(Popup::Settings { tab, at })
    }

    /// Right-click menu for the open contact: the Ctrl+key and its label.
    pub fn actions(&self) -> Vec<(char, String)> {
        let Some(c) = self.current() else {
            return Vec::new();
        };
        let mut items = Vec::new();
        if self.engine.is_request(&c.key) {
            items.push(('y', "accept request".to_string()));
        }
        let verified = if c.verified {
            "verified"
        } else {
            "not verified"
        };
        items.extend([
            ('r', "rename".into()),
            ('f', format!("safety number ({verified})")),
            ('t', format!("disappearing: {}", timer_step(c.expires_in))),
            (
                'e',
                format!("no history: {}", if c.no_history { "on" } else { "off" }),
            ),
            ('b', if c.blocked { "unblock" } else { "block" }.into()),
            ('x', "report to the relay operator".into()),
            ('d', "delete".into()),
        ]);
        items
    }

    fn send(&mut self) {
        self.scroll = 0;
        let text = std::mem::take(&mut self.input);
        if text.trim().is_empty() && self.current().is_some() {
            return;
        }
        if let Err(e) = self.engine.send(self.selected, &text) {
            self.input = text;
            self.status = e;
        }
    }

    fn on_popup_key(&mut self, popup: Popup, k: KeyEvent, settings: &mut Settings) {
        let i = self.selected;
        let keep = match popup {
            Popup::Notice(_) => None,
            Popup::Leaving => Some(Popup::Leaving),
            Popup::WipeAnyway(_) => {
                if k.code == KeyCode::Char('y') {
                    self.engine.finish_wipe("");
                }
                None
            }
            Popup::Pager { .. } if k.code == KeyCode::Esc => None,
            Popup::Pager {
                doc, mut filter, ..
            } => {
                filter.on_key(k);
                Some(Popup::Pager {
                    doc,
                    filter,
                    scroll: 0,
                })
            }
            Popup::Settings { tab, at } => self.on_settings_key(tab, at, k, settings),
            Popup::Menu(at) => match pick(&self.menu(), at, k) {
                Err(at) => Some(Popup::Menu(at)),
                Ok(None) => None,
                Ok(Some('s')) => {
                    let at = theme::index(&settings.theme);
                    Some(Popup::Settings { tab: 0, at })
                }
                Ok(Some('k')) => Some(pager(Doc::Keys)),
                Ok(Some('m')) => Some(pager(Doc::Manual)),
                Ok(Some('n')) => {
                    self.engine.news_seen();
                    Some(pager(Doc::News))
                }
                Ok(Some('b')) => Some(Popup::Form(Form::new(Kind::Backup))),
                // Invite and add: the same as Ctrl+O and Ctrl+A.
                Ok(Some(c)) => {
                    return self.on_key(
                        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                        true,
                        settings,
                    );
                }
            },
            Popup::Actions(at) => match pick(&self.actions(), at, k) {
                Err(at) => Some(Popup::Actions(at)),
                Ok(None) => None,
                // The same as pressing Ctrl+key in the chat.
                Ok(Some(c)) => {
                    return self.on_key(
                        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                        true,
                        settings,
                    );
                }
            },
            Popup::Form(mut f) => match f.on_key(k) {
                Action::Stay => Some(Popup::Form(f)),
                Action::Cancel => None,
                Action::Submit if f.kind == Kind::Wipe => match f.fields[0].trim() {
                    "WIPE" => self.engine.leave(false).then_some(Popup::Leaving),
                    "MOVED" => {
                        self.engine.leave(true);
                        None
                    }
                    _ => {
                        f.error = "type WIPE or MOVED in capitals".into();
                        Some(Popup::Form(f))
                    }
                },
                Action::Submit => match self.submit(&f) {
                    Ok(done) => Some(Popup::Notice(done)),
                    Err(e) => {
                        f.error = e;
                        Some(Popup::Form(f))
                    }
                },
            },
            Popup::Invite { text, kind } => match k.code {
                KeyCode::Char('c') => {
                    copy_to_clipboard(&text);
                    self.status = "invite copied (if your terminal supports OSC 52)".into();
                    None
                }
                KeyCode::Char('p') if kind == Share::Once => Some(Popup::Invite {
                    text: self.engine.public_invite(false),
                    kind: Share::Public,
                }),
                KeyCode::Char('i') if kind == Share::Once => {
                    Some(Popup::Incognito(Input::default()))
                }
                KeyCode::Char('n') if kind == Share::Public => {
                    self.status = "new public invite: the old one no longer works".into();
                    Some(Popup::Invite {
                        text: self.engine.public_invite(true),
                        kind: Share::Public,
                    })
                }
                KeyCode::Char('r') if kind == Share::Once => {
                    let n = self.engine.revoke_invites();
                    self.status = format!("{n} unused one-time invite(s) revoked");
                    None
                }
                _ => None,
            },
            Popup::Incognito(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => match self.engine.incognito_invite(&s) {
                    Ok(text) => Some(Popup::Invite {
                        text,
                        kind: Share::Incognito,
                    }),
                    Err(e) => {
                        self.status = e;
                        None
                    }
                },
                false => Some(Popup::Incognito(s)),
            },
            Popup::Verify => {
                if k.code == KeyCode::Char('v') {
                    self.engine.edit(i, |c| c.verified = !c.verified);
                }
                None
            }
            Popup::Report(text) => {
                if k.code == KeyCode::Char('y') {
                    self.engine.report(i, text);
                }
                None
            }
            Popup::Delete => {
                if k.code == KeyCode::Char('y') && i < self.engine.data.contacts.len() {
                    self.engine.delete(i);
                    self.selected = i.saturating_sub(1);
                }
                None
            }
            // Ctrl+N: from a new identity of its own (ADR-014).
            Popup::Add(s)
                if k.code == KeyCode::Char('n') && k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                match self.engine.add(&s, true) {
                    Ok(i) => self.select(i),
                    Err(e) => self.status = e,
                }
                None
            }
            Popup::Add(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => {
                    match self.engine.add(&s, false) {
                        Ok(i) => self.select(i),
                        Err(e) => self.status = e,
                    }
                    None
                }
                false => Some(Popup::Add(s)),
            },
            Popup::Rename(mut s) if k.code != KeyCode::Esc => match s.on_key(k) {
                true => {
                    let name = s.trim();
                    let name = (!name.is_empty()).then(|| name.chars().take(64).collect());
                    self.engine.edit(i, |c| c.petname = name);
                    None
                }
                false => Some(Popup::Rename(s)),
            },
            Popup::Add(_) | Popup::Rename(_) | Popup::Incognito(_) => None,
        };
        self.popup = keep;
    }

    /// Settings forms. `Ok` carries what to tell the user.
    fn submit(&mut self, f: &Form) -> Result<String, String> {
        match f.kind {
            Kind::Backup => {
                if f.fields[0].trim().is_empty() {
                    return Err("where should it go? A USB stick, for example".into());
                }
                let path = form::expand(&f.fields[0]);
                self.engine.backup(&path, f.new_password(1)?)?;
                Ok(format!(
                    "Backup written to {}.\nMove it off this device: a wipe erases it only while it\nstays at this path. Restore: Ctrl+B on the first-run screen.",
                    path.display()
                ))
            }
            Kind::Duress => {
                let duress = match f.fields[1].is_empty() {
                    true => None,
                    false => Some(f.new_password(1)?),
                };
                self.engine.set_duress(&f.fields[0], duress)
            }
            Kind::Password => {
                self.engine
                    .change_password(&f.fields[0], f.new_password(1)?)?;
                Ok("Password changed.".into())
            }
            Kind::Setup | Kind::Restore | Kind::Wipe => Ok(String::new()),
        }
    }
}

/// Scrolls the open pager, or else the chat: `by` rows up (positive) or down.
fn scroll(m: &mut Main, by: isize, hits: &Hits) {
    match &mut m.popup {
        Some(Popup::Pager { scroll, .. }) => {
            *scroll = scroll.saturating_add_signed(-by).min(hits.pager_max);
        }
        None => m.scroll = m.scroll.saturating_add_signed(by).min(hits.max_scroll),
        Some(_) => {}
    }
}

fn pager(doc: Doc) -> Popup {
    Popup::Pager {
        doc,
        filter: Input::default(),
        scroll: 0,
    }
}

/// A menu: Up/Down move, Enter or a hotkey picks, any other key closes.
/// `Err` keeps it open with the new highlighted row.
fn pick(items: &[(char, String)], at: usize, k: KeyEvent) -> Result<Option<char>, usize> {
    let n = items.len().max(1);
    match k.code {
        KeyCode::Up => Err((at + n - 1) % n),
        KeyCode::Down => Err((at + 1) % n),
        KeyCode::Enter => Ok(items.get(at).map(|i| i.0)),
        KeyCode::Char(c) => Ok(items.iter().any(|i| i.0 == c).then_some(c)),
        _ => Ok(None),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_step_names_the_next_press() {
        assert_eq!(timer_step(None), "off → 5m");
        assert_eq!(timer_step(Some(3600)), "1h → 1d");
        assert_eq!(timer_step(Some(604_800)), "7d → off");
    }
}
