//! Scriptable mode (ADR-024): `termchat <command>` runs without the TUI and exits.
//!
//! Exit codes: 0 done, 1 password/state/input error, 2 bad arguments,
//! 3 no such contact or more than one, 4 the relay did not take it in time
//! (it stays queued), 5 cannot send to that contact. See ADR-032 for `ask`
//! and ADR-035 for `run`, which exits with its command's status.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termchat_engine::data::{self, Msg, Status};
use termchat_engine::engine::{
    DEFAULT_ONION, DEFAULT_RELAY, Fetched, NEEDS_INVITE, REACTION_NAMES,
};
use termchat_engine::{Engine, Setup, Unlocked, clean, engine::clean_text};
use zeroize::Zeroizing;

#[cfg(unix)]
mod daemon;
mod group;
#[cfg(unix)]
mod hook;
mod run;
mod unit;

pub const COMMANDS: [&str; 16] = [
    "init",
    "daemon",
    "invite",
    "add",
    "contacts",
    "groups",
    "send",
    "send-file",
    "recv",
    "watch",
    "ask",
    "run",
    "hook",
    "page",
    "resolve",
    "systemd-unit",
];

const USAGE: &str = "usage:
  termchat init --nickname NAME [--relay URL] [--onion URL] [--tor]
  termchat daemon                       (keeps the identity open for the commands below)
  termchat invite
  termchat add INVITE
  termchat contacts
  termchat groups [--json]
  termchat send CONTACT [TEXT] [--expires 30s|5m|1h|1d]   (TEXT from stdin if not given)
  termchat send (--group NAME | --group-id ID) [TEXT]
  termchat send-file CONTACT PATH
  termchat recv [--from CONTACT] [--wait SECS] [--follow] [--json] [--save-dir DIR]
  termchat recv (--group NAME | --group-id ID) [--wait SECS | --follow] [--json]
  termchat watch send WATCHER --every SECS
  termchat watch monitor SERVER --notify OWNER --after SECS
  termchat ask CONTACT QUESTION [--timeout SECS|Nm]   (default 10m)
  termchat run (--to CONTACT | --group NAME | --group-id ID) [--name LABEL]
               [--lines N] [--failures-only] -- COMMAND [ARGS...]
  termchat page --to CONTACT [--then CONTACT ...] [--key KEY] [--every 5m]
                [--escalate 15m] [--give-up 2h] TEXT   (repeats until ack CODE)
  termchat resolve --key KEY
  termchat systemd-unit (CONTACT | --group NAME) [--password-file PATH]
                                        (prints a unit that tells them when a unit fails)
  termchat hook --token-file PATH --route NAME=contact:CONTACT|group:GROUP [...]
                                        (monitoring webhooks; termchat hook --help)
identity options: --password-file PATH | --password-stdin, --timeout SECS (default 60)
state folder: TERMCHAT_HOME, or ~/.termchat";

/// `send --expires`: from 10 seconds to a week, the longest timer in the apps.
const EXPIRES: std::ops::RangeInclusive<u64> = 10..=604_800;

fn expires(seconds: u64) -> Result<u32, Fail> {
    u32::try_from(seconds)
        .ok()
        .filter(|s| EXPIRES.contains(&u64::from(*s)))
        .ok_or_else(|| Fail(2, "--expires takes 10s to 7d".into()))
}

/// `30s`, `5m`, `1h` or `1d`; a bare number counts as seconds.
fn parse_expires(text: &str) -> Result<u32, Fail> {
    let (count, scale) = match text.char_indices().last() {
        Some((at, 's')) => (&text[..at], 1),
        Some((at, 'm')) => (&text[..at], 60),
        Some((at, 'h')) => (&text[..at], 3600),
        Some((at, 'd')) => (&text[..at], 86_400),
        _ => (text, 1),
    };
    let seconds = count
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| count.parse::<u64>().ok())
        .flatten()
        .and_then(|n| n.checked_mul(scale));
    expires(seconds.unwrap_or(0))
}

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
    group: Option<group::Selector>,
    every: Option<u64>,
    after: Option<u64>,
    notify: Option<String>,
    to: Option<String>,
    name: Option<String>,
    lines: Option<usize>,
    failures_only: bool,
    expires: Option<u32>,
    then: Vec<String>,
    key: Option<String>,
    escalate: Option<u64>,
    give_up: Option<u64>,
}

/// ADR-040: `90`, `90s`, `5m`, `2h` or `1d`, in seconds.
fn span(text: &str, flag: &str) -> Result<u64, Fail> {
    let (count, scale) = match text.char_indices().last() {
        Some((at, 's')) => (&text[..at], 1),
        Some((at, 'm')) => (&text[..at], 60),
        Some((at, 'h')) => (&text[..at], 3600),
        Some((at, 'd')) => (&text[..at], 86_400),
        _ => (text, 1),
    };
    count
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| count.parse::<u64>().ok())
        .flatten()
        .and_then(|n| n.checked_mul(scale))
        .filter(|&n| n > 0)
        .ok_or_else(|| Fail(2, format!("{flag} takes a time like 90s, 5m, 2h or 1d")))
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
            "--timeout" => {
                let timeout = value(&mut it, arg)?;
                a.timeout = Some(if a.command == "ask" {
                    let (count, scale) = timeout
                        .strip_suffix('m')
                        .map_or((timeout.as_str(), 1), |m| (m, 60));
                    count
                        .parse::<u64>()
                        .ok()
                        .and_then(|n| n.checked_mul(scale))
                        .filter(|&n| n > 0 && count.bytes().all(|b| b.is_ascii_digit()))
                        .filter(|&n| Instant::now().checked_add(Duration::from_secs(n)).is_some())
                        .ok_or_else(|| {
                            Fail(
                                2,
                                "ask timeout takes positive whole seconds or minutes (10m)".into(),
                            )
                        })?
                } else {
                    secs(timeout, arg)?
                });
            }
            "--wait" => a.wait = Some(secs(value(&mut it, arg)?, arg)?),
            "--follow" => a.follow = true,
            "--json" => a.json = true,
            "--save-dir" => a.save_dir = Some(data::expand(&value(&mut it, arg)?)),
            "--from" => a.from = Some(value(&mut it, arg)?),
            "--group" | "--group-id" => {
                if a.group.is_some() {
                    return fail(2, "use one group selector");
                }
                let query = value(&mut it, arg)?;
                a.group = Some(if arg == "--group" {
                    group::Selector::Name(query)
                } else {
                    group::Selector::Id(group::parse_id(&query)?)
                });
            }
            "--every" if a.command == "page" => a.every = Some(span(&value(&mut it, arg)?, arg)?),
            "--every" => a.every = Some(secs(value(&mut it, arg)?, arg)?),
            "--then" => a.then.push(value(&mut it, arg)?),
            "--key" => a.key = Some(value(&mut it, arg)?),
            "--escalate" => a.escalate = Some(span(&value(&mut it, arg)?, arg)?),
            "--give-up" => a.give_up = Some(span(&value(&mut it, arg)?, arg)?),
            "--after" => a.after = Some(secs(value(&mut it, arg)?, arg)?),
            "--notify" => a.notify = Some(value(&mut it, arg)?),
            "--to" => a.to = Some(value(&mut it, arg)?),
            "--name" => a.name = Some(value(&mut it, arg)?),
            "--lines" => {
                let lines = value(&mut it, arg)?;
                a.lines = Some(
                    lines
                        .parse()
                        .ok()
                        .filter(|&n| n <= run::MAX_LINES)
                        .ok_or_else(|| Fail(2, format!("--lines takes 0 to {}", run::MAX_LINES)))?,
                );
            }
            "--failures-only" => a.failures_only = true,
            "--expires" => a.expires = Some(parse_expires(&value(&mut it, arg)?)?),
            "--help" | "-h" => return fail(2, USAGE),
            "--" if a.command == "ask"
                || a.command == "run"
                || (a.command == "send" && a.group.is_some()) =>
            {
                a.positional.extend(it.cloned());
                break;
            }
            s if a.command == "ask" && s.starts_with("--") => {
                return fail(2, format!("unknown ask option\n{USAGE}"));
            }
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
        "invite" | "contacts" | "groups" | "recv" | "daemon" => n == 0,
        "add" => n == 1,
        "send" if a.group.is_some() => {
            n <= 1 && (n == 1 || !a.password_stdin) && a.expires.is_none()
        }
        "send" => n == 1 || n == 2,
        "send-file" => n == 2,
        "ask" => matches!(a.positional.as_slice(), [to, question]
            if !to.trim().is_empty() && !question.trim().is_empty()),
        "watch" => {
            let valid = |s: Option<u64>| s.is_some_and(|s| s > 0);
            match a.positional.as_slice() {
                [role, _] if role == "send" => {
                    valid(a.every) && a.after.is_none() && a.notify.is_none()
                }
                [role, _] if role == "monitor" => {
                    valid(a.after) && a.every.is_none() && a.notify.is_some()
                }
                _ => false,
            }
        }
        // ADR-040: one text and a chain; the daemon checks the limits.
        "page" => {
            n == 1
                && !a.positional[0].trim().is_empty()
                && a.to.is_some()
                && a.group.is_none()
                && a.key.as_ref().is_none_or(|k| !k.trim().is_empty())
        }
        "resolve" => n == 0 && a.key.as_ref().is_some_and(|k| !k.trim().is_empty()),
        // PLAN D3.5: prints text, opens nothing.
        "systemd-unit" => {
            (n == 1) != a.group.is_some()
                && !a.password_stdin
                && a.wait.is_none()
                && !a.follow
                && !a.json
                && a.timeout.is_none()
        }
        // ADR-035: one recipient, a command, and no stdin taken from it.
        "run" => {
            n >= 1
                && !a.positional[0].is_empty()
                && a.to.is_some() != a.group.is_some()
                && !a.password_stdin
                && a.wait.is_none()
                && !a.follow
                && !a.json
                && a.from.is_none()
                && a.save_dir.is_none()
                && a.name
                    .as_ref()
                    .is_none_or(|l| !l.trim().is_empty() && l.len() <= run::MAX_NAME)
        }
        _ => false,
    };
    let run_only = a.name.is_some() || a.lines.is_some() || a.failures_only;
    let page_only = !a.then.is_empty() || a.escalate.is_some() || a.give_up.is_some();
    let paging = a.command == "page" || a.command == "resolve";
    if !ok
        || (!matches!(a.command.as_str(), "watch" | "page") && a.every.is_some())
        || (a.command != "watch" && (a.after.is_some() || a.notify.is_some()))
        || (a.command != "run" && run_only)
        || (!matches!(a.command.as_str(), "run" | "page") && a.to.is_some())
        || (a.command != "page" && page_only)
        || (!paging && a.key.is_some())
        || (a.command != "send" && a.expires.is_some())
    {
        return fail(2, format!("wrong arguments for {}\n{USAGE}", a.command));
    }
    if a.group.is_some() || a.command == "groups" {
        group::validate_args(&a)?;
    }
    Ok(a)
}

/// Runs one command; the exit code.
pub async fn run(args: &[String]) -> i32 {
    match command(args).await {
        Ok(()) => 0,
        Err(Fail(code, why)) => {
            // An empty reason is `run` passing on its command's status.
            if !why.is_empty() {
                eprintln!("termchat: {why}");
            }
            code
        }
    }
}

async fn command(args: &[String]) -> Result<(), Fail> {
    // ADR-039: flags of its own, no identity options.
    if args.first().is_some_and(|c| c == "hook") {
        #[cfg(unix)]
        return hook::run(&args[1..]).await;
        #[cfg(not(unix))]
        return fail(2, "termchat hook needs Linux or macOS");
    }
    let a = parse(args)?;
    if a.command == "run" {
        return run_job(a).await;
    }
    if a.command == "systemd-unit" {
        let exe = std::env::current_exe().map_err(|e| Fail(1, e.to_string()))?;
        let pw = a
            .password_file
            .as_ref()
            .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()));
        let home = std::path::absolute(data::home()).unwrap_or_else(|_| data::home());
        let user = std::env::var("USER").ok();
        let text = unit::unit(
            a.positional.first().map(String::as_str),
            a.group.as_ref(),
            &exe,
            &home,
            user.as_deref(),
            pw.as_deref(),
        )?;
        print!("{text}");
        return Ok(());
    }
    execute(&a).await
}

/// ADR-035: check the recipient, run the command, then report on it.
async fn run_job(a: Args) -> Result<(), Fail> {
    execute(&a).await?;
    let (program, args) = a
        .positional
        .split_first()
        .ok_or_else(|| Fail(2, USAGE.into()))?;
    let label = a.name.clone().unwrap_or_else(|| run::label(program));
    let lines = a.lines.unwrap_or(run::DEFAULT_LINES);
    let (program, args) = (program.clone(), args.to_vec());
    let outcome = {
        // Signals go to the command; termchat stays to report how it ended.
        #[cfg(unix)]
        let _keep = (
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()),
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()),
        );
        tokio::task::spawn_blocking(move || run::run(&label, &program, &args, lines))
            .await
            .map_err(|why| Fail(1, format!("run: {why}")))?
    };
    if a.failures_only && outcome.code == 0 {
        return Ok(());
    }
    let report = Args {
        command: "send".into(),
        positional: a.to.iter().cloned().chain([outcome.text]).collect(),
        group: a.group.clone(),
        password_file: a.password_file.clone(),
        timeout: a.timeout,
        ..Args::default()
    };
    match (execute(&report).await, outcome.code) {
        (Ok(()), 0) => Ok(()),
        (Err(f), 0) => Err(f),
        (sent, code) => {
            if let Err(Fail(_, why)) = sent {
                eprintln!("termchat: the report was not sent: {why}");
            }
            Err(Fail(code, String::new()))
        }
    }
}

async fn execute(a: &Args) -> Result<(), Fail> {
    let path = data::home().join("state");
    if a.command == "init" {
        return init(a, &path).await;
    }
    if a.command == "ask" {
        #[cfg(unix)]
        return daemon::ask(a, &data::home())
            .await
            .unwrap_or_else(|| fail(6, "ask needs a running termchat daemon for this identity"));
        #[cfg(not(unix))]
        return fail(2, "termchat ask needs Linux or macOS");
    }
    if a.command == "page" || a.command == "resolve" {
        #[cfg(unix)]
        return daemon::ask(a, &data::home())
            .await
            .unwrap_or_else(|| fail(6, "page needs a running termchat daemon for this identity"));
        #[cfg(not(unix))]
        return fail(2, "termchat page needs Linux or macOS");
    }
    if a.command == "watch" {
        #[cfg(unix)]
        return daemon::ask(a, &data::home())
            .await
            .unwrap_or_else(|| fail(1, "watch needs a running termchat daemon for this identity"));
        #[cfg(not(unix))]
        return fail(2, "termchat watch needs Linux or macOS");
    }
    // ADR-029: a running daemon answers without a password.
    #[cfg(unix)]
    if a.command != "daemon"
        && let Some(answer) = daemon::ask(a, &data::home()).await
    {
        return answer;
    }
    #[cfg(not(unix))]
    if a.command == "daemon" {
        return fail(2, "termchat daemon needs Linux or macOS");
    }
    let _lock = lock(&path)?;
    if !path.exists() {
        return fail(1, "no identity here yet: run termchat init first");
    }
    // ADR-035: a job's stdin and terminal belong to the command it runs.
    if a.command == "run" && a.password_file.is_none() {
        return fail(2, "run needs a running daemon or --password-file");
    }
    let password = password(a, false)?;
    let mut e = match Engine::unlock(&path, password.as_bytes()) {
        Ok(Unlocked::Open(e)) => e,
        // The duress password erased everything (ADR-003): say no more than
        // a wrong password would.
        Ok(Unlocked::Wiped) => return fail(1, "wrong password"),
        Err(why) => return fail(1, why),
    };
    drop(password);
    e.connect_now().map_err(|why| Fail(1, why))?;
    let deadline = group::deadline(a.timeout.unwrap_or(60))?;
    if a.group.is_some() || a.command == "groups" {
        let done = group::run(&mut e, a, deadline).await;
        if done.is_ok() && a.command == "send" {
            pump(&mut e, deadline, Engine::all_answered).await;
        }
        return done;
    }
    let done = match a.command.as_str() {
        "invite" => invite(&mut e, deadline).await,
        "add" => add(&mut e, a, deadline).await,
        "contacts" => {
            contacts(&e);
            Ok(())
        }
        "send" => send(&mut e, a, deadline).await,
        "send-file" => send_file(&mut e, a, deadline).await,
        "recv" => recv(&mut e, a).await,
        "run" => writable(&mut e, a.to.as_deref().unwrap_or_default(), deadline)
            .await
            .map(drop),
        #[cfg(unix)]
        "daemon" => daemon::serve(&mut e, &data::home()).await,
        other => fail(2, format!("unknown command {other}\n{USAGE}")),
    };
    // What went to the link must reach the relay before we exit: the ratchet
    // has moved on, so a frame lost now is a hole the contact sees for good.
    if !pump(&mut e, deadline, Engine::all_answered).await && done.is_ok() {
        eprintln!("termchat: the relay did not answer everything in time");
    }
    done
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
    keys_out(&mut e, deadline).await;
    println!(
        "identity made in {}; fingerprint {}",
        path.parent().unwrap_or(path).display(),
        termchat_core::fingerprint(&e.me())
    );
    Ok(())
}

async fn invite(e: &mut Engine, deadline: Instant) -> Result<(), Fail> {
    if !keys_out(e, deadline).await {
        return fail(4, "could not reach the relay in time");
    }
    let invite = e.invite().map_err(|why| Fail(4, why))?;
    println!("{invite}");
    Ok(())
}

/// The invite queue is made and its keys are on their way: without them,
/// whoever takes the invite cannot write until this identity runs again.
async fn keys_out(e: &mut Engine, deadline: Instant) -> bool {
    let ready = pump(e, deadline, |e| {
        e.data.invite_queue.is_some() && e.data.fallback_published
    })
    .await;
    // ponytail: a fixed second for the frame to leave; an ack from the relay
    // would be exact, if one is ever added.
    let flush = Instant::now() + Duration::from_secs(1);
    pump(e, flush.min(deadline), |_| false).await;
    ready
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
    for [name, fingerprint, state] in contact_rows(e) {
        println!("{name}\t{fingerprint}\t{state}");
    }
}

/// Name, fingerprint and state of each contact.
fn contact_rows(e: &Engine) -> Vec<[String; 3]> {
    let rows = e.data.contacts.iter().filter(|c| !c.own).map(|c| {
        let state = match () {
            _ if e.is_request(&c.key) => "request",
            _ if c.left => "left",
            _ if c.blocked => "blocked",
            _ if e.needs_invite(c) => "needs a new invite",
            _ if c.theirs.is_none() => "waiting for them",
            _ => "ok",
        };
        [
            e.name(c),
            termchat_core::fingerprint(&c.key),
            state.to_string(),
        ]
    });
    rows.collect()
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
    can_write(e, i)
}

/// Contact `i`, if it can be written to.
fn can_write(e: &Engine, i: usize) -> Result<usize, Fail> {
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
    let query = &a.positional[0];
    let text = send_input(a, a.positional.get(1))?;
    let text = text.strip_suffix('\n').unwrap_or(&text);
    if text.trim().is_empty() {
        return fail(2, "nothing to send");
    }
    let i = writable(e, query, deadline).await?;
    match a.expires {
        None => e.send(i, text),
        Some(s) => e.send_expiring(i, text, s),
    }
    .map_err(|why| Fail(5, why))?;
    let m = last_mark(e, i);
    delivered(e, i, m, deadline).await
}

fn send_input(a: &Args, text: Option<&String>) -> Result<Zeroizing<String>, Fail> {
    if let Some(text) = text {
        return Ok(Zeroizing::new(text.clone()));
    }
    if a.password_stdin {
        return fail(2, "with --password-stdin the text goes as an argument");
    }
    let mut text = Zeroizing::new(String::new());
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| Fail(1, format!("cannot read stdin: {e}")))?;
    Ok(text)
}

async fn send_file(e: &mut Engine, a: &Args, deadline: Instant) -> Result<(), Fail> {
    let [query, file] = a.positional.as_slice() else {
        return fail(2, format!("send-file takes a contact and a path\n{USAGE}"));
    };
    let i = writable(e, query, deadline).await?;
    let (name, bytes) = data::read_document(&data::expand(file)).map_err(|why| Fail(1, why))?;
    e.send_file(i, &name, bytes).map_err(|why| Fail(5, why))?;
    let m = last_mark(e, i);
    delivered(e, i, m, deadline).await
}

/// What tells a message of ours apart: its id, or its document's. Not its
/// place in the chat, which shifts as older messages disappear.
type Mark = (Option<u64>, Option<u32>);

fn mark(m: &Msg) -> Mark {
    (m.id, m.file.as_ref().map(|f| f.id))
}

/// The mark of the message just sent to contact `i`.
fn last_mark(e: &Engine, i: usize) -> Mark {
    e.data.contacts[i]
        .messages
        .last()
        .map(mark)
        .unwrap_or_default()
}

/// The status of our message `m` to contact `i`; `None` once it is gone.
fn status_of(e: &Engine, i: usize, m: Mark) -> Option<Status> {
    let msgs = &e.data.contacts[i].messages;
    msgs.iter()
        .find(|x| x.mine && mark(x) == m)
        .map(|x| x.status.clone())
}

/// Waits until message `m` of contact `i` is on the relay.
async fn delivered(e: &mut Engine, i: usize, m: Mark, deadline: Instant) -> Result<(), Fail> {
    let status = |e: &Engine| status_of(e, i, m);
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
        for (i, m, what) in e.take_new(from) {
            show(e, a, i, &m, what)?;
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

fn show(e: &mut Engine, a: &Args, i: usize, m: &Msg, what: Fetched) -> Result<(), Fail> {
    let record = record(e, i, m, what, a.save_dir.is_some(), false);
    let at = record["_file"]["at"].as_u64();
    // The bytes leave the engine only once they are on disk.
    if print_record(a, record)?
        && let Some(at) = at.and_then(|at| usize::try_from(at).ok())
    {
        e.discard_file(i, at);
    }
    Ok(())
}

/// One received message as a `recv --json` line. Keys starting with `_`
/// are for `print_record` only; with `files`, a document's bytes come along,
/// and with `hand_over` they leave the engine.
fn record(
    e: &mut Engine,
    i: usize,
    m: &Msg,
    what: Fetched,
    files: bool,
    hand_over: bool,
) -> serde_json::Value {
    let c = &e.data.contacts[i];
    let (name, fingerprint) = (e.name(c), termchat_core::fingerprint(&c.key));
    // Ids as strings: JSON readers lose digits past 2^53.
    let id = m.id.map(|id| id.to_string());
    let text = clean_text(&m.text);
    let mut line = serde_json::json!({
        "from": name, "fingerprint": fingerprint, "at": m.at, "id": id,
    });
    let more = match what {
        Fetched::Message => serde_json::json!({
            "kind": "message",
            "text": text,
            "reply_to": m.reply_to.map(|r| serde_json::json!({
                "id": r.id.to_string(), "mine": !r.theirs,
            })),
        }),
        Fetched::Edit => serde_json::json!({ "kind": "edit", "text": text }),
        Fetched::Delete => serde_json::json!({ "kind": "delete" }),
        Fetched::React => serde_json::json!({
            "kind": "react", "mine": m.mine,
            "reaction": m.their_reaction.map(|r| REACTION_NAMES[usize::from(r)]),
            "_text": text,
        }),
        // ADR-041: about one of ours; a document by its name, never its bytes.
        Fetched::Delivered | Fetched::Read => {
            let kind = if what == Fetched::Read {
                "read"
            } else {
                "delivered"
            };
            let file = m.file.as_ref().map(|f| clean(&f.name));
            return serde_json::json!({
                "from": name, "fingerprint": fingerprint, "at": m.at, "id": id,
                "kind": kind, "file": file, "_text": text,
            });
        }
    };
    if let (Some(line), Some(more)) = (line.as_object_mut(), more.as_object()) {
        line.extend(more.clone());
    }
    if let Some(f) = &m.file {
        let at = c
            .messages
            .iter()
            .position(|x| x.at == m.at && x.file.as_ref().is_some_and(|y| y.id == f.id));
        let bytes = at
            .filter(|_| files)
            .and_then(|at| e.file_bytes(i, at).map(|(_, b)| (at, b)));
        let data = bytes.map(|(at, b)| {
            if hand_over {
                e.discard_file(i, at);
            }
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b)
        });
        line["_file"] =
            serde_json::json!({ "name": clean(&f.name), "size": f.size, "data": data, "at": at });
    }
    line
}

/// Prints a `record`, saving its document under `--save-dir`; `true` if
/// a document was saved.
fn print_record(a: &Args, mut v: serde_json::Value) -> Result<bool, Fail> {
    let mut saved = false;
    let s = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    if let Some(f) = v.as_object_mut().and_then(|o| o.remove("_file")) {
        let name = s(&f, "name");
        let bytes = f["data"].as_str().and_then(|d| {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, d).ok()
        });
        v["text"] = match (&a.save_dir, bytes) {
            (Some(dir), Some(bytes)) => {
                match data::save_new(dir, &termchat_engine::engine::safe_name(&name), &bytes) {
                    Ok(path) => {
                        saved = true;
                        format!("file saved: {}", path.display())
                    }
                    Err(why) => format!("file {name} not saved: {why}"),
                }
            }
            (Some(_), None) => format!("file {name} not saved"),
            (None, _) => format!(
                "file {name} ({} bytes) arrived; --save-dir saves it",
                f["size"].as_u64().unwrap_or(0)
            ),
        }
        .into();
    }
    let text = s(&v, "text");
    let quoted = v.as_object_mut().and_then(|o| o.remove("_text"));
    let name = s(&v, "from");
    let mut out = std::io::stdout().lock();
    let written = match (a.json, s(&v, "kind").as_str()) {
        (true, _) => writeln!(out, "{v}"),
        (false, "edit") => writeln!(out, "{name} edited: {}", text.replace('\n', "\n  ")),
        (false, "delete") => writeln!(out, "{name} deleted a message"),
        (false, kind @ ("delivered" | "read")) => {
            let quoted = quoted.as_ref().and_then(|q| q.as_str()).unwrap_or_default();
            let what = if kind == "read" { "read" } else { "got" };
            writeln!(out, "{name} {what}: {}", first_line(quoted))
        }
        (false, "react") => match v["reaction"].as_str() {
            Some(r) => {
                let quoted = quoted.as_ref().and_then(|q| q.as_str()).unwrap_or_default();
                writeln!(out, "{name} reacted {r} to: {}", first_line(quoted))
            }
            None => writeln!(out, "{name} took back a reaction"),
        },
        (false, _) => writeln!(out, "{name}: {}", text.replace('\n', "\n  ")),
    };
    written.map_err(|e| Fail(1, format!("cannot write: {e}")))?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_takes_seconds_to_days() {
        let ok = |t: &str| parse_expires(t).ok();
        assert_eq!(ok("30s"), Some(30));
        assert_eq!(ok("5m"), Some(300));
        assert_eq!(ok("1h"), Some(3600));
        assert_eq!(ok("7d"), Some(604_800));
        assert_eq!(ok("600"), Some(600));
        for bad in [
            "9s",
            "8d",
            "",
            "m",
            "5x",
            "-5m",
            "+5m",
            "1.5h",
            "99999999999999999999d",
        ] {
            assert!(parse_expires(bad).is_err(), "{bad}");
        }
    }
}
