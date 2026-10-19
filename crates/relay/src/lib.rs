//! Relay: forwards encrypted envelopes between authenticated clients.
//!
//! Sees only public keys and ciphertext. Logs neither IPs nor contents.
//! Accounts, prekeys, queue and bans live in SQLite (ADR-005); rate limits,
//! online connections and who wrote to whom stay in memory.

pub mod db;
mod push;

use std::collections::HashMap;
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
    ClientFrame, Ed25519Key, ErrorCode, MAX_FRAME_BYTES, ServerFrame, VERSION, VERSION_HEADER,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct Config {
    /// Hosts clients may sign in `Auth` (protocol.md, section 3): the public name
    /// and, behind Tor, the onion address (ADR-004).
    pub hosts: Vec<String>,
    /// Take the client IP from the last `X-Forwarded-For` entry. Only behind a proxy that sets it.
    pub trust_proxy: bool,
    /// SQLite file (ADR-005); `None` keeps everything in memory (tests).
    pub db: Option<std::path::PathBuf>,
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
    /// A client that stops reading must not pin a task and its buffers forever.
    pub send_timeout: Duration,
    pub connections_per_ip: usize,
    /// Every frame after `Auth`, whatever it is. `Ack` has no limit of its own,
    /// yet each one is a database write; the burst must cover a whole queue
    /// (`queue_envelopes`) being acknowledged after a reconnect.
    pub frames_per_key: Rate,
    pub registrations_per_ip: Rate,
    /// All onion connections arrive from 127.0.0.1, so they share one budget (ADR-004).
    pub onion_connections: usize,
    pub onion_registrations: Rate,
    pub sends_per_key: Rate,
    pub new_recipients_per_key: Rate,
    /// Each `PublishKeys` costs up to 100 inserts; a client needs a handful per day.
    pub publishes_per_key: Rate,
    pub claims_per_key: Rate,
    pub claims_per_target: Rate,
    pub stored_one_time_keys: usize,
    pub queue_envelopes: usize,
    /// ADR-007: what one sender may park with a recipient who never wrote back.
    pub stranger_queue_envelopes: usize,
    pub stranger_queue_bytes: u64,
    pub queue_bytes: usize,
    /// ADR-005: undelivered envelopes and idle accounts are deleted after these.
    pub queue_ttl: Duration,
    pub account_ttl: Duration,
    /// All queues together, bytes.
    pub max_queue_bytes: u64,
    pub reports_per_key: Rate,
    /// Distinct people who were really written to before a key is banned (ADR-006).
    pub report_threshold: usize,
    /// ADR-007: a report counts only after this many envelopes to the reporter,
    /// and reporters younger than `young_reporter` count half.
    pub report_min_envelopes: u32,
    pub young_reporter: Duration,
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
            send_timeout: Duration::from_secs(30),
            connections_per_ip: 50,
            frames_per_key: Rate {
                burst: 2000.0,
                per_sec: 20.0,
            },
            registrations_per_ip: Rate::per(20.0, HOUR),
            onion_connections: 2000,
            onion_registrations: Rate::per(200.0, HOUR),
            sends_per_key: Rate {
                burst: 30.0,
                per_sec: 0.5,
            },
            new_recipients_per_key: Rate::per(20.0, DAY),
            publishes_per_key: Rate::per(30.0, HOUR),
            claims_per_key: Rate::per(30.0, HOUR),
            claims_per_target: Rate::per(100.0, DAY),
            stored_one_time_keys: 100,
            queue_envelopes: 1000,
            stranger_queue_envelopes: 20,
            stranger_queue_bytes: 256 * 1024,
            queue_bytes: 10 * 1024 * 1024,
            queue_ttl: Duration::from_secs(termchat_proto::QUEUE_TTL_DAYS * 86_400),
            account_ttl: Duration::from_secs(90 * 86_400),
            max_queue_bytes: 10 << 30,
            reports_per_key: Rate::per(10.0, DAY),
            report_threshold: 3,
            report_min_envelopes: 3,
            young_reporter: Duration::from_secs(7 * 86_400),
        }
    }
}

/// Where a connection came from: per-IP limits for direct ones, one shared
/// budget for everything that came through the local Tor daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Origin {
    Ip(IpAddr),
    Onion,
}

impl Limits {
    fn connections(&self, o: Origin) -> usize {
        match o {
            Origin::Ip(_) => self.connections_per_ip,
            Origin::Onion => self.onion_connections,
        }
    }

    fn registrations(&self, o: Origin) -> Rate {
        match o {
            Origin::Ip(_) => self.registrations_per_ip,
            Origin::Onion => self.onion_registrations,
        }
    }
}

/// `onion`: a second listener that only the local Tor daemon forwards to (ADR-004).
pub async fn serve(
    listener: TcpListener,
    onion: Option<TcpListener>,
    config: Config,
) -> std::io::Result<()> {
    let db = db::Db::open(config.db.as_deref()).map_err(std::io::Error::other)?;
    let relay = Relay {
        db,
        online: HashMap::new(),
        mem: HashMap::new(),
        connections: HashMap::new(),
        registrations: HashMap::new(),
        next_conn: 0,
    };
    let shared = Arc::new(Shared {
        config,
        relay: Mutex::new(relay),
    });
    tokio::spawn(cleanup(Arc::clone(&shared)));
    let run = |listener: TcpListener, onion: bool| {
        let app = Router::new()
            .route("/", get(upgrade))
            .with_state((Arc::clone(&shared), onion));
        async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
        }
    };
    match onion {
        Some(o) => tokio::try_join!(run(listener, false), run(o, true)).map(|_| ()),
        None => run(listener, false).await,
    }
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

struct Relay {
    db: db::Db,
    online: HashMap<Ed25519Key, (u64, mpsc::Sender<ServerFrame>)>,
    // grows with every key seen since start, bounded by the registration limit.
    mem: HashMap<Ed25519Key, Account>,
    connections: HashMap<Origin, usize>,
    registrations: HashMap<Origin, Bucket>,
    next_conn: u64,
}

/// Per-key state that never touches the disk (ADR-005): limits and who wrote to whom.
struct Account {
    /// Everyone this key has written to: only new names count against the limit,
    /// and only they can report it (ADR-006).
    recipients: HashMap<Ed25519Key, u32>,
    /// People this key wrote to who reported it.
    reporters: HashMap<Ed25519Key, f64>,
    frames: Bucket,
    sends: Bucket,
    new_recipients: Bucket,
    publishes: Bucket,
    claims: Bucket,
    claimed: Bucket,
    reports: Bucket,
    /// The last UnifiedPush wake-up sent for this key (ADR-012).
    pushed: Option<Instant>,
}

/// At most one wake-up per key in this time (ADR-012).
const PUSH_GAP: Duration = Duration::from_secs(30);

impl Account {
    fn new(l: &Limits, now: Instant) -> Self {
        Self {
            recipients: HashMap::new(),
            reporters: HashMap::new(),
            frames: Bucket::full(l.frames_per_key, now),
            sends: Bucket::full(l.sends_per_key, now),
            new_recipients: Bucket::full(l.new_recipients_per_key, now),
            publishes: Bucket::full(l.publishes_per_key, now),
            claims: Bucket::full(l.claims_per_key, now),
            claimed: Bucket::full(l.claims_per_target, now),
            reports: Bucket::full(l.reports_per_key, now),
            pushed: None,
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Storage errors never reach the client as details; they are logged without content.
fn db<T>(r: db::Result<T>) -> Result<T, ErrorCode> {
    r.map_err(|e| {
        tracing::warn!("storage: {e}");
        ErrorCode::Internal
    })
}

fn key_count(store: &db::Db, key: &Ed25519Key) -> Result<ServerFrame, ErrorCode> {
    let (one_time_left, fallback_used) = db(store.key_count(key))?;
    Ok(ServerFrame::KeyCount {
        one_time_left,
        fallback_used,
    })
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

/// Every 10 minutes: expire old envelopes and idle accounts (ADR-005) and forget
/// per-IP state that has fully recovered, so it does not grow forever.
async fn cleanup(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    loop {
        tick.tick().await;
        let limits = &shared.config.limits;
        let (now, unix) = (Instant::now(), unix_now());
        let mut relay = shared.lock();
        relay.registrations.retain(|origin, b| {
            let rate = limits.registrations(*origin);
            b.refill(rate, now);
            b.tokens < rate.burst
        });
        // Long-lived connections count as seen.
        let online: Vec<Ed25519Key> = relay.online.keys().copied().collect();
        for key in &online {
            let _ = db(relay.db.register(key, unix));
        }
        let ttl = |d: Duration| d.as_secs() as i64;
        let _ = db(relay
            .db
            .expire(unix, ttl(limits.queue_ttl), ttl(limits.account_ttl)));
    }
}

async fn upgrade(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State((shared, onion)): State<(Arc<Shared>, bool)>,
) -> Response {
    let origin = match onion {
        true => Origin::Onion,
        false => Origin::Ip(client_ip(&shared.config, addr, &headers)),
    };
    let mut response = ws
        .max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| connection(shared, socket, origin));
    // ADR-010: clients tell their user when a newer version is out.
    response.headers_mut().insert(
        VERSION_HEADER,
        axum::http::HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    );
    response
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

async fn connection(shared: Arc<Shared>, mut ws: WebSocket, ip: Origin) {
    let timeout = shared.config.limits.send_timeout;
    let allowed = {
        let mut relay = shared.lock();
        let n = relay.connections.entry(ip).or_default();
        let ok = *n < shared.config.limits.connections(ip);
        if ok {
            *n += 1;
        }
        ok
    };
    if !allowed {
        let _ = send(&mut ws, &error(None, ErrorCode::RateLimited), timeout).await;
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
            let _ = send(&mut ws, &error(None, code), timeout).await;
            None
        }
        Err(_) => {
            let _ = send(&mut ws, &error(None, ErrorCode::ChallengeExpired), timeout).await;
            None
        }
    };
    if let Some((key, id, rx, queued)) = session {
        if let Err(code) = serve_client(&shared, &mut ws, key, rx, queued).await {
            let _ = send(&mut ws, &error(None, code), timeout).await;
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

/// The key, the connection id, live frames, and the queue to deliver first.
type Session = (
    Ed25519Key,
    u64,
    mpsc::Receiver<ServerFrame>,
    Vec<ServerFrame>,
);

async fn handshake(shared: &Shared, ws: &mut WebSocket, ip: Origin) -> Result<Session, ErrorCode> {
    let timeout = shared.config.limits.send_timeout;
    let ClientFrame::Hello { version } = recv(ws).await? else {
        return Err(ErrorCode::NotAuthenticated);
    };
    if version != VERSION {
        return Err(ErrorCode::UnsupportedVersion);
    }
    let mut nonce = [0; 32];
    getrandom::fill(&mut nonce).map_err(|_| ErrorCode::Internal)?;
    send(ws, &ServerFrame::Challenge { nonce }, timeout).await?;

    let ClientFrame::Auth {
        identity,
        signature,
    } = recv(ws).await?
    else {
        return Err(ErrorCode::NotAuthenticated);
    };
    let signed_for =
        |host: &String| termchat_core::verify_auth(&identity, host, &nonce, &signature).is_ok();
    if !shared.config.hosts.iter().any(signed_for) {
        return Err(ErrorCode::BadSignature);
    }
    let key = identity.ed25519;

    let (tx, rx) = mpsc::channel(256);
    let (id, queued) = {
        let mut relay = shared.lock();
        let now = Instant::now();
        let limits = &shared.config.limits;
        match db(relay.db.banned(&key))? {
            Some(true) => return Err(ErrorCode::Banned),
            Some(false) => {}
            None => {
                let ok = relay
                    .registrations
                    .entry(ip)
                    .or_insert_with(|| Bucket::full(limits.registrations(ip), now))
                    .take(limits.registrations(ip), now);
                if !ok {
                    return Err(ErrorCode::RateLimited);
                }
            }
        }
        db(relay.db.register(&key, unix_now()))?;
        relay.next_conn += 1;
        let id = relay.next_conn;
        // A newer connection replaces the old one; the old loop ends when its sender drops.
        relay.online.insert(key, (id, tx));
        let mut frames: Vec<ServerFrame> = db(relay.db.queued(&key))?
            .into_iter()
            .map(|q| ServerFrame::Deliver {
                relay_id: q.id,
                sender: q.sender,
                envelope: q.envelope,
            })
            .collect();
        frames.push(key_count(&relay.db, &key)?);
        (id, frames)
    };

    send(ws, &ServerFrame::AuthOk, timeout).await?;
    Ok((key, id, rx, queued))
}

async fn serve_client(
    shared: &Shared,
    ws: &mut WebSocket,
    key: Ed25519Key,
    mut rx: mpsc::Receiver<ServerFrame>,
    queued: Vec<ServerFrame>,
) -> Result<(), ErrorCode> {
    let limits = &shared.config.limits;
    // Outside the auth timeout: a full queue over Tor takes longer than that,
    // and cutting it off there would lock a slow client out for good.
    for frame in &queued {
        send(ws, frame, limits.send_timeout).await?;
    }
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
                            send(ws, &reply, limits.send_timeout).await?;
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Text(_))) => return Err(ErrorCode::Malformed),
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Ok(()),
                }
            }
            frame = rx.recv() => match frame {
                Some(frame) => send(ws, &frame, limits.send_timeout).await?,
                None => return Ok(()),
            },
            _ = ping.tick() => {
                if last_seen.elapsed() > limits.idle_timeout {
                    return Ok(());
                }
                write(ws, Message::Ping(Default::default()), limits.send_timeout).await?;
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
    // Has the recipient ever written to us? Strangers get a small share of its queue (ADR-007).
    let mutual = match &frame {
        ClientFrame::Send { envelope, .. } => relay
            .mem
            .get(&envelope.recipient)
            .is_some_and(|r| r.recipients.contains_key(&me)),
        _ => false,
    };
    // Whom the recipient writes to: their senders are not strangers (ADR-007).
    let known: Vec<Ed25519Key> = match &frame {
        ClientFrame::Send { envelope, .. } if !mutual => relay
            .mem
            .get(&envelope.recipient)
            .map(|r| r.recipients.keys().copied().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let account = relay
        .mem
        .entry(me)
        .or_insert_with(|| Account::new(limits, now));
    // Flooding frames closes the connection; the bucket is still empty after a reconnect.
    if !account.frames.take(limits.frames_per_key, now) {
        return Err(ErrorCode::RateLimited);
    }

    Ok(match frame {
        ClientFrame::Hello { .. } | ClientFrame::Auth { .. } => return Err(ErrorCode::Malformed),

        ClientFrame::PublishKeys { one_time, fallback } => {
            if !account.publishes.take(limits.publishes_per_key, now) {
                return Ok(vec![error(None, ErrorCode::RateLimited)]);
            }
            db(relay.db.publish(
                &me,
                &one_time,
                fallback.as_ref(),
                limits.stored_one_time_keys,
            ))?;
            vec![key_count(&relay.db, &me)?]
        }

        ClientFrame::ClaimKey { target } => {
            if !account.claims.take(limits.claims_per_key, now) {
                return Ok(vec![error(None, ErrorCode::RateLimited)]);
            }
            if db(relay.db.banned(&target))?.is_none() {
                return Ok(vec![error(None, ErrorCode::UnknownRecipient)]);
            }
            // Past the per-target limit only the fallback key is handed out.
            let t = relay
                .mem
                .entry(target)
                .or_insert_with(|| Account::new(limits, now));
            let one_time_allowed = t.claimed.take(limits.claims_per_target, now);
            let Some(key) = db(relay.db.claim(&target, one_time_allowed))? else {
                return Ok(vec![error(None, ErrorCode::NoKeys)]);
            };
            if let Some((_, tx)) = relay.online.get(&target) {
                let _ = tx.try_send(key_count(&relay.db, &target)?);
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
            if db(relay.db.banned(&to))?.is_none() {
                return Ok(vec![error(Some(client_id), ErrorCode::UnknownRecipient)]);
            }
            if !account.recipients.contains_key(&to)
                && !account
                    .new_recipients
                    .take(limits.new_recipients_per_key, now)
            {
                return Ok(vec![error(Some(client_id), ErrorCode::RateLimited)]);
            }
            let size = envelope.body.len() as u64;
            let (count, bytes) = db(relay.db.queue_stats(&to, None))?;
            let stranger_full = !mutual && {
                let (n, b) = db(relay.db.queue_stats(&to, Some(&me)))?;
                // All strangers together get a quarter of the queue, so many
                // fresh keys cannot fill it and lock out real contacts (ADR-007).
                let (sn, sb) = db(relay.db.queue_by_sender(&to))?
                    .into_iter()
                    .filter(|(s, ..)| !known.contains(s))
                    .fold((0, 0), |(n, b), (_, c, z)| (n + c, b + z));
                n >= limits.stranger_queue_envelopes
                    || b + size > limits.stranger_queue_bytes
                    || sn >= limits.queue_envelopes / 4
                    || sb + size > limits.queue_bytes as u64 / 4
            };
            if stranger_full
                || count >= limits.queue_envelopes
                || bytes + size > limits.queue_bytes as u64
                || relay.db.total_queue_bytes() + size > limits.max_queue_bytes
            {
                return Ok(vec![error(Some(client_id), ErrorCode::QueueFull)]);
            }
            let relay_id = db(relay.db.enqueue(&to, &me, &envelope, unix_now()))?;
            *account.recipients.entry(to).or_default() += 1;
            let deliver = ServerFrame::Deliver {
                relay_id,
                sender: me,
                envelope,
            };
            // A full channel means the client lags: drop its connection so it
            // reconnects and gets the whole queue (ADR-007).
            match relay.online.get(&to) {
                Some((_, tx)) => {
                    if tx.try_send(deliver).is_err() {
                        relay.online.remove(&to);
                    }
                }
                // Not here: wake it, if it asked for that (ADR-012).
                None => {
                    let t = relay
                        .mem
                        .entry(to)
                        .or_insert_with(|| Account::new(limits, now));
                    if t.pushed.is_none_or(|at| now.duration_since(at) >= PUSH_GAP)
                        && let Some(url) = db(relay.db.push(&to))?
                    {
                        t.pushed = Some(now);
                        tokio::spawn(push::wake(url));
                    }
                }
            }
            vec![ServerFrame::Accepted { client_id }]
        }

        ClientFrame::Report { sender, text } => {
            // Same answer on every path: a report must not reveal whether it counted.
            let reported = vec![ServerFrame::Reported];
            if !account.reports.take(limits.reports_per_key, now) {
                return Ok(reported);
            }
            // Only someone the sender actually wrote to (since the relay started) can report it.
            let Some(s) = relay.mem.get_mut(&sender) else {
                return Ok(reported);
            };
            // One ContactRequest is not enough: adding three public invites must
            // not let their owners ban you (ADR-007).
            if s.recipients.get(&me).copied().unwrap_or(0) < limits.report_min_envelopes {
                return Ok(reported);
            }
            let young = db(relay.db.created_at(&me))?
                .is_none_or(|t| unix_now() - t < limits.young_reporter.as_secs() as i64);
            s.reporters.insert(me, if young { 0.5 } else { 1.0 });
            let banned = s.reporters.values().sum::<f64>() >= limits.report_threshold as f64;
            if banned {
                db(relay.db.ban(&sender))?;
                // Dropping its sender ends the banned key's connection loop.
                relay.online.remove(&sender);
            }
            if let Some(path) = &shared.config.reports
                && let Err(e) = journal(path, &sender, &text, banned)
            {
                tracing::warn!("cannot write report journal: {e}");
            }
            reported
        }

        ClientFrame::Ack { relay_id } => {
            db(relay.db.ack(&me, relay_id))?;
            vec![]
        }

        // ADR-012. Only the shape is checked here; the address when it is used.
        ClientFrame::SetPush { endpoint } => {
            if endpoint
                .as_deref()
                .is_some_and(|u| push::parse(u).is_none())
            {
                return Ok(vec![error(None, ErrorCode::Malformed)]);
            }
            db(relay.db.set_push(&me, endpoint.as_deref()))?;
            vec![ServerFrame::PushSet]
        }

        // ADR-009. Dropping the sender ends the loop once the reply is out.
        ClientFrame::Unregister => {
            db(relay.db.unregister(&me))?;
            relay.mem.remove(&me);
            relay.online.remove(&me);
            vec![ServerFrame::Unregistered]
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

async fn send(ws: &mut WebSocket, frame: &ServerFrame, timeout: Duration) -> Result<(), ErrorCode> {
    write(ws, Message::Binary(frame.encode().into()), timeout).await
}

/// A peer that does not read for `timeout` is gone as far as we are concerned.
async fn write(ws: &mut WebSocket, msg: Message, timeout: Duration) -> Result<(), ErrorCode> {
    tokio::time::timeout(timeout, ws.send(msg))
        .await
        .map_err(|_| ErrorCode::Internal)?
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
