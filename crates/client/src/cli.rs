//! Scriptable mode (ADR-024): `termchat <command>` runs without the TUI and exits.
//!
//! Exit codes: 0 done, 1 password/state/input error, 2 bad arguments,
//! 3 no such contact or more than one, 4 the relay did not take it in time
//! (it stays queued), 5 cannot send to that contact.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termchat_engine::data::{self, Msg, Status};
use termchat_engine::engine::{DEFAULT_ONION, DEFAULT_RELAY, NEEDS_INVITE};
use termchat_engine::{Engine, Setup, Unlocked, clean, engine::clean_text};
use zeroize::Zeroizing;

pub const COMMANDS: [&str; 7] = [
    "init",
    "invite",
    "add",
    "contacts",
    "send",
    "send-file",
    "recv",
];

const USAGE: &str = "usage:
  termchat init --nickname NAME [--relay URL] [--onion URL] [--tor]
  termchat invite
  termchat add INVITE
  termchat contacts
  termchat send CONTACT [TEXT]          (TEXT from stdin if not given)
  termchat send-file CONTACT PATH
  termchat recv [--from CONTACT] [--wait SECS] [--follow] [--json] [--save-dir DIR]
options for all: --password-file PATH | --password-stdin, --timeout SECS (default 60)
state folder: TERMCHAT_HOME, or ~/.termchat";

/// A failure and its exit code.
struct Fail(i32, String);

fn fail<T>(code: i32, why: impl Into<String>) -> Result<T, Fail> {
    Err(Fail(code, why.into()))
}

#[derive(Default)]
struct Args {
    command: String,
    positional: Vec<String>,
    nickname: Option<String>,
    relay: Option<String>,
    onion: Option<String>,
    tor: bool,
    password_file: Option<PathBuf>,
    password_stdin: bool,
    timeout: Option<u64>,
    wait: Option<u64>,
    follow: bool,
    json: bool,
    save_dir: Option<PathBuf>,
    from: Option<String>,
}

fn parse(args: &[String]) -> Result<Args, Fail> {
    let mut a = Args {
        command: args.first().cloned().unwrap_or_default(),
        ..Args::default()
    };
    let mut it = args.iter().skip(1);
    let value = |it: &mut std::iter::Skip<std::slice::Iter<String>>, flag: &str| {
        it.next()
            .cloned()
            .ok_or_else(|| Fail(2, format!("{flag} needs a value\n{USAGE}")))
    };
    let secs = |s: String, flag: &str| {
        s.parse::<u64>()
            .map_err(|_| Fail(2, format!("{flag} takes whole seconds")))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--nickname" => a.nickname = Some(value(&mut it, arg)?),
            "--relay" => a.relay = Some(value(&mut it, arg)?),
            "--onion" => a.onion = Some(value(&mut it, arg)?),
            "--tor" => a.tor = true,
            "--password-file" => a.password_file = Some(value(&mut it, arg)?.into()),
            "--password-stdin" => a.password_stdin = true,
            "--timeout" => a.timeout = Some(secs(value(&mut it, arg)?, arg)?),
            "--wait" => a.wait = Some(secs(value(&mut it, arg)?, arg)?),
            "--follow" => a.follow = true,
            "--json" => a.json = true,
            "--save-dir" => a.save_dir = Some(data::expand(&value(&mut it, arg)?)),
            "--from" => a.from = Some(value(&mut it, arg)?),
            "--help" | "-h" => return fail(2, USAGE),
            s if s.starts_with("--") => return fail(2, format!("unknown option {s}\n{USAGE}")),
            _ => a.positional.push(arg.clone()),
        }
    }
    if a.password_file.is_some() && a.password_stdin {
        return fail(2, "use one of --password-file and --password-stdin");
    }
    // Before the password is asked for: a typo should not cost a prompt.
    let n = a.positional.len();
    let ok = match a.command.as_str() {
        "init" => n == 0 && a.nickname.is_some(),
        "invite" | "contacts" | "recv" => n == 0,
        "add" => n == 1,
        "send" => n == 1 || n == 2,
        "send-file" => n == 2,
        _ => false,
    };
    if !ok {
        return fail(2, format!("wrong arguments for {}\n{USAGE}", a.command));
    }
    Ok(a)
}

/// Runs one command; the exit code.
pub async fn run(args: &[String]) -> i32 {
    match command(args).await {
        Ok(()) => 0,
        Err(Fail(code, why)) => {
            eprintln!("termchat: {why}");
            code
        }
    }
}

async fn command(args: &[String]) -> Result<(), Fail> {
    let a = parse(args)?;
    let path = data::home().join("state");
    if a.command == "init" {
        return init(&a, &path).await;
    }
    let _lock = lock(&path)?;
    if !path.exists() {
        return fail(1, "no identity here yet: run termchat init first");
    }
    let password = password(&a, false)?;
    let mut e = match Engine::unlock(&path, password.as_bytes()) {
        Ok(Unlocked::Open(e)) => e,
        // The duress password erased everything (ADR-003): say no more than
        // a wrong password would.
        Ok(Unlocked::Wiped) => return fail(1, "wrong password"),
        Err(why) => return fail(1, why),
    };
    drop(password);
    e.connect_now().map_err(|why| Fail(1, why))?;
    let deadline = Instant::now() + Duration::from_secs(a.timeout.unwrap_or(60));
    match a.command.as_str() {
        "invite" => invite(&mut e, deadline).await,
        "add" => add(&mut e, &a, deadline).await,
        "contacts" => {
            contacts(&e);
            Ok(())
        }
        "send" => send(&mut e, &a, deadline).await,
        "send-file" => send_file(&mut e, &a, deadline).await,
        "recv" => recv(&mut e, &a).await,
        other => fail(2, format!("unknown command {other}\n{USAGE}")),
    }
}

/// One process holds a state file: say so at once rather than wait.
fn lock(path: &Path) -> Result<Option<std::fs::File>, Fail> {
    match data::lock(path) {
        Ok(Some(file)) => Ok(Some(file)),
        Ok(None) => fail(1, "this identity is open in another termchat"),
        // A folder without locks (some network drives) must not stop termchat.
        Err(_) => Ok(None),
    }
}

/// From the file, stdin, or typed without echo (ADR-024, section 2); never
/// from the environment. `twice` asks again to confirm a new one.
fn password(a: &Args, twice: bool) -> Result<Zeroizing<String>, Fail> {
    if let Some(file) = &a.password_file {
        check_private(file)?;
        let text = Zeroizing::new(
            std::fs::read_to_string(file)
                .map_err(|e| Fail(1, format!("cannot read {}: {e}", file.display())))?,
        );
        return Ok(Zeroizing::new(first_line(&text)));
    }
    if a.password_stdin {
        let mut line = Zeroizing::new(String::new());
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| Fail(1, format!("cannot read stdin: {e}")))?;
        return Ok(Zeroizing::new(first_line(&line)));
    }
    if !std::io::stdin().is_terminal() {
        return fail(
            2,
            "no terminal to type the password: use --password-file or --password-stdin",
        );
    }
    let first = hidden("password: ")?;
    if twice && *hidden("again: ")? != *first {
        return fail(1, "the passwords differ");
    }
    Ok(first)
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_string()
}

/// The password file must be the user's own and readable by nobody else.
fn check_private(file: &Path) -> Result<(), Fail> {
    let meta = std::fs::metadata(file)
        .map_err(|e| Fail(1, format!("cannot read {}: {e}", file.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != rustix::process::geteuid().as_raw() {
            return fail(1, format!("{} belongs to another user", file.display()));
        }
        if meta.mode() & 0o077 != 0 {
            return fail(
                1,
                format!("others can read {}: run chmod 600 on it", file.display()),
            );
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    Ok(())
}

/// Typed at the terminal, shown as nothing.
fn hidden(prompt: &str) -> Result<Zeroizing<String>, Fail> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let io = |e: std::io::Error| Fail(1, format!("terminal: {e}"));
    crossterm::terminal::enable_raw_mode().map_err(io)?;
    let mut out = Zeroizing::new(String::new());
    let result = loop {
        match read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break fail(1, "cancelled");
                }
                KeyCode::Esc => break fail(1, "cancelled"),
                KeyCode::Backspace => {
                    out.pop();
                }
                KeyCode::Char(c) => out.push(c),
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(io(e)),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    eprintln!();
    result.map(|()| out)
}

/// Feeds relay events and timers until `done` holds or `deadline` passes.
async fn pump(e: &mut Engine, deadline: Instant, done: impl Fn(&Engine) -> bool) -> bool {
    loop {
        if done(e) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::select! {
            Some(ev) = e.events.recv() => e.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
        e.tick();
        e.happened.clear();
    }
}

async fn init(a: &Args, path: &Path) -> Result<(), Fail> {
    let _lock = lock(path)?;
    if path.exists() {
        return fail(
            1,
            format!("an identity already exists in {}", path.display()),
        );
    }
    let nickname = a
        .nickname
        .clone()
        .ok_or_else(|| Fail(2, format!("init needs --nickname\n{USAGE}")))?;
    let relay = a.relay.clone().unwrap_or_else(|| DEFAULT_RELAY.into());
    let onion = match (&a.onion, &a.relay) {
        (Some(o), _) => Some(o.clone()),
        // The project's onion address goes only with the project's relay.
        (None, None) => Some(DEFAULT_ONION.to_string()),
        (None, Some(_)) => None,
    };
    let password = password(a, true)?;
    let setup = Setup {
        relay_url: &relay,
        onion_url: onion.as_deref(),
        tor: a.tor,
        nickname: &nickname,
        password: &password,
    };
    let mut e = Engine::setup(path, &setup).map_err(|why| Fail(1, why))?;
    let deadline = Instant::now() + Duration::from_secs(a.timeout.unwrap_or(60));
    pump(&mut e, deadline, |e| e.data.invite_queue.is_some()).await;
    println!(
        "identity made in {}; fingerprint {}",
        path.parent().unwrap_or(path).display(),
        termchat_core::fingerprint(&e.me())
    );
    Ok(())
}

async fn invite(e: &mut Engine, deadline: Instant) -> Result<(), Fail> {
    if !pump(e, deadline, |e| e.data.invite_queue.is_some()).await {
        return fail(4, "could not reach the relay in time");
    }
    let invite = e.invite().map_err(|why| Fail(4, why))?;
    println!("{invite}");
    Ok(())
}

async fn add(e: &mut Engine, a: &Args, deadline: Instant) -> Result<(), Fail> {
    let [invite] = a.positional.as_slice() else {
        return fail(2, format!("add takes one invite\n{USAGE}"));
    };
    let i = e.add(invite, false).map_err(|why| Fail(1, why))?;
    let sent = pump(e, deadline, |e| {
        let c = &e.data.contacts[i];
        !c.request_pending && !c.offer_pending && c.mine.is_some()
    })
    .await;
    let name = e.name(&e.data.contacts[i]);
    if !sent {
        return fail(
            4,
            format!("request to {name} not sent in time; it goes with the next command"),
        );
    }
    println!("request sent to {name}; they accept it on their device");
    Ok(())
}

fn contacts(e: &Engine) {
    for c in &e.data.contacts {
        let state = match () {
            _ if e.is_request(&c.key) => "request",
            _ if c.left => "left",
            _ if c.blocked => "blocked",
            _ if e.needs_invite(c) => "needs a new invite",
            _ if c.theirs.is_none() => "waiting for them",
            _ => "ok",
        };
        println!(
            "{}\t{}\t{state}",
            e.name(c),
            termchat_core::fingerprint(&c.key)
        );
    }
}

/// The contact `query` names. Not found: what waits on the relay may be
/// the very name (their `ContactAccept`), so connect, take it in, look again.
async fn locate(e: &mut Engine, query: &str, deadline: Instant) -> Result<usize, Fail> {
    if let Ok(i) = e.find(query) {
        return Ok(i);
    }
    pump(e, deadline, Engine::all_connected).await;
    let settle = Instant::now() + Duration::from_secs(3);
    pump(e, settle.min(deadline), |e| e.find(query).is_ok()).await;
    e.find(query).map_err(|why| Fail(3, why))
}

/// The contact `query` names, if it can be written to.
async fn writable(e: &mut Engine, query: &str, deadline: Instant) -> Result<usize, Fail> {
    let i = locate(e, query, deadline).await?;
    let c = &e.data.contacts[i];
    let why = match () {
        _ if c.left => "this contact left",
        _ if c.blocked => "this contact is blocked",
        _ if e.needs_invite(c) => NEEDS_INVITE,
        _ if e.is_request(&c.key) => "accept their request first, in an app",
        _ => return Ok(i),
    };
    fail(5, why)
}

async fn send(e: &mut Engine, a: &Args, deadline: Instant) -> Result<(), Fail> {
    let (query, text) = match a.positional.as_slice() {
        [query, text] => (query, Zeroizing::new(text.clone())),
        [query] if !a.password_stdin => {
            let mut text = Zeroizing::new(String::new());
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| Fail(1, format!("cannot read stdin: {e}")))?;
            (query, text)
        }
        [_] => return fail(2, "with --password-stdin the text goes as an argument"),
        _ => return fail(2, format!("send takes a contact and a text\n{USAGE}")),
    };
    let text = text.strip_suffix('\n').unwrap_or(&text);
    if text.trim().is_empty() {
        return fail(2, "nothing to send");
    }
    let i = writable(e, query, deadline).await?;
    e.send(i, text).map_err(|why| Fail(5, why))?;
    let m = e.data.contacts[i].messages.len() - 1;
    delivered(e, i, m, deadline).await
}

async fn send_file(e: &mut Engine, a: &Args, deadline: Instant) -> Result<(), Fail> {
    let [query, file] = a.positional.as_slice() else {
        return fail(2, format!("send-file takes a contact and a path\n{USAGE}"));
    };
    let i = writable(e, query, deadline).await?;
    let (name, bytes) = data::read_document(&data::expand(file)).map_err(|why| Fail(1, why))?;
    e.send_file(i, &name, bytes).map_err(|why| Fail(5, why))?;
    let m = e.data.contacts[i].messages.len() - 1;
    delivered(e, i, m, deadline).await
}

/// Waits until message `m` of contact `i` is on the relay.
async fn delivered(e: &mut Engine, i: usize, m: usize, deadline: Instant) -> Result<(), Fail> {
    let status = |e: &Engine| e.data.contacts[i].messages.get(m).map(|m| m.status.clone());
    pump(e, deadline, |e| {
        matches!(status(e), Some(Status::Sent | Status::Failed(_)) | None)
    })
    .await;
    match status(e) {
        Some(Status::Sent) => Ok(()),
        Some(Status::Failed(why)) => fail(5, why),
        _ => fail(
            4,
            "the relay did not take it in time; it goes with the next command",
        ),
    }
}

async fn recv(e: &mut Engine, a: &Args) -> Result<(), Fail> {
    if !a.positional.is_empty() {
        return fail(2, format!("recv takes no arguments\n{USAGE}"));
    }
    let from = match &a.from {
        Some(q) => {
            let deadline = Instant::now() + Duration::from_secs(a.timeout.unwrap_or(60));
            Some(locate(e, q, deadline).await?)
        }
        None => None,
    };
    // What waits in the queues arrives within moments of connecting.
    let connect = Instant::now() + Duration::from_secs(a.timeout.unwrap_or(60));
    pump(e, connect, Engine::all_connected).await;
    let settle = Instant::now() + Duration::from_secs(3);
    pump(e, settle, |_| false).await;
    let until = a.wait.map(|s| Instant::now() + Duration::from_secs(s));
    let mut printed = 0;
    loop {
        for (i, m) in e.take_new(from) {
            show(e, a, i, &m)?;
            printed += 1;
        }
        let waiting = until.is_some_and(|u| printed == 0 && Instant::now() < u);
        if !a.follow && !waiting {
            return Ok(());
        }
        let next = Instant::now() + Duration::from_secs(1);
        pump(e, next, |_| false).await;
    }
}

fn show(e: &mut Engine, a: &Args, i: usize, m: &Msg) -> Result<(), Fail> {
    let c = &e.data.contacts[i];
    let (name, fingerprint) = (e.name(c), termchat_core::fingerprint(&c.key));
    let text = match &m.file {
        None => clean_text(&m.text),
        Some(f) => match &a.save_dir {
            Some(dir) => {
                let at = c
                    .messages
                    .iter()
                    .position(|x| x.at == m.at && x.file.as_ref().is_some_and(|y| y.id == f.id));
                match at.map(|at| e.save_file(i, at, dir)) {
                    Some(Ok(path)) => format!("file saved: {}", path.display()),
                    Some(Err(why)) => format!("file {} not saved: {why}", clean(&f.name)),
                    None => format!("file {} not saved", clean(&f.name)),
                }
            }
            None => format!(
                "file {} ({} bytes) arrived; --save-dir saves it",
                clean(&f.name),
                f.size
            ),
        },
    };
    let mut out = std::io::stdout().lock();
    let written = match a.json {
        true => {
            let line = serde_json::json!({
                "from": name, "fingerprint": fingerprint, "at": m.at, "text": text,
            });
            writeln!(out, "{line}")
        }
        false => writeln!(out, "{name}: {}", text.replace('\n', "\n  ")),
    };
    written.map_err(|e| Fail(1, format!("cannot write: {e}")))
}
