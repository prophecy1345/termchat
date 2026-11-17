//! Relay: forwards encrypted envelopes between anonymous queues (ADR-023).
//!
//! Knows no people: a queue is two random IDs and a token hash. Logs neither
//! IPs nor contents. Queues, prekeys and envelopes live in SQLite (ADR-005);
//! rate limits and online connections stay in memory.

pub mod db;
mod push;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use termchat_proto::{
    ClientFrame, Envelope, ErrorCode, MAX_FRAME_BYTES, MAX_GROUP_MEMBERS, QueueId, QueueToken,
    ServerFrame, VERSION, VERSION_HEADER,
};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc};

#[derive(Debug, Clone)]
pub struct Config {
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
    /// Time for the first frame, `Hello`.
    pub hello_timeout: Duration,
    pub ping_interval: Duration,
    pub idle_timeout: Duration,
    /// A client that stops reading must not pin a task and its buffers forever.
    pub send_timeout: Duration,
    pub connections_per_ip: usize,
    /// Every frame on a connection, whatever it is. `Ack` has no limit of its
    /// own, yet each one is a database write; the burst must cover a whole
    /// queue (`queue_envelopes`) being acknowledged after a reconnect.
    pub frames_per_connection: Rate,
    pub queues_per_ip: Rate,
    /// All onion connections arrive from 127.0.0.1, so they share one budget (ADR-004).
    pub onion_connections: usize,
    pub onion_queues: Rate,
    /// Writes into one queue, whoever writes: the relay does not know senders.
    pub sends_per_queue: Rate,
    /// Each `PublishKeys` costs up to 100 inserts; a client needs a handful per day.
    pub publishes_per_queue: Rate,
    pub claims_per_connection: Rate,
    /// Past this only the fallback key is handed out.
    pub claims_per_queue: Rate,
    pub stored_one_time_keys: usize,
    pub queue_envelopes: usize,
    pub queue_bytes: usize,
    /// ADR-005: undelivered envelopes and unused queues are deleted after these.
    pub queue_ttl: Duration,
    pub idle_queue_ttl: Duration,
    /// All queues together, bytes.
    pub max_queue_bytes: u64,
    pub reports_per_connection: Rate,
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
            hello_timeout: Duration::from_secs(10),
            ping_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(3600),
            send_timeout: Duration::from_secs(30),
            connections_per_ip: 50,
            frames_per_connection: Rate {
                burst: 2000.0,
                per_sec: 20.0,
            },
            queues_per_ip: Rate::per(20.0, HOUR),
            onion_connections: 2000,
            onion_queues: Rate::per(200.0, HOUR),
            sends_per_queue: Rate {
                burst: 30.0,
                per_sec: 0.5,
            },
            publishes_per_queue: Rate::per(30.0, HOUR),
            claims_per_connection: Rate::per(30.0, HOUR),
            claims_per_queue: Rate::per(100.0, DAY),
            stored_one_time_keys: 100,
            queue_envelopes: 1000,
            queue_bytes: 10 * 1024 * 1024,
            queue_ttl: Duration::from_secs(termchat_proto::QUEUE_TTL_DAYS * 86_400),
            idle_queue_ttl: Duration::from_secs(90 * 86_400),
            max_queue_bytes: 10 << 30,
            reports_per_connection: Rate::per(10.0, DAY),
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

    fn queues(&self, o: Origin) -> Rate {
        match o {
            Origin::Ip(_) => self.queues_per_ip,
            Origin::Onion => self.onion_queues,
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
        creations: HashMap::new(),
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
    // one global lock; shard per queue if a single relay gets busy.
    relay: Mutex<Relay>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Relay> {
        self.relay.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One connection, shared with every queue it is subscribed to.
struct Conn {
    id: u64,
    tx: mpsc::Sender<ServerFrame>,
    /// Missed a ping: a phone in deep sleep (ADR-012).
    asleep: AtomicBool,
    /// Its channel overflowed: end it, so the client reconnects and gets the
    /// whole queue from the database (ADR-007).
    kill: Notify,
}

struct Relay {
    db: db::Db,
    /// Who reads each queue right now, by `recv_id`.
    online: HashMap<QueueId, Arc<Conn>>,
    // grows with every queue used since start, bounded by the creation limit.
    mem: HashMap<QueueId, QueueMem>,
    connections: HashMap<Origin, usize>,
    creations: HashMap<Origin, Bucket>,
    next_conn: u64,
}

/// Per-queue state that never touches the disk (ADR-005).
struct QueueMem {
    sends: Bucket,
    publishes: Bucket,
    claimed: Bucket,
    /// The last UnifiedPush wake-up sent for this queue (ADR-012).
    pushed: Option<Instant>,
}

impl QueueMem {
    fn new(l: &Limits, now: Instant) -> Self {
        Self {
            sends: Bucket::full(l.sends_per_queue, now),
            publishes: Bucket::full(l.publishes_per_queue, now),
            claimed: Bucket::full(l.claims_per_queue, now),
            pushed: None,
        }
    }
}

/// Per-connection limits; they die with the connection.
struct ConnLimits {
    frames: Bucket,
    claims: Bucket,
    reports: Bucket,
}

/// At most one wake-up per queue in this time (ADR-012).
const PUSH_GAP: Duration = Duration::from_secs(30);

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

fn key_count(store: &db::Db, recv_id: QueueId) -> Result<ServerFrame, ErrorCode> {
    let (one_time_left, fallback_used) = db(store.key_count(&recv_id))?;
    Ok(ServerFrame::KeyCount {
        recv_id,
        one_time_left,
        fallback_used,
    })
}

fn random<const N: usize>() -> Result<[u8; N], ErrorCode> {
    let mut b = [0; N];
    getrandom::fill(&mut b).map_err(|_| ErrorCode::Internal)?;
    Ok(b)
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

/// Every 10 minutes: expire old envelopes and unused queues (ADR-005) and
/// forget per-IP state that has fully recovered, so it does not grow forever.
async fn cleanup(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    loop {
        tick.tick().await;
        let limits = &shared.config.limits;
        let (now, unix) = (Instant::now(), unix_now());
        let mut relay = shared.lock();
        relay.creations.retain(|origin, b| {
            let rate = limits.queues(*origin);
            b.refill(rate, now);
            b.tokens < rate.burst
        });
        // Queues read over a long-lived connection count as seen.
        let online: Vec<QueueId> = relay.online.keys().copied().collect();
        for q in &online {
            let _ = db(relay.db.touch(q, unix));
        }
        let ttl = |d: Duration| d.as_secs() as i64;
        let _ = db(relay
            .db
            .expire(unix, ttl(limits.queue_ttl), ttl(limits.idle_queue_ttl)));
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
    // Both limits: tungstenite reads a whole frame before it checks the
    // message size, so one 16 MiB frame would sit in memory otherwise.
    let mut response = ws
        .max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
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

async fn connection(shared: Arc<Shared>, mut ws: WebSocket, origin: Origin) {
    let limits = &shared.config.limits;
    let allowed = {
        let mut relay = shared.lock();
        let n = relay.connections.entry(origin).or_default();
        let ok = *n < limits.connections(origin);
        if ok {
            *n += 1;
        }
        ok
    };
    if !allowed {
        let _ = send(
            &mut ws,
            &error(None, ErrorCode::RateLimited),
            limits.send_timeout,
        )
        .await;
        return;
    }

    // ADR-022: a slow reader holds at most one queue page here.
    let (tx, rx) = mpsc::channel(QUEUE_PAGE);
    let conn = {
        let mut relay = shared.lock();
        relay.next_conn += 1;
        Arc::new(Conn {
            id: relay.next_conn,
            tx,
            asleep: AtomicBool::new(false),
            kill: Notify::new(),
        })
    };
    let hello = match tokio::time::timeout(limits.hello_timeout, recv(&mut ws)).await {
        Ok(Ok(ClientFrame::Hello { version })) if version == VERSION => Ok(()),
        // A version 1 client reads this as its own `Error` (proto, ServerFrame).
        Ok(Ok(ClientFrame::Hello { .. })) => Err(ErrorCode::UnsupportedVersion),
        Ok(Ok(_)) => Err(ErrorCode::Malformed),
        Ok(Err(code)) => Err(code),
        Err(_) => Err(ErrorCode::Malformed),
    };
    let result = match hello {
        Ok(()) => serve_client(&shared, &mut ws, &conn, rx, origin).await,
        Err(code) => Err(code),
    };
    if let Err(code) = result {
        let _ = send(&mut ws, &error(None, code), limits.send_timeout).await;
    }

    let mut relay = shared.lock();
    relay.online.retain(|_, c| c.id != conn.id);
    if let Some(n) = relay.connections.get_mut(&origin) {
        *n -= 1;
        if *n == 0 {
            relay.connections.remove(&origin);
        }
    }
}

/// Envelopes read from the queue at once: a connection that subscribes and
/// then does not read must not pin a whole queue (10 MiB) in memory.
const QUEUE_PAGE: usize = 32;

/// What `handle` leaves for the async loop: the queue just subscribed to,
/// whose stored envelopes go out page by page.
type Backlog = Option<QueueId>;

async fn serve_client(
    shared: &Shared,
    ws: &mut WebSocket,
    conn: &Arc<Conn>,
    mut rx: mpsc::Receiver<ServerFrame>,
    origin: Origin,
) -> Result<(), ErrorCode> {
    let limits = &shared.config.limits;
    let now = Instant::now();
    let mut mine = ConnLimits {
        frames: Bucket::full(limits.frames_per_connection, now),
        claims: Bucket::full(limits.claims_per_connection, now),
        reports: Bucket::full(limits.reports_per_connection, now),
    };
    // Per subscribed queue: envelopes up to this id went out in its pages.
    let mut sent_up_to: HashMap<QueueId, u64> = HashMap::new();
    let mut ping = tokio::time::interval(limits.ping_interval);
    let mut last_seen = Instant::now();
    let mut pinged = last_seen;
    loop {
        tokio::select! {
            msg = ws.recv() => {
                last_seen = Instant::now();
                conn.asleep.store(false, Ordering::Relaxed);
                match msg {
                    Some(Ok(Message::Binary(bytes))) => {
                        let frame = ClientFrame::decode(&bytes).map_err(ErrorCode::from)?;
                        let (replies, backlog) = handle(shared, conn, &mut mine, origin, frame)?;
                        for reply in replies {
                            send(ws, &reply, limits.send_timeout).await?;
                        }
                        if let Some(q) = backlog {
                            let up_to = send_backlog(shared, ws, q).await?;
                            sent_up_to.insert(q, up_to);
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Text(_))) => return Err(ErrorCode::Malformed),
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Ok(()),
                }
            }
            frame = rx.recv() => match frame {
                // Queued while its pages went out, and sent with them.
                Some(ServerFrame::Deliver { recv_id, relay_id, .. })
                    if sent_up_to.get(&recv_id).is_some_and(|&n| relay_id <= n) => {}
                Some(frame) => send(ws, &frame, limits.send_timeout).await?,
                None => return Ok(()),
            },
            () = conn.kill.notified() => return Ok(()),
            _ = ping.tick() => {
                if last_seen.elapsed() > limits.idle_timeout {
                    return Ok(());
                }
                // ADR-012, the sleeping link: a phone in deep sleep answers no
                // ping, yet its kernel takes in what we send. Hold the link
                // (up to `idle_timeout`) and push for what goes into it.
                conn.asleep.store(last_seen < pinged, Ordering::Relaxed);
                pinged = Instant::now();
                write(ws, Message::Ping(Default::default()), limits.send_timeout).await?;
            }
        }
    }
}

/// Stored envelopes of a queue just subscribed to, page by page, then its
/// key count. Returns the last id sent.
async fn send_backlog(shared: &Shared, ws: &mut WebSocket, q: QueueId) -> Result<u64, ErrorCode> {
    let timeout = shared.config.limits.send_timeout;
    let mut sent_up_to = 0;
    loop {
        let (page, last) = db(shared.lock().db.queued_after(&q, sent_up_to, QUEUE_PAGE))?;
        if page.is_empty() {
            break;
        }
        sent_up_to = last;
        for e in page {
            let frame = ServerFrame::Deliver {
                recv_id: q,
                relay_id: e.id,
                envelope: e.envelope,
            };
            send(ws, &frame, timeout).await?;
        }
    }
    let count = key_count(&shared.lock().db, q)?;
    send(ws, &count, timeout).await?;
    Ok(sent_up_to)
}

/// Handles one frame after `Hello`. `Err` closes the connection.
fn handle(
    shared: &Shared,
    conn: &Arc<Conn>,
    mine: &mut ConnLimits,
    origin: Origin,
    frame: ClientFrame,
) -> Result<(Vec<ServerFrame>, Backlog), ErrorCode> {
    let limits = &shared.config.limits;
    let now = Instant::now();
    // Flooding frames closes the connection.
    if !mine.frames.take(limits.frames_per_connection, now) {
        return Err(ErrorCode::RateLimited);
    }
    let mut relay = shared.lock();
    let relay = &mut *relay;
    let unknown = |client_id| Ok((vec![error(client_id, ErrorCode::UnknownQueue)], None));
    // The owner's frames: a wrong token gets the same answer as no queue.
    let owned = |relay: &Relay, recv_id: &QueueId, token: &QueueToken| -> Result<bool, ErrorCode> {
        db(relay.db.owns(recv_id, token))
    };
    let reply = |frames| Ok((frames, None));

    match frame {
        ClientFrame::Hello { .. } => Err(ErrorCode::Malformed),

        ClientFrame::CreateQueue => {
            if !created(relay, limits, origin, now) {
                return reply(vec![error(None, ErrorCode::RateLimited)]);
            }
            let (recv_id, send_id, token) = (random()?, random()?, random()?);
            db(relay.db.create(&recv_id, &send_id, &token, unix_now()))?;
            reply(vec![ServerFrame::QueueCreated {
                recv_id,
                send_id,
                token,
            }])
        }

        ClientFrame::DeleteQueue { recv_id, token } => {
            if !owned(relay, &recv_id, &token)? {
                return unknown(None);
            }
            db(relay.db.delete(&recv_id))?;
            relay.mem.remove(&recv_id);
            relay.online.remove(&recv_id);
            reply(vec![ServerFrame::QueueDeleted { recv_id }])
        }

        ClientFrame::Subscribe { recv_id, token } => {
            if !owned(relay, &recv_id, &token)? {
                return unknown(None);
            }
            db(relay.db.touch(&recv_id, unix_now()))?;
            // A newer connection takes the queue over; the old one just stops getting it.
            relay.online.insert(recv_id, Arc::clone(conn));
            Ok((vec![ServerFrame::Subscribed { recv_id }], Some(recv_id)))
        }

        ClientFrame::PublishKeys {
            recv_id,
            token,
            one_time,
            fallback,
        } => {
            if !owned(relay, &recv_id, &token)? {
                return unknown(None);
            }
            let q = relay
                .mem
                .entry(recv_id)
                .or_insert_with(|| QueueMem::new(limits, now));
            if !q.publishes.take(limits.publishes_per_queue, now) {
                return reply(vec![error(None, ErrorCode::RateLimited)]);
            }
            db(relay.db.publish(
                &recv_id,
                &one_time,
                fallback.as_ref(),
                limits.stored_one_time_keys,
            ))?;
            reply(vec![key_count(&relay.db, recv_id)?])
        }

        ClientFrame::ClaimKey { send_id } => {
            if !mine.claims.take(limits.claims_per_connection, now) {
                return reply(vec![error(None, ErrorCode::RateLimited)]);
            }
            let Some(recv_id) = db(relay.db.recv_id(&send_id))? else {
                return unknown(None);
            };
            // Past the per-queue limit only the fallback key is handed out.
            let one_time_allowed = relay
                .mem
                .entry(recv_id)
                .or_insert_with(|| QueueMem::new(limits, now))
                .claimed
                .take(limits.claims_per_queue, now);
            let Some(key) = db(relay.db.claim(&recv_id, one_time_allowed))? else {
                return reply(vec![error(None, ErrorCode::NoKeys)]);
            };
            if let Some(owner) = relay.online.get(&recv_id) {
                let _ = owner.tx.try_send(key_count(&relay.db, recv_id)?);
            }
            reply(vec![ServerFrame::ClaimedKey { send_id, key }])
        }

        ClientFrame::Send {
            client_id,
            send_id,
            envelope,
        } => {
            if let Some(to) = db(relay.db.recv_id(&send_id))? {
                return match put(relay, limits, to, envelope, now)? {
                    None => reply(vec![ServerFrame::Accepted { client_id }]),
                    Some(code) => reply(vec![error(Some(client_id), code)]),
                };
            }
            // ADR-027: a group's message goes to each reader's queue. One
            // reader with a full queue does not hold up the others.
            let Some(group) = db(relay.db.group_of(&send_id, unix_now()))? else {
                return unknown(Some(client_id));
            };
            let g = relay
                .mem
                .entry(group)
                .or_insert_with(|| QueueMem::new(limits, now));
            if !g.sends.take(limits.sends_per_queue, now) {
                return reply(vec![error(Some(client_id), ErrorCode::RateLimited)]);
            }
            for reader in db(relay.db.readers(&group))? {
                put(relay, limits, reader, envelope.clone(), now)?;
            }
            reply(vec![ServerFrame::Accepted { client_id }])
        }

        ClientFrame::CreateGroup => {
            if !created(relay, limits, origin, now) {
                return reply(vec![error(None, ErrorCode::RateLimited)]);
            }
            let (group_id, send_id, token) = (random()?, random()?, random()?);
            db(relay
                .db
                .create_group(&group_id, &send_id, &token, unix_now()))?;
            reply(vec![ServerFrame::GroupCreated {
                group_id,
                send_id,
                token,
            }])
        }

        ClientFrame::AddReader { group_id, token } => {
            if !db(relay.db.owns_group(&group_id, &token))? {
                return unknown(None);
            }
            if db(relay.db.readers(&group_id))?.len() >= MAX_GROUP_MEMBERS {
                return reply(vec![error(None, ErrorCode::QueueFull)]);
            }
            if !created(relay, limits, origin, now) {
                return reply(vec![error(None, ErrorCode::RateLimited)]);
            }
            let (recv_id, send_id, token) = (random()?, random()?, random()?);
            db(relay
                .db
                .add_reader(&group_id, &recv_id, &send_id, &token, unix_now()))?;
            reply(vec![ServerFrame::ReaderAdded {
                group_id,
                recv_id,
                send_id,
                token,
            }])
        }

        ClientFrame::RemoveReader {
            group_id,
            token,
            recv_id,
        } => {
            if !db(relay.db.owns_group(&group_id, &token))?
                || !db(relay.db.is_reader(&group_id, &recv_id))?
            {
                return unknown(None);
            }
            db(relay.db.delete(&recv_id))?;
            relay.mem.remove(&recv_id);
            relay.online.remove(&recv_id);
            reply(vec![ServerFrame::ReaderRemoved { recv_id }])
        }

        ClientFrame::DeleteGroup { group_id, token } => {
            if !db(relay.db.owns_group(&group_id, &token))? {
                return unknown(None);
            }
            for reader in db(relay.db.readers(&group_id))? {
                relay.mem.remove(&reader);
                relay.online.remove(&reader);
            }
            db(relay.db.delete_group(&group_id))?;
            relay.mem.remove(&group_id);
            reply(vec![ServerFrame::GroupDeleted { group_id }])
        }

        ClientFrame::Report {
            recv_id,
            token,
            text,
        } => {
            // Same answer on every path. No ban: the relay knows no sender (ADR-023).
            if mine.reports.take(limits.reports_per_connection, now)
                && owned(relay, &recv_id, &token)?
                && let Some(path) = &shared.config.reports
                && let Err(e) = journal(path, &text)
            {
                tracing::warn!("cannot write report journal: {e}");
            }
            reply(vec![ServerFrame::Reported])
        }

        // Only a subscriber of the queue may acknowledge what is in it.
        ClientFrame::Ack { recv_id, relay_id } => {
            if relay.online.get(&recv_id).is_some_and(|c| c.id == conn.id) {
                db(relay.db.ack(&recv_id, relay_id))?;
            }
            reply(vec![])
        }

        // ADR-012. Only the shape is checked here; the address when it is used.
        ClientFrame::SetPush {
            recv_id,
            token,
            endpoint,
        } => {
            if !owned(relay, &recv_id, &token)? {
                return unknown(None);
            }
            if endpoint
                .as_deref()
                .is_some_and(|u| push::parse(u).is_none())
            {
                return reply(vec![error(None, ErrorCode::Malformed)]);
            }
            db(relay.db.set_push(&recv_id, endpoint.as_deref()))?;
            reply(vec![ServerFrame::PushSet])
        }
    }
}

/// ADR-022: the report journal stops growing here.
const MAX_JOURNAL: u64 = 10 << 20;

/// One JSON line per report: time and the disclosed text. No queue, no IP
/// (rule 14). The operator deletes entries once reviewed.
// file IO under the global lock; reports are rare and rate-limited.
fn journal(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = serde_json::json!({ "at": at, "text": text });
    // ADR-022: past the cap a report's text is not kept.
    if std::fs::metadata(path).is_ok_and(|m| m.len() >= MAX_JOURNAL) {
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "report journal is full ({MAX_JOURNAL} bytes); new report texts are dropped"
            );
        }
        return Ok(());
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    writeln!(opts.open(path)?, "{line}")
}

/// One more queue from `origin` (a queue, a group or a reader), if its budget allows.
fn created(relay: &mut Relay, limits: &Limits, origin: Origin, now: Instant) -> bool {
    let rate = limits.queues(origin);
    relay
        .creations
        .entry(origin)
        .or_insert_with(|| Bucket::full(rate, now))
        .take(rate, now)
}

/// Stores `envelope` in queue `to`, hands it to the reader if connected, or
/// wakes them. `Some(code)`: the queue refused it.
fn put(
    relay: &mut Relay,
    limits: &Limits,
    to: QueueId,
    envelope: Envelope,
    now: Instant,
) -> Result<Option<ErrorCode>, ErrorCode> {
    let q = relay
        .mem
        .entry(to)
        .or_insert_with(|| QueueMem::new(limits, now));
    if !q.sends.take(limits.sends_per_queue, now) {
        return Ok(Some(ErrorCode::RateLimited));
    }
    let size = envelope.body.len() as u64;
    let (count, bytes) = db(relay.db.queue_stats(&to))?;
    if count >= limits.queue_envelopes
        || bytes + size > limits.queue_bytes as u64
        || relay.db.total_queue_bytes() + size > limits.max_queue_bytes
    {
        return Ok(Some(ErrorCode::QueueFull));
    }
    let relay_id = db(relay.db.enqueue(&to, &envelope, unix_now()))?;
    let deliver = ServerFrame::Deliver {
        recv_id: to,
        relay_id,
        envelope,
    };
    // A full channel means the reader lags: end its connection so it
    // reconnects and gets the whole queue (ADR-007).
    // Not here, or asleep (ADR-012): wake it, if it asked for that.
    let wake = match relay.online.get(&to) {
        Some(owner) => {
            if owner.tx.try_send(deliver).is_err() {
                owner.kill.notify_one();
            }
            owner.asleep.load(Ordering::Relaxed)
        }
        None => true,
    };
    if wake
        && let Some(q) = relay.mem.get_mut(&to)
        && q.pushed.is_none_or(|at| now.duration_since(at) >= PUSH_GAP)
        && let Some(url) = db(relay.db.push(&to))?
    {
        q.pushed = Some(now);
        tokio::spawn(push::wake(url));
    }
    Ok(None)
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
    fn journal_holds_only_time_and_text() {
        let path = std::env::temp_dir().join(format!("termchat-reports-{}", std::process::id()));
        journal(&path, "line \"one\"\nline two").unwrap();
        let line = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["text"], "line \"one\"\nline two");
        assert_eq!(v.as_object().unwrap().len(), 2, "no queue, no IP");
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
