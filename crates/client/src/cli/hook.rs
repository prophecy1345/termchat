//! ADR-039: `termchat hook`, a webhook receiver for monitoring alerts.
//! It holds no password and no keys: every message goes through the daemon.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixStream};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use super::group::{Request as GroupRequest, Selector, parse_id};
use super::{Fail, check_private, fail};
use crate::alert::{self, Format};
use termchat_engine::data;

const USAGE: &str = "usage:
  termchat hook --token-file PATH --route NAME=contact:CONTACT[:FORMAT] [--route ...]
                [--route NAME=group:GROUP[:FORMAT] | NAME=group-id:ID[:FORMAT]]
                [--listen 127.0.0.1:9099] [--labels LABEL,...] [--quiet HH:MM-HH:MM]
  termchat hook --new-token PATH
FORMAT: auto (default), alertmanager, kuma or text. Needs a running termchat daemon.
--quiet, in UTC: only Alertmanager alerts with severity critical or page go
through; the rest wait and come as one summary per route when it ends.";

/// The request body limit (ADR-039, section 3).
const MAX_BODY: usize = 1 << 20;
/// Connections at once.
const MAX_CONNECTIONS: usize = 16;
/// To read the headers, and again for the body.
const READ_LIMIT: Duration = Duration::from_secs(10);
/// Messages a minute per route; above it the answer is 429.
const PER_MINUTE: usize = 30;
/// How long the daemon may take to hand a message to the relay.
const SEND_TIMEOUT: u64 = 30;
/// At least 32 random bytes, as hex (64) or base64 (43).
const MIN_TOKEN: usize = 43;
/// The same alert on a route within this time is counted, not sent again.
const REPEAT_WINDOW: Duration = Duration::from_secs(600);
/// Alert texts not seen for this long are forgotten, counted repeats too.
const REPEAT_MEMORY: Duration = Duration::from_secs(86_400);
/// Different alerts a route keeps for its summary in quiet hours; more are only counted.
const MAX_HELD: usize = 100;

/// Quiet hours in UTC, as minutes after midnight; `from` after `to` wraps midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Quiet {
    from: u32,
    to: u32,
}

impl Quiet {
    /// `HH:MM-HH:MM`.
    fn parse(spec: &str) -> Option<Self> {
        let minute = |t: &str| {
            let (h, m) = t.trim().split_once(':')?;
            let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
            (h < 24 && m < 60).then_some(h * 60 + m)
        };
        let (from, to) = spec.split_once('-')?;
        let (from, to) = (minute(from)?, minute(to)?);
        (from != to).then_some(Self { from, to })
    }

    fn holds(self, minute: u32) -> bool {
        match self.from < self.to {
            true => (self.from..self.to).contains(&minute),
            false => minute >= self.from || minute < self.to,
        }
    }

    fn now() -> u32 {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        (secs % 86_400 / 60) as u32
    }
}

impl std::fmt::Display for Quiet {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let (a, b) = (self.from, self.to);
        write!(f, "{:02}:{:02}-{:02}:{:02}", a / 60, a % 60, b / 60, b % 60)
    }
}

/// What quiet hours held back on a route: each alert with how often it
/// came, and how many more did not fit.
#[derive(Default)]
struct Held {
    alerts: Vec<(String, u32)>,
    more: u32,
}

impl Held {
    fn add(&mut self, alert: String) {
        let room = self.alerts.len() < MAX_HELD;
        match self.alerts.iter_mut().find(|(a, _)| *a == alert) {
            Some((_, n)) => *n = n.saturating_add(1),
            None if room => self.alerts.push((alert, 1)),
            None => self.more = self.more.saturating_add(1),
        }
    }

    /// One message for all of it.
    fn summary(&self, quiet: Quiet) -> String {
        let total = self.alerts.iter().map(|(_, n)| *n).sum::<u32>() + self.more;
        let mut out = format!("QUIET HOURS {quiet} UTC: {total} alert(s) held");
        for (alert, n) in self.alerts.iter().take(alert::MAX_ALERTS) {
            out.push_str("\n\n");
            out.push_str(alert);
            if *n > 1 {
                out.push_str(&format!("\n(came {n} times)"));
            }
        }
        let left = self.alerts.len().saturating_sub(alert::MAX_ALERTS) as u32 + self.more;
        if left > 0 {
            out.push_str(&format!("\n\nand {left} more"));
        }
        alert::cut(&out)
    }
}

#[derive(Clone)]
enum Target {
    Contact(String),
    Group(Selector),
    /// ADR-040: a firing alert pages the chain, a resolved one resolves it.
    Page {
        chain: Vec<String>,
        escalate: Option<u64>,
    },
}

struct Route {
    target: Target,
    format: Format,
}

struct Hook {
    home: PathBuf,
    token: Zeroizing<String>,
    routes: HashMap<String, Route>,
    labels: Vec<String>,
    sent: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// Per route and alert text: when it was last sent, repeats since.
    repeats: Mutex<HashMap<(String, String), (Instant, u32)>>,
    quiet: Option<Quiet>,
    /// Per route: what quiet hours held back for the summary.
    held: Mutex<HashMap<String, Held>>,
}

/// `termchat hook ...`, `args` without the command name.
pub(super) async fn run(args: &[String]) -> Result<(), Fail> {
    let mut listen: SocketAddr = ([127, 0, 0, 1], 9099).into();
    let mut token_file = None;
    let mut new_token = None;
    let mut routes = HashMap::new();
    let mut labels = Vec::new();
    let mut quiet = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut next = || {
            it.next()
                .cloned()
                .ok_or_else(|| Fail(2, format!("{arg} needs a value\n{USAGE}")))
        };
        match arg.as_str() {
            "--listen" => {
                listen = next()?
                    .parse()
                    .map_err(|_| Fail(2, "--listen takes an address like 127.0.0.1:9099".into()))?;
            }
            "--token-file" => token_file = Some(data::expand(&next()?)),
            "--new-token" => new_token = Some(data::expand(&next()?)),
            "--route" => {
                let (name, route) = parse_route(&next()?)?;
                if routes.insert(name.clone(), route).is_some() {
                    return fail(2, format!("route {name} is given twice"));
                }
            }
            "--labels" => {
                labels = next()?
                    .split(',')
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect();
            }
            "--quiet" => {
                quiet =
                    Some(Quiet::parse(&next()?).ok_or_else(|| {
                        Fail(2, "--quiet takes UTC hours like 22:00-08:00".into())
                    })?);
            }
            "--help" | "-h" => return fail(2, USAGE),
            other => return fail(2, format!("unknown option {other}\n{USAGE}")),
        }
    }
    if let Some(path) = new_token {
        if token_file.is_some() || !routes.is_empty() {
            return fail(2, USAGE);
        }
        return write_token(&path);
    }
    let Some(token_file) = token_file else {
        return fail(2, format!("--token-file is required\n{USAGE}"));
    };
    if routes.is_empty() {
        return fail(2, format!("give at least one --route\n{USAGE}"));
    }
    let token = read_token(&token_file)?;
    let home = data::home();
    for (name, route) in &routes {
        check_route(&home, name, &route.target).await?;
    }
    if !listen.ip().is_loopback() {
        eprintln!(
            "termchat: warning: listening on {listen}: the token crosses the network in \
             the clear unless a TLS proxy stands in front (ADR-039)"
        );
    }
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|e| Fail(1, format!("cannot listen on {listen}: {e}")))?;
    let mut names: Vec<_> = routes.keys().cloned().collect();
    names.sort();
    eprintln!(
        "termchat: hook listening on {listen}, routes: /{}",
        names.join(", /")
    );
    let hook = Arc::new(Hook {
        home,
        token,
        routes,
        labels,
        sent: Mutex::new(HashMap::new()),
        repeats: Mutex::new(HashMap::new()),
        quiet,
        held: Mutex::new(HashMap::new()),
    });
    if quiet.is_some() {
        tokio::spawn(summaries(Arc::clone(&hook)));
    }
    serve(listener, hook).await
}

/// `NAME=contact:CONTACT[:FORMAT]`, `NAME=group:GROUP[:FORMAT]` or `NAME=group-id:ID[:FORMAT]`.
fn parse_route(spec: &str) -> Result<(String, Route), Fail> {
    let bad = || Fail(2, format!("bad route {spec:?}\n{USAGE}"));
    let (name, rest) = spec.split_once('=').ok_or_else(bad)?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return fail(2, format!("a route name takes a-z, 0-9, - and _: {name:?}"));
    }
    let (kind, rest) = rest.split_once(':').ok_or_else(bad)?;
    let (who, format) = match rest.rsplit_once(':') {
        Some((who, f)) if Format::parse(f).is_some() => (who, Format::parse(f).ok_or_else(bad)?),
        _ => (rest, Format::Auto),
    };
    // `page:alice,bob:15m`: the escalation time after the chain.
    let (who, escalate) = match who.rsplit_once(':') {
        Some((chain, time)) if kind == "page" => (chain, Some(super::span(time, "escalation")?)),
        _ => (who, None),
    };
    if who.trim().is_empty() {
        return Err(bad());
    }
    let target = match kind {
        "contact" => Target::Contact(who.to_string()),
        "group" => Target::Group(Selector::Name(who.to_string())),
        "group-id" => Target::Group(Selector::Id(parse_id(who)?)),
        "page" => {
            let chain: Vec<String> = who.split(',').map(|c| c.trim().to_string()).collect();
            if chain.iter().any(String::is_empty) {
                return Err(bad());
            }
            Target::Page { chain, escalate }
        }
        _ => return Err(bad()),
    };
    Ok((name.to_string(), Route { target, format }))
}

fn write_token(path: &Path) -> Result<(), Fail> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut bytes[..]).map_err(|e| Fail(1, format!("no randomness: {e}")))?;
    let hex: Zeroizing<String> = Zeroizing::new(bytes.iter().map(|b| format!("{b:02x}")).collect());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| Fail(1, format!("cannot create {}: {e}", path.display())))?;
    file.write_all(format!("{}\n", hex.as_str()).as_bytes())
        .map_err(|e| Fail(1, format!("cannot write {}: {e}", path.display())))?;
    eprintln!("termchat: a new token is in {}", path.display());
    Ok(())
}

fn read_token(path: &Path) -> Result<Zeroizing<String>, Fail> {
    check_private(path)?;
    let text = Zeroizing::new(
        std::fs::read_to_string(path)
            .map_err(|e| Fail(1, format!("cannot read {}: {e}", path.display())))?,
    );
    let token = text.trim();
    let alphabet =
        |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_');
    if token.len() < MIN_TOKEN || !token.bytes().all(alphabet) {
        return fail(
            1,
            "the token must be at least 32 random bytes as hex or base64: \
             termchat hook --new-token PATH makes one",
        );
    }
    Ok(Zeroizing::new(token.to_string()))
}

/// At start: the route's contact is accepted and verified, its group ours.
async fn check_route(home: &Path, name: &str, target: &Target) -> Result<(), Fail> {
    let nodaemon = || {
        Fail(
            6,
            "hook needs a running termchat daemon for this identity".into(),
        )
    };
    match target {
        Target::Page { chain, .. } => {
            for who in chain {
                check_contact(home, name, who).await?;
            }
        }
        Target::Contact(who) => check_contact(home, name, who).await?,
        Target::Group(group) => {
            let check = GroupRequest::GroupCheck {
                group: group.clone(),
            };
            let req = serde_json::to_value(check).map_err(|e| Fail(1, e.to_string()))?;
            call(home, &req)
                .await
                .ok_or_else(nodaemon)?
                .map_err(|Fail(code, why)| Fail(code, format!("route {name}: {why}")))?;
        }
    }
    Ok(())
}

/// A route's contact is accepted and has a verified safety number.
async fn check_contact(home: &Path, name: &str, who: &str) -> Result<(), Fail> {
    let v = call(home, &json!({ "cmd": "contacts" }))
        .await
        .ok_or_else(|| {
            Fail(
                6,
                "hook needs a running termchat daemon for this identity".into(),
            )
        })??;
    let rows = v["contacts"].as_array().cloned().unwrap_or_default();
    let matches: Vec<&Value> = rows
        .iter()
        .filter(|c| c["name"] == who || c["fingerprint"] == who)
        .collect();
    let [c] = matches.as_slice() else {
        return fail(3, format!("route {name}: no single contact named {who}"));
    };
    if c["state"] != "ok" || c["verified"] != true {
        return fail(
            5,
            format!(
                "route {name}: {who} must be an accepted contact with a verified safety number"
            ),
        );
    }
    Ok(())
}

/// One request to the daemon; its final answer. `None`: no daemon answers.
async fn call(home: &Path, request: &Value) -> Option<Result<Value, Fail>> {
    let stream = UnixStream::connect(home.join(super::daemon::SOCKET))
        .await
        .ok()?;
    if !stream
        .peer_cred()
        .is_ok_and(|cred| cred.uid() == rustix::process::geteuid().as_raw())
    {
        return Some(fail(1, "the daemon socket belongs to another user"));
    }
    let (read, mut write) = stream.into_split();
    if write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .is_err()
    {
        return None;
    }
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            return Some(fail(1, "daemon: bad answer"));
        };
        match v["ok"].as_bool() {
            Some(true) => return Some(Ok(v)),
            Some(false) => {
                let code = v["code"]
                    .as_i64()
                    .and_then(|c| i32::try_from(c).ok())
                    .unwrap_or(1);
                let why = v["error"].as_str().unwrap_or("daemon error").to_string();
                return Some(Err(Fail(code, why)));
            }
            None => continue,
        }
    }
    None
}

async fn serve(listener: TcpListener, hook: Arc<Hook>) -> Result<(), Fail> {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                eprintln!("termchat: hook: accept: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
            continue;
        };
        let hook = Arc::clone(&hook);
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |req| {
                let hook = Arc::clone(&hook);
                async move { Ok::<_, std::convert::Infallible>(handle(&hook, req).await) }
            });
            let conn = hyper::server::conn::http1::Builder::new()
                .keep_alive(false)
                .timer(TokioTimer::new())
                .header_read_timeout(READ_LIMIT)
                .serve_connection(TokioIo::new(stream), service);
            let limit = READ_LIMIT + Duration::from_secs(SEND_TIMEOUT + 5);
            let _ = tokio::time::timeout(limit, conn).await;
            drop(slot);
        });
    }
}

fn answer(status: StatusCode, text: &'static str) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from_static(text.as_bytes())));
    *r.status_mut() = status;
    r
}

async fn handle(hook: &Hook, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let path = req.uri().path().trim_start_matches('/').to_string();
    let status = respond(hook, &path, req).await;
    // The route and the outcome only: an alert's text stays out of logs.
    // Review 2026-11d, note 6: without the token, not even the path.
    match status.status() {
        StatusCode::UNAUTHORIZED => eprintln!("termchat: hook 401"),
        code => eprintln!(
            "termchat: hook /{} {}",
            path.chars().take(64).collect::<String>(),
            code.as_u16()
        ),
    }
    status
}

async fn respond(hook: &Hook, path: &str, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let presented = req
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.as_bytes().strip_prefix(b"Bearer "))
        .unwrap_or_default();
    if !bool::from(presented.ct_eq(hook.token.as_bytes())) {
        return answer(StatusCode::UNAUTHORIZED, "unauthorized\n");
    }
    if req.method() != Method::POST {
        return answer(StatusCode::METHOD_NOT_ALLOWED, "POST only\n");
    }
    let Some(route) = hook.routes.get(path) else {
        return answer(StatusCode::NOT_FOUND, "no such route\n");
    };
    let body = match tokio::time::timeout(
        READ_LIMIT,
        Limited::new(req.into_body(), MAX_BODY).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(e))
            if e.downcast_ref::<http_body_util::LengthLimitError>()
                .is_some() =>
        {
            return answer(StatusCode::PAYLOAD_TOO_LARGE, "over 1 MiB\n");
        }
        _ => return answer(StatusCode::BAD_REQUEST, "could not read the body\n"),
    };
    if let Target::Page { chain, escalate } = &route.target {
        return page(hook, path, route.format, chain, *escalate, &body).await;
    }
    let Some(alert) = alert::render(route.format, &body, &hook.labels) else {
        return answer(StatusCode::BAD_REQUEST, "not this route's format\n");
    };
    if hook.quiet.is_some_and(|q| q.holds(Quiet::now())) && !alert::critical(route.format, &body) {
        if let Ok(mut held) = hook.held.lock() {
            held.entry(path.to_string()).or_default().add(alert);
        }
        return answer(StatusCode::OK, "quiet hours: held for the summary\n");
    }
    // The same alert again soon: counted, said with the next one sent.
    let Some(text) = hook.unless_repeat(path, &alert, Instant::now()) else {
        return answer(StatusCode::OK, "repeat: counted, not sent\n");
    };
    if !hook.take_slot(path) {
        return answer(
            StatusCode::TOO_MANY_REQUESTS,
            "too many alerts: try again later\n",
        );
    }
    // Pages were handled above, before the body became one message.
    let Some(request) = send_request(&route.target, text) else {
        return answer(StatusCode::INTERNAL_SERVER_ERROR, "internal error\n");
    };
    match call(&hook.home, &request).await {
        Some(Ok(_)) => {
            hook.was_sent(path, alert, Instant::now());
            answer(StatusCode::OK, "sent\n")
        }
        // No daemon, the relay not reached in time, or the daemon lost: retry later.
        None | Some(Err(Fail(1 | 4 | 6, _))) => answer(
            StatusCode::SERVICE_UNAVAILABLE,
            "not sent: try again later\n",
        ),
        Some(Err(_)) => answer(
            StatusCode::INTERNAL_SERVER_ERROR,
            "not sent: check the route\n",
        ),
    }
}

/// The daemon request that sends `text` to a contact or group route.
fn send_request(target: &Target, text: String) -> Option<Value> {
    match target {
        Target::Contact(to) => Some(
            json!({ "cmd": "send", "to": to, "text": text, "timeout": SEND_TIMEOUT, "expires": null }),
        ),
        Target::Group(group) => serde_json::to_value(GroupRequest::GroupSend {
            group: group.clone(),
            text,
            timeout: SEND_TIMEOUT,
        })
        .ok(),
        Target::Page { .. } => None,
    }
}

/// Once quiet hours end, each route's held alerts go as one message; a
/// summary the daemon did not take waits for the next minute.
async fn summaries(hook: Arc<Hook>) {
    let Some(quiet) = hook.quiet else { return };
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        if quiet.holds(Quiet::now()) {
            continue;
        }
        let held = match hook.held.lock() {
            Ok(mut held) => std::mem::take(&mut *held),
            Err(_) => continue,
        };
        for (path, h) in held {
            let request = hook
                .routes
                .get(&path)
                .and_then(|r| send_request(&r.target, h.summary(quiet)));
            let Some(request) = request else { continue };
            if !matches!(call(&hook.home, &request).await, Some(Ok(_)))
                && let Ok(mut held) = hook.held.lock()
            {
                let back = held.entry(path).or_default();
                for (alert, n) in h.alerts {
                    for _ in 0..n {
                        back.add(alert.clone());
                    }
                }
                back.more = back.more.saturating_add(h.more);
            }
        }
    }
}

/// ADR-040: each firing incident opens or adds to a page, each resolved one
/// resolves it.
async fn page(
    hook: &Hook,
    path: &str,
    format: Format,
    chain: &[String],
    escalate: Option<u64>,
    body: &[u8],
) -> Response<Full<Bytes>> {
    let Some(incidents) = alert::incidents(format, body, &hook.labels, path) else {
        return answer(StatusCode::BAD_REQUEST, "not this route's format\n");
    };
    if !hook.take_slot(path) {
        return answer(
            StatusCode::TOO_MANY_REQUESTS,
            "too many alerts: try again later\n",
        );
    }
    let mut worst = StatusCode::OK;
    for incident in incidents {
        let request = match incident.firing {
            true => json!({
                "cmd": "page", "to": chain, "text": incident.text, "key": incident.key,
                "escalate": escalate, "wait": false,
            }),
            false => json!({ "cmd": "resolve", "key": incident.key }),
        };
        let status = match call(&hook.home, &request).await {
            // A resolution for a page this daemon never opened, or already closed.
            Some(Ok(_)) | Some(Err(Fail(3, _))) if !incident.firing => StatusCode::OK,
            Some(Ok(v)) if v["page"] == "opened" => StatusCode::OK,
            None | Some(Err(Fail(1 | 4 | 6, _))) => StatusCode::SERVICE_UNAVAILABLE,
            Some(Ok(v)) if matches!(v["page"].as_str(), Some("unavailable" | "send_failed")) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if status != StatusCode::OK && worst != StatusCode::SERVICE_UNAVAILABLE {
            worst = status;
        }
    }
    match worst {
        StatusCode::OK => answer(StatusCode::OK, "paged\n"),
        StatusCode::SERVICE_UNAVAILABLE => answer(worst, "not paged: try again later\n"),
        _ => answer(worst, "not paged: check the route\n"),
    }
}

impl Hook {
    /// `None`: `alert` went out on `route` less than `REPEAT_WINDOW` ago,
    /// and is counted. Otherwise the text to send, saying how many repeats
    /// were held back since the last time.
    fn unless_repeat(&self, route: &str, alert: &str, now: Instant) -> Option<String> {
        let Ok(mut seen) = self.repeats.lock() else {
            return Some(alert.to_string());
        };
        seen.retain(|_, (at, _)| now.duration_since(*at) < REPEAT_MEMORY);
        match seen.get_mut(&(route.to_string(), alert.to_string())) {
            Some((at, n)) if now.duration_since(*at) < REPEAT_WINDOW => {
                *n = n.saturating_add(1);
                None
            }
            Some((_, n)) if *n > 0 => Some(format!(
                "{alert}\n(the same alert came {n} more times after it was last sent)"
            )),
            _ => Some(alert.to_string()),
        }
    }

    /// `alert` reached the daemon: repeats count from now.
    fn was_sent(&self, route: &str, alert: String, now: Instant) {
        if let Ok(mut seen) = self.repeats.lock() {
            seen.insert((route.to_string(), alert), (now, 0));
        }
    }

    /// One of `PER_MINUTE` sends a minute for `route`.
    fn take_slot(&self, route: &str) -> bool {
        let Ok(mut sent) = self.sent.lock() else {
            return false;
        };
        let times = sent.entry(route.to_string()).or_default();
        let now = Instant::now();
        while times
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            times.pop_front();
        }
        if times.len() >= PER_MINUTE {
            return false;
        }
        times.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_parse_with_and_without_a_format() {
        let (name, r) = parse_route("ops=group:Ops").ok().unwrap();
        assert_eq!(name, "ops");
        assert!(matches!(r.target, Target::Group(Selector::Name(ref g)) if g == "Ops"));
        assert_eq!(r.format, Format::Auto);
        let (_, r) = parse_route("oncall=contact:alice:alertmanager")
            .ok()
            .unwrap();
        assert!(matches!(r.target, Target::Contact(ref c) if c == "alice"));
        assert_eq!(r.format, Format::Alertmanager);
        // A colon that is not a format stays part of the name.
        let (_, r) = parse_route("x=contact:a:b").ok().unwrap();
        assert!(matches!(r.target, Target::Contact(ref c) if c == "a:b"));
        for bad in [
            "ops",
            "Ops=group:x",
            "o=team:x",
            "o=contact:",
            "=contact:a",
            "o/p=contact:a",
        ] {
            assert!(parse_route(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn repeats_are_counted_then_said_once() {
        let hook = Hook {
            home: PathBuf::new(),
            token: Zeroizing::new(String::new()),
            routes: HashMap::new(),
            labels: Vec::new(),
            sent: Mutex::new(HashMap::new()),
            repeats: Mutex::new(HashMap::new()),
            quiet: None,
            held: Mutex::new(HashMap::new()),
        };
        let t0 = Instant::now();
        let later = |s: u64| t0 + Duration::from_secs(s);
        assert_eq!(
            hook.unless_repeat("ops", "disk full", t0).as_deref(),
            Some("disk full")
        );
        hook.was_sent("ops", "disk full".into(), t0);
        // Within the window: counted; another text or route still goes.
        for s in [1, 60, 599] {
            assert_eq!(hook.unless_repeat("ops", "disk full", later(s)), None);
        }
        assert!(hook.unless_repeat("ops", "disk ok", later(2)).is_some());
        assert!(hook.unless_repeat("web", "disk full", later(2)).is_some());
        // After it: sent once more, with the count, which then starts over.
        let next = hook.unless_repeat("ops", "disk full", later(600)).unwrap();
        assert!(
            next.ends_with("came 3 more times after it was last sent)"),
            "{next}"
        );
        hook.was_sent("ops", "disk full".into(), later(600));
        assert_eq!(
            hook.unless_repeat("ops", "disk full", later(1300))
                .as_deref(),
            Some("disk full")
        );
        // A send that failed is tried again on the next alert, not counted.
        assert!(hook.unless_repeat("ops", "cpu hot", later(1)).is_some());
        assert!(hook.unless_repeat("ops", "cpu hot", later(2)).is_some());
    }

    #[test]
    fn quiet_hours_parse_wrap_and_sum_up() {
        let night = Quiet::parse("22:00-08:30").unwrap();
        assert_eq!(night.to_string(), "22:00-08:30");
        assert!(night.holds(23 * 60) && night.holds(0) && night.holds(8 * 60 + 29));
        assert!(!night.holds(8 * 60 + 30) && !night.holds(12 * 60));
        let lunch = Quiet::parse("12:00-13:00").unwrap();
        assert!(lunch.holds(12 * 60) && !lunch.holds(13 * 60) && !lunch.holds(11 * 60));
        for bad in [
            "",
            "22:00",
            "24:00-01:00",
            "10:60-11:00",
            "09:00-09:00",
            "a-b",
        ] {
            assert_eq!(Quiet::parse(bad), None, "{bad}");
        }
        let mut h = Held::default();
        for a in ["disk full", "disk full", "cpu hot"] {
            h.add(a.into());
        }
        let text = h.summary(night);
        assert!(
            text.starts_with("QUIET HOURS 22:00-08:30 UTC: 3 alert(s) held"),
            "{text}"
        );
        assert!(text.contains("disk full\n(came 2 times)") && text.contains("cpu hot"));
        for n in 0..MAX_HELD + 5 {
            h.add(format!("alert {n}"));
        }
        assert!(
            h.summary(night).ends_with("and 97 more"),
            "{}",
            h.summary(night)
        );
    }

    #[test]
    fn a_route_takes_at_most_its_share_a_minute() {
        let hook = Hook {
            home: PathBuf::new(),
            token: Zeroizing::new(String::new()),
            routes: HashMap::new(),
            labels: Vec::new(),
            sent: Mutex::new(HashMap::new()),
            repeats: Mutex::new(HashMap::new()),
            quiet: None,
            held: Mutex::new(HashMap::new()),
        };
        for _ in 0..PER_MINUTE {
            assert!(hook.take_slot("ops"));
        }
        assert!(!hook.take_slot("ops"));
        assert!(hook.take_slot("other"));
    }
}
