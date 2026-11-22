//! ADR-039: `termchat hook`, a webhook receiver for monitoring alerts.
//! It holds no password and no keys: every message goes through the daemon.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
                [--listen 127.0.0.1:9099] [--labels LABEL,...]
  termchat hook --new-token PATH
FORMAT: auto (default), alertmanager, kuma or text. Needs a running termchat daemon.";

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
}

/// `termchat hook ...`, `args` without the command name.
pub(super) async fn run(args: &[String]) -> Result<(), Fail> {
    let mut listen: SocketAddr = ([127, 0, 0, 1], 9099).into();
    let mut token_file = None;
    let mut new_token = None;
    let mut routes = HashMap::new();
    let mut labels = Vec::new();
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
    });
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
    eprintln!(
        "termchat: hook /{} {}",
        path.chars().take(64).collect::<String>(),
        status.status().as_u16()
    );
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
    let Some(text) = alert::render(route.format, &body, &hook.labels) else {
        return answer(StatusCode::BAD_REQUEST, "not this route's format\n");
    };
    if !hook.take_slot(path) {
        return answer(
            StatusCode::TOO_MANY_REQUESTS,
            "too many alerts: try again later\n",
        );
    }
    let request = match &route.target {
        Target::Contact(to) => {
            json!({ "cmd": "send", "to": to, "text": text, "timeout": SEND_TIMEOUT, "expires": null })
        }
        Target::Group(group) => {
            let send = GroupRequest::GroupSend {
                group: group.clone(),
                text,
                timeout: SEND_TIMEOUT,
            };
            match serde_json::to_value(send) {
                Ok(v) => v,
                Err(_) => return answer(StatusCode::INTERNAL_SERVER_ERROR, "internal error\n"),
            }
        }
        // Handled above, before the body became one message.
        Target::Page { .. } => {
            return answer(StatusCode::INTERNAL_SERVER_ERROR, "internal error\n");
        }
    };
    match call(&hook.home, &request).await {
        Some(Ok(_)) => answer(StatusCode::OK, "sent\n"),
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
    fn a_route_takes_at_most_its_share_a_minute() {
        let hook = Hook {
            home: PathBuf::new(),
            token: Zeroizing::new(String::new()),
            routes: HashMap::new(),
            labels: Vec::new(),
            sent: Mutex::new(HashMap::new()),
        };
        for _ in 0..PER_MINUTE {
            assert!(hook.take_slot("ops"));
        }
        assert!(!hook.take_slot("ops"));
        assert!(hook.take_slot("other"));
    }
}
