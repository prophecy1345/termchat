//! Relay: forwards encrypted envelopes between authenticated clients.
//!
//! Sees only public keys and ciphertext. Logs neither IPs nor contents.
//! State lives in memory until M5.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use termchat_proto::{
    ClientFrame, Ed25519Key, Envelope, ErrorCode, MAX_FRAME_BYTES, ServerFrame, SignedKey, VERSION,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct Config {
    /// Host that clients sign in `Auth` (protocol.md, section 3), e.g. `relay.example.org`.
    pub host: String,
    /// Take the client IP from the last `X-Forwarded-For` entry. Only behind a proxy that sets it.
    pub trust_proxy: bool,
    /// Operator's report journal (ADR-006); `None` keeps no journal.
    pub reports: Option<std::path::PathBuf>,
    pub limits: Limits,
}

/// protocol.md, section 11.
#[derive(Debug, Clone)]
pub struct Limits {
    pub auth_timeout: Duration,
    pub ping_interval: Duration,
    pub idle_timeout: Duration,
    pub connections_per_ip: usize,
    pub registrations_per_ip: Rate,
    pub sends_per_key: Rate,
    pub new_recipients_per_key: Rate,
    pub claims_per_key: Rate,
    pub claims_per_target: Rate,
    pub stored_one_time_keys: usize,
    pub queue_envelopes: usize,
    pub queue_bytes: usize,
    pub reports_per_key: Rate,
    /// Distinct people who were really written to before a key is banned (ADR-006).
    pub report_threshold: usize,
}

/// Token bucket: `burst` at once, then `per_sec` on average.
#[derive(Debug, Clone, Copy)]
pub struct Rate {
    pub burst: f64,
    pub per_sec: f64,
}

impl Rate {
    pub const fn per(count: f64, secs: f64) -> Self {
        Self {
            burst: count,
            per_sec: count / secs,
        }
    }
}

const HOUR: f64 = 3600.0;
const DAY: f64 = 24.0 * HOUR;

impl Default for Limits {
    fn default() -> Self {
        Self {
            auth_timeout: Duration::from_secs(10),
            ping_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            connections_per_ip: 50,
            registrations_per_ip: Rate::per(20.0, HOUR),
            sends_per_key: Rate {
                burst: 30.0,
                per_sec: 0.5,
            },
            new_recipients_per_key: Rate::per(20.0, DAY),
            claims_per_key: Rate::per(30.0, HOUR),
            claims_per_target: Rate::per(100.0, DAY),
            stored_one_time_keys: 100,
            queue_envelopes: 1000,
            queue_bytes: 10 * 1024 * 1024,
            reports_per_key: Rate::per(10.0, DAY),
            report_threshold: 3,
        }
    }
}

pub async fn serve(listener: TcpListener, config: Config) -> std::io::Result<()> {
    let shared = Arc::new(Shared {
        config,
        relay: Mutex::new(Relay::default()),
    });
    tokio::spawn(cleanup(Arc::clone(&shared)));
    let app = Router::new().route("/", get(upgrade)).with_state(shared);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

struct Shared {
    config: Config,
    // one global lock; shard per key if a single relay gets busy.
    relay: Mutex<Relay>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Relay> {
        self.relay.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Default)]
struct Relay {
    online: HashMap<Ed25519Key, (u64, mpsc::Sender<ServerFrame>)>,
    // accounts are never forgotten while the process runs; M5 moves them to SQLite.
    accounts: HashMap<Ed25519Key, Account>,
    connections: HashMap<IpAddr, usize>,
    registrations: HashMap<IpAddr, Bucket>,
    next_id: u64,
}

struct Account {
    one_time: VecDeque<SignedKey>,
    fallback: Option<SignedKey>,
    fallback_used: bool,
    queue: VecDeque<(u64, Ed25519Key, Envelope)>,
    queue_bytes: usize,
    /// Everyone this key has written to: only new names count against the limit.
    recipients: HashSet<Ed25519Key>,
    sends: Bucket,
    new_recipients: Bucket,
    claims: Bucket,
    claimed: Bucket,
    reports: Bucket,
    /// People this key wrote to who reported it.
    reporters: HashSet<Ed25519Key>,
    banned: bool,
}

impl Account {
    fn new(l: &Limits, now: Instant) -> Self {
        Self {
            one_time: VecDeque::new(),
            fallback: None,
            fallback_used: false,
            queue: VecDeque::new(),
            queue_bytes: 0,
            recipients: HashSet::new(),
            sends: Bucket::full(l.sends_per_key, now),
            new_recipients: Bucket::full(l.new_recipients_per_key, now),
            claims: Bucket::full(l.claims_per_key, now),
            claimed: Bucket::full(l.claims_per_target, now),
            reports: Bucket::full(l.reports_per_key, now),
            reporters: HashSet::new(),
            banned: false,
        }
    }

    fn key_count(&self) -> ServerFrame {
        ServerFrame::KeyCount {
            one_time_left: u32::try_from(self.one_time.len()).unwrap_or(u32::MAX),
            fallback_used: self.fallback_used,
        }
    }
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full(rate: Rate, now: Instant) -> Self {
        Self {
            tokens: rate.burst,
            at: now,
        }
    }

    fn refill(&mut self, rate: Rate, now: Instant) {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + elapsed * rate.per_sec).min(rate.burst);
        self.at = now;
    }

    fn take(&mut self, rate: Rate, now: Instant) -> bool {
        self.refill(rate, now);
        let ok = self.tokens >= 1.0;
        if ok {
            self.tokens -= 1.0;
        }
        ok
    }
}

/// Forgets per-IP state that has fully recovered, so it does not grow forever.
async fn cleanup(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    loop {
        tick.tick().await;
        let now = Instant::now();
        let rate = shared.config.limits.registrations_per_ip;
        let mut relay = shared.lock();
        relay.registrations.retain(|_, b| {
            b.refill(rate, now);
            b.tokens < rate.burst
        });
    }
}

async fn upgrade(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(shared): State<Arc<Shared>>,
) -> Response {
    let ip = client_ip(&shared.config, addr, &headers);
    ws.max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| connection(shared, socket, ip))
}

fn client_ip(config: &Config, addr: SocketAddr, headers: &HeaderMap) -> IpAddr {
    if config.trust_proxy
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse().ok())
    {
        return ip;
    }
    addr.ip()
}

async fn connection(shared: Arc<Shared>, mut ws: WebSocket, ip: IpAddr) {
    let allowed = {
        let mut relay = shared.lock();
        let n = relay.connections.entry(ip).or_default();
        let ok = *n < shared.config.limits.connections_per_ip;
        if ok {
            *n += 1;
        }
        ok
    };
    if !allowed {
        let _ = send(&mut ws, &error(None, ErrorCode::RateLimited)).await;
        return;
    }

    let session = match tokio::time::timeout(
        shared.config.limits.auth_timeout,
        handshake(&shared, &mut ws, ip),
    )
    .await
    {
        Ok(Ok(session)) => Some(session),
        Ok(Err(code)) => {
            let _ = send(&mut ws, &error(None, code)).await;
            None
        }
        Err(_) => {
            let _ = send(&mut ws, &error(None, ErrorCode::ChallengeExpired)).await;
            None
        }
    };
    if let Some((key, id, rx)) = session {
        if let Err(code) = serve_client(&shared, &mut ws, key, rx).await {
            let _ = send(&mut ws, &error(None, code)).await;
        }
        let mut relay = shared.lock();
        if relay
            .online
            .get(&key)
            .is_some_and(|(current, _)| *current == id)
        {
            relay.online.remove(&key);
        }
    }

    let mut relay = shared.lock();
    if let Some(n) = relay.connections.get_mut(&ip) {
        *n -= 1;
        if *n == 0 {
            relay.connections.remove(&ip);
        }
    }
}

type Session = (Ed25519Key, u64, mpsc::Receiver<ServerFrame>);

async fn handshake(shared: &Shared, ws: &mut WebSocket, ip: IpAddr) -> Result<Session, ErrorCode> {
    let ClientFrame::Hello { version } = recv(ws).await? else {
        return Err(ErrorCode::NotAuthenticated);
    };
    if version != VERSION {
        return Err(ErrorCode::UnsupportedVersion);
    }
    let mut nonce = [0; 32];
    getrandom::fill(&mut nonce).map_err(|_| ErrorCode::Internal)?;
    send(ws, &ServerFrame::Challenge { nonce }).await?;

    let ClientFrame::Auth {
        identity,
        signature,
    } = recv(ws).await?
    else {
        return Err(ErrorCode::NotAuthenticated);
    };
    termchat_core::verify_auth(&identity, &shared.config.host, &nonce, &signature)
        .map_err(|_| ErrorCode::BadSignature)?;
    let key = identity.ed25519;

    let (tx, rx) = mpsc::channel(256);
    let (id, queued) = {
        let mut relay = shared.lock();
        let now = Instant::now();
        let limits = &shared.config.limits;
        if relay.accounts.get(&key).is_some_and(|a| a.banned) {
            return Err(ErrorCode::Banned);
        }
        if !relay.accounts.contains_key(&key) {
            let ok = relay
                .registrations
                .entry(ip)
                .or_insert_with(|| Bucket::full(limits.registrations_per_ip, now))
                .take(limits.registrations_per_ip, now);
            if !ok {
                return Err(ErrorCode::RateLimited);
            }
            relay.accounts.insert(key, Account::new(limits, now));
        }
        relay.next_id += 1;
        let id = relay.next_id;
        // A newer connection replaces the old one; the old loop ends when its sender drops.
        relay.online.insert(key, (id, tx));
        let account = relay.accounts.get(&key).ok_or(ErrorCode::Internal)?;
        let mut frames: Vec<ServerFrame> = account
            .queue
            .iter()
            .map(|(relay_id, sender, envelope)| ServerFrame::Deliver {
                relay_id: *relay_id,
                sender: *sender,
                envelope: envelope.clone(),
            })
            .collect();
        frames.push(account.key_count());
        (id, frames)
    };

    send(ws, &ServerFrame::AuthOk).await?;
    for frame in &queued {
        send(ws, frame).await?;
    }
    Ok((key, id, rx))
}

async fn serve_client(
    shared: &Shared,
    ws: &mut WebSocket,
    key: Ed25519Key,
    mut rx: mpsc::Receiver<ServerFrame>,
) -> Result<(), ErrorCode> {
    let limits = &shared.config.limits;
    let mut ping = tokio::time::interval(limits.ping_interval);
    let mut last_seen = Instant::now();
    loop {
        tokio::select! {
            msg = ws.recv() => {
                last_seen = Instant::now();
                match msg {
                    Some(Ok(Message::Binary(bytes))) => {
                        let frame = ClientFrame::decode(&bytes).map_err(ErrorCode::from)?;
                        for reply in handle(shared, key, frame)? {
                            send(ws, &reply).await?;
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Text(_))) => return Err(ErrorCode::Malformed),
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Ok(()),
                }
            }
            frame = rx.recv() => match frame {
                Some(frame) => send(ws, &frame).await?,
                None => return Ok(()),
            },
            _ = ping.tick() => {
                if last_seen.elapsed() > limits.idle_timeout {
                    return Ok(());
                }
                ws.send(Message::Ping(Default::default())).await.map_err(|_| ErrorCode::Internal)?;
            }
        }
    }
}

/// Handles one frame from an authenticated client. `Err` closes the connection.
fn handle(
    shared: &Shared,
    me: Ed25519Key,
    frame: ClientFrame,
) -> Result<Vec<ServerFrame>, ErrorCode> {
    let limits = &shared.config.limits;
    let now = Instant::now();
    let mut relay = shared.lock();
    let relay = &mut *relay;
    let account = relay.accounts.get_mut(&me).ok_or(ErrorCode::Internal)?;

    Ok(match frame {
        ClientFrame::Hello { .. } | ClientFrame::Auth { .. } => return Err(ErrorCode::Malformed),

        ClientFrame::PublishKeys { one_time, fallback } => {
            account.one_time.extend(one_time);
            while account.one_time.len() > limits.stored_one_time_keys {
                account.one_time.pop_front();
            }
            if fallback.is_some() {
                account.fallback = fallback;
                account.fallback_used = false;
            }
            vec![account.key_count()]
        }

        ClientFrame::ClaimKey { target } => {
            if !account.claims.take(limits.claims_per_key, now) {
                return Ok(vec![error(None, ErrorCode::RateLimited)]);
            }
            let Some(t) = relay.accounts.get_mut(&target) else {
                return Ok(vec![error(None, ErrorCode::UnknownRecipient)]);
            };
            // Past the per-target limit only the fallback key is handed out.
            let one_time = match t.claimed.take(limits.claims_per_target, now) {
                true => t.one_time.pop_front(),
                false => None,
            };
            let key = match one_time {
                Some(k) => k,
                None => match &t.fallback {
                    Some(k) => {
                        t.fallback_used = true;
                        k.clone()
                    }
                    None => return Ok(vec![error(None, ErrorCode::NoKeys)]),
                },
            };
            if let Some((_, tx)) = relay.online.get(&target) {
                let _ = tx.try_send(t.key_count());
            }
            vec![ServerFrame::ClaimedKey { target, key }]
        }

        ClientFrame::Send {
            client_id,
            envelope,
        } => {
            let to = envelope.recipient;
            if !account.sends.take(limits.sends_per_key, now) {
                return Ok(vec![error(Some(client_id), ErrorCode::RateLimited)]);
            }
            if !relay.accounts.contains_key(&to) {
                return Ok(vec![error(Some(client_id), ErrorCode::UnknownRecipient)]);
            }
            let account = relay.accounts.get_mut(&me).ok_or(ErrorCode::Internal)?;
            if !account.recipients.contains(&to) {
                if !account
                    .new_recipients
                    .take(limits.new_recipients_per_key, now)
                {
                    return Ok(vec![error(Some(client_id), ErrorCode::RateLimited)]);
                }
                account.recipients.insert(to);
            }
            let r = relay.accounts.get_mut(&to).ok_or(ErrorCode::Internal)?;
            let size = envelope.body.len();
            if r.queue.len() >= limits.queue_envelopes || r.queue_bytes + size > limits.queue_bytes
            {
                return Ok(vec![error(Some(client_id), ErrorCode::QueueFull)]);
            }
            relay.next_id += 1;
            let relay_id = relay.next_id;
            if let Some((_, tx)) = relay.online.get(&to) {
                // Full channel: the envelope stays queued and goes out on reconnect.
                let _ = tx.try_send(ServerFrame::Deliver {
                    relay_id,
                    sender: me,
                    envelope: envelope.clone(),
                });
            }
            r.queue.push_back((relay_id, me, envelope));
            r.queue_bytes += size;
            vec![ServerFrame::Accepted { client_id }]
        }

        ClientFrame::Report { sender, text } => {
            // Same answer on every path: a report must not reveal whether it counted.
            let reported = vec![ServerFrame::Reported];
            if !account.reports.take(limits.reports_per_key, now) {
                return Ok(reported);
            }
            // Only someone the sender actually wrote to can report it.
            let Some(s) = relay.accounts.get_mut(&sender) else {
                return Ok(reported);
            };
            if !s.recipients.contains(&me) {
                return Ok(reported);
            }
            s.reporters.insert(me);
            if s.reporters.len() >= limits.report_threshold && !s.banned {
                s.banned = true;
                // Dropping its sender ends the banned key's connection loop.
                relay.online.remove(&sender);
            }
            if let Some(path) = &shared.config.reports
                && let Err(e) = journal(path, &sender, &text, s.banned)
            {
                tracing::warn!("cannot write report journal: {e}");
            }
            reported
        }

        ClientFrame::Ack { relay_id } => {
            if let Some(i) = account.queue.iter().position(|(id, _, _)| *id == relay_id)
                && let Some((_, _, env)) = account.queue.remove(i)
            {
                account.queue_bytes -= env.body.len();
            }
            vec![]
        }
    })
}

/// One JSON line per report: time, reported key, whether it is now banned, the
/// disclosed text. No reporter, no IP (rule 14). The operator deletes entries once reviewed.
// file IO under the global lock; reports are rare and rate-limited.
fn journal(
    path: &std::path::Path,
    sender: &Ed25519Key,
    text: &str,
    banned: bool,
) -> std::io::Result<()> {
    use std::io::Write;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let sender: String = sender.iter().map(|b| format!("{b:02x}")).collect();
    let line = serde_json::json!({ "at": at, "sender": sender, "banned": banned, "text": text });
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    writeln!(opts.open(path)?, "{line}")
}

fn error(client_id: Option<u64>, code: ErrorCode) -> ServerFrame {
    ServerFrame::Error { client_id, code }
}

async fn send(ws: &mut WebSocket, frame: &ServerFrame) -> Result<(), ErrorCode> {
    ws.send(Message::Binary(frame.encode().into()))
        .await
        .map_err(|_| ErrorCode::Internal)
}

async fn recv(ws: &mut WebSocket) -> Result<ClientFrame, ErrorCode> {
    loop {
        match ws.recv().await {
            Some(Ok(Message::Binary(bytes))) => {
                return ClientFrame::decode(&bytes).map_err(ErrorCode::from);
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            _ => return Err(ErrorCode::Malformed),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn journal_holds_only_sender_time_and_text() {
        let path = std::env::temp_dir().join(format!("termchat-reports-{}", std::process::id()));
        journal(&path, &[0xab; 32], "line \"one\"\nline two", true).unwrap();
        let line = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["sender"], "ab".repeat(32));
        assert_eq!(v["text"], "line \"one\"\nline two");
        assert_eq!(v["banned"], true);
        assert_eq!(v.as_object().unwrap().len(), 4, "no reporter, no IP");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(&path).unwrap();
    }
}
