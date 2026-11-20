//! Relay transport: WebSocket with reconnect. No login: frames name queues (ADR-023).
//!
//! Knows nothing about encryption: frames in, frames out. Unacknowledged
//! `Send` frames are resent after every reconnect (protocol.md, section 13),
//! and after a temporary refusal (ADR-020).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use futures_util::{SinkExt, StreamExt};
use termchat_proto::{ClientFrame, ErrorCode, QueueId, ServerFrame, VERSION, VERSION_HEADER};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{WebSocketStream, connect_async_with_config};

const MIN_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(60);
/// The relay pings every 30 s; silence this long means the link is dead.
const IDLE: Duration = Duration::from_secs(90);
/// First wait before a refused `Send` goes again; doubles up to `MAX_DELAY` (ADR-020).
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// ADR-012: on a wake-up, a link that heard nothing for this long (the relay
/// pings every 30 s) is checked with a ping of ours.
const STALE: Duration = Duration::from_secs(35);
/// ADR-012, the sleeping link: a checked link silent this long is dialled again.
const PROBE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `Hello` is out. Subscribe, publish keys and claim what you need again.
    Connected,
    Disconnected,
    Frame(ServerFrame),
    /// The relay's version, sent before `Connected` if it said one (ADR-010).
    RelayVersion(String),
}

/// How to reach the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dial {
    Direct,
    /// Only through Tor, with Arti's state in this folder (ADR-004). Never falls
    /// back to a direct connection, so the IP cannot leak.
    Tor(PathBuf, Bridges),
}

/// Tor bridges and the transport program that reaches them (ADR-015).
/// Bridge lines hold secrets (`cert=`), so `Debug` shows only how many.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Bridges {
    pub lines: Vec<String>,
    /// lyrebird (or obfs4proxy, obfs4 only).
    pub program: Option<PathBuf>,
}

impl std::fmt::Debug for Bridges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bridges({} lines)", self.lines.len())
    }
}

/// Transports lyrebird carries that termchat uses (ADR-015, section 3).
pub const TRANSPORTS: [&str; 3] = ["obfs4", "webtunnel", "snowflake"];

/// The transport a bridge line names, if Arti accepts the line. A leading
/// `Bridge ` (as torrc writes it) is fine.
#[cfg(feature = "tor")]
pub fn bridge_transport(line: &str) -> Option<String> {
    let b: arti_client::config::BridgeConfigBuilder = line.trim().parse().ok()?;
    b.build().ok()?;
    let name = line
        .trim()
        .trim_start_matches("Bridge ")
        .split_whitespace()
        .next()?;
    TRANSPORTS.contains(&name).then(|| name.to_string())
}

#[cfg(not(feature = "tor"))]
pub fn bridge_transport(_line: &str) -> Option<String> {
    None
}

/// Handle to the background connection. Dropping it closes the connection.
#[derive(Clone)]
pub struct Relay {
    tx: mpsc::UnboundedSender<Outgoing>,
    wake: Arc<Notify>,
    pushed: Arc<AtomicBool>,
    /// `Send` frames the relay has not answered yet, queued ones included.
    unanswered: Arc<AtomicUsize>,
}

struct Outgoing {
    frame: ClientFrame,
    group_session: Option<Arc<AtomicBool>>,
}

impl Outgoing {
    fn cancelled(&self) -> bool {
        self.group_session
            .as_ref()
            .is_some_and(|session| !session.load(Ordering::Acquire))
    }
}

impl Relay {
    /// Starts connecting in the background. `url` is `wss://host/` (or `ws://` for tests).
    pub fn connect(url: &str, dial: Dial) -> Result<(Self, mpsc::Receiver<Event>), Error> {
        Self::connect_after(url, dial, Duration::ZERO, false)
    }

    /// Like `connect`, but waits `delay` first, and with `spread` also a
    /// random 0-30 s before every reconnect, so the links of one device do
    /// not come and go on the relay together (ADR-014, ADR-023).
    pub fn connect_after(
        url: &str,
        dial: Dial,
        delay: Duration,
        spread: bool,
    ) -> Result<(Self, mpsc::Receiver<Event>), Error> {
        let uri = url.parse::<Uri>().map_err(|_| Error::BadUrl)?;
        let host = uri.host().map(str::to_owned).ok_or(Error::BadUrl)?;
        // Plain ws:// only where TLS adds nothing: this machine, or an onion
        // service. Queue tokens must not cross a network in the clear.
        // An onion host only through Tor: directly it would go through plain
        // DNS, and whoever answers that stands in for the relay (ADR-004).
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let onion = bare.ends_with(".onion");
        let tor = matches!(dial, Dial::Tor(..));
        if onion && !tor {
            return Err(Error::BadUrl);
        }
        let local = matches!(bare, "localhost" | "127.0.0.1" | "::1") || onion;
        match uri.scheme_str() {
            Some("wss") => {}
            Some("ws") if local => {}
            _ => return Err(Error::BadUrl),
        }
        #[cfg(feature = "tor")]
        let port = uri
            .port_u16()
            .unwrap_or(if uri.scheme_str() == Some("wss") {
                443
            } else {
                80
            });
        #[cfg(not(feature = "tor"))]
        if matches!(dial, Dial::Tor(..)) {
            return Err(Error::NoTor);
        }
        // Ignore the error: a provider may already be installed by the app.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (tx, rx) = mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::channel(256);
        let wake = Arc::new(Notify::new());
        let pushed = Arc::new(AtomicBool::new(false));
        let unanswered = Arc::new(AtomicUsize::new(0));
        let link = Link {
            url: url.to_owned(),
            #[cfg(feature = "tor")]
            host,
            #[cfg(feature = "tor")]
            port,
            dial,
            #[cfg(feature = "tor")]
            tor: None,
            events: events_tx,
            pending: BTreeMap::new(),
            in_flight: BTreeMap::new(),
            retry: BTreeMap::new(),
            relay_version: None,
            delay,
            spread,
            wake: Arc::clone(&wake),
            pushed: Arc::clone(&pushed),
            unanswered: Arc::clone(&unanswered),
            heard: SystemTime::now(),
        };
        tokio::spawn(link.run(rx));
        Ok((
            Self {
                tx,
                wake,
                pushed,
                unanswered,
            },
            events,
        ))
    }

    /// Queues a frame. `Send` frames survive reconnects until the relay answers.
    pub fn send(&self, frame: ClientFrame) -> Result<(), Error> {
        self.send_scoped(frame, None)
    }

    pub(crate) fn send_scoped(
        &self,
        frame: ClientFrame,
        group_session: Option<Arc<AtomicBool>>,
    ) -> Result<(), Error> {
        let counted = matches!(frame, ClientFrame::Send { .. });
        if counted {
            self.unanswered.fetch_add(1, Ordering::Relaxed);
        }
        self.tx
            .send(Outgoing {
                frame,
                group_session,
            })
            .map_err(|_| {
                if counted {
                    self.unanswered.fetch_sub(1, Ordering::Relaxed);
                }
                Error::Closed
            })
    }

    /// `Send` frames not yet accepted or refused for good. A process that
    /// exits before this is 0 loses them: the ratchet has moved on already.
    pub fn unanswered(&self) -> usize {
        self.unanswered.load(Ordering::Relaxed)
    }

    /// The app woke up or came on screen (ADR-012): stop waiting to reconnect,
    /// and drop a connection the phone may have let die while it slept.
    pub fn reconnect(&self) {
        self.wake.notify_one();
    }

    /// A push for this identity (ADR-012, the sleeping link): the relay holds
    /// our connection but heard nothing from it, so the message may already
    /// sit in the socket. Read it, and check the link with a ping however
    /// recent its last word; dial again only if the ping goes unanswered.
    pub fn check(&self) {
        self.pushed.store(true, Ordering::Relaxed);
        self.wake.notify_one();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadUrl,
    Closed,
    /// This build was made without the `tor` feature.
    NoTor,
}

struct Link {
    url: String,
    #[cfg(feature = "tor")]
    host: String,
    #[cfg(feature = "tor")]
    port: u16,
    dial: Dial,
    /// Bootstrapped once, reused across reconnects.
    #[cfg(feature = "tor")]
    tor: Option<std::sync::Arc<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>>,
    events: mpsc::Sender<Event>,
    /// Unanswered `Send` frames by `client_id`.
    pending: BTreeMap<u64, Outgoing>,
    /// ADR-020: per queue written to, the one `Send` on the wire, so the
    /// relay takes them in order even when it refuses one for now.
    in_flight: BTreeMap<QueueId, u64>,
    /// Queues with a refused `Send` that was not yet accepted.
    retry: BTreeMap<QueueId, Retry>,
    /// From the last upgrade response, until `talk` reports it.
    relay_version: Option<String>,
    /// Before the first connection.
    delay: Duration,
    /// Add 0-30 s to every reconnect.
    spread: bool,
    wake: Arc<Notify>,
    /// Set by `Relay::check`: a push came, check the connection held now.
    pushed: Arc<AtomicBool>,
    /// Shared with `Relay::unanswered`.
    unanswered: Arc<AtomicUsize>,
    /// Last word from the relay, wall clock: the monotonic one stops while
    /// the phone sleeps.
    heard: SystemTime,
}

enum Ws {
    Direct(Box<WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>),
    #[cfg(feature = "tor")]
    Tor(Box<WebSocketStream<arti_client::DataStream>>),
}

/// ADR-020: a refused `Send` waiting to go again.
struct Retry {
    /// When it goes again; `None` while that attempt is on the wire.
    due: Option<Instant>,
    /// The wait after its next refusal.
    wait: Duration,
}

enum Answer {
    /// A frame unrelated to `Send`.
    Other,
    /// An answer with no matching pending send.
    Unsolicited,
    /// Refused for now; the engine is not told.
    Retry,
    /// Accepted or refused for good; this recipient's queue moves on.
    Done(QueueId),
}

enum End {
    /// The app dropped every `Relay` handle.
    Stop,
    /// Connection lost; `true` if it had got as far as `Hello`.
    Lost(bool),
}

impl Link {
    fn spread_delay(&self) -> Duration {
        match self.spread {
            true => Duration::from_millis(u64::from(getrandom::u32().unwrap_or(0) % 30_000)),
            false => Duration::ZERO,
        }
    }

    async fn run(mut self, mut out: mpsc::UnboundedReceiver<Outgoing>) {
        tokio::select! {
            _ = tokio::time::sleep(self.delay) => {}
            _ = self.events.closed() => return,
        }
        let mut delay = MIN_DELAY;
        loop {
            match self.session(&mut out).await {
                End::Stop => return,
                End::Lost(authed) => {
                    if authed {
                        delay = MIN_DELAY;
                        if self.events.send(Event::Disconnected).await.is_err() {
                            return;
                        }
                    }
                }
            }
            // Stop waiting as soon as the app is gone (e.g. locked).
            // A wake-up cuts the backoff short, never the spread: identities
            // must not come back together (ADR-014).
            tokio::select! {
                _ = tokio::time::sleep(jitter(delay)) => {}
                () = self.wake.notified() => {}
                _ = self.events.closed() => return,
            }
            tokio::select! {
                _ = tokio::time::sleep(self.spread_delay()) => {}
                _ = self.events.closed() => return,
            }
            delay = (delay * 2).min(MAX_DELAY);
        }
    }

    /// Connecting can take long (TCP timeouts, Tor bootstrap). The app may lock
    /// meanwhile, and the link must not outlive it: give up on the
    /// connection attempt as soon as the event receiver is gone. Once connected,
    /// `talk` drains queued frames (acks) before it stops.
    async fn session(&mut self, out: &mut mpsc::UnboundedReceiver<Outgoing>) -> End {
        let events = self.events.clone();
        let connected = tokio::select! {
            ws = self.connect() => ws,
            _ = events.closed() => return End::Stop,
        };
        match connected {
            Some(Ws::Direct(ws)) => self.talk(*ws, out).await,
            #[cfg(feature = "tor")]
            Some(Ws::Tor(ws)) => self.talk(*ws, out).await,
            None => End::Lost(false),
        }
    }

    async fn connect(&mut self) -> Option<Ws> {
        match self.dial.clone() {
            Dial::Direct => {
                let (ws, response) =
                    connect_async_with_config(self.url.as_str(), Some(ws_config()), false)
                        .await
                        .ok()?;
                self.relay_version = version(&response);
                Some(Ws::Direct(Box::new(ws)))
            }
            #[cfg(feature = "tor")]
            Dial::Tor(dir, bridges) => {
                let stream = self.tor_stream(&dir, &bridges).await?;
                let (ws, response) = tokio_tungstenite::client_async_with_config(
                    self.url.as_str(),
                    stream,
                    Some(ws_config()),
                )
                .await
                .ok()?;
                self.relay_version = version(&response);
                Some(Ws::Tor(Box::new(ws)))
            }
            #[cfg(not(feature = "tor"))]
            Dial::Tor(..) => None,
        }
    }

    #[cfg(feature = "tor")]
    async fn tor_stream(
        &mut self,
        dir: &std::path::Path,
        bridges: &Bridges,
    ) -> Option<arti_client::DataStream> {
        if self.tor.is_none() {
            // Its own circuits: two identities never share one (ADR-014).
            self.tor = Some(shared_tor(dir, bridges).await?.isolated_client());
        }
        self.tor
            .as_ref()?
            .connect((self.host.as_str(), self.port))
            .await
            .ok()
    }

    async fn talk<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        mut ws: WebSocketStream<S>,
        out: &mut mpsc::UnboundedReceiver<Outgoing>,
    ) -> End {
        if self.handshake(&mut ws).await.is_none() {
            return End::Lost(false);
        }
        if let Some(v) = self.relay_version.take()
            && self.events.send(Event::RelayVersion(v)).await.is_err()
        {
            return End::Lost(false);
        }
        if self.events.send(Event::Connected).await.is_err() {
            return End::Lost(false);
        }
        // Waits end with the connection: the reconnect was its own wait.
        self.in_flight.clear();
        self.retry.clear();
        let all: BTreeSet<QueueId> = self
            .pending
            .values()
            .filter_map(|o| queue(&o.frame))
            .collect();
        for to in all {
            if self.pump(&mut ws, to).await.is_none() {
                return End::Lost(true);
            }
        }
        self.heard = SystemTime::now();
        // A check asked for before this connection came up is answered by it.
        self.pushed.store(false, Ordering::Relaxed);
        let wake = Arc::clone(&self.wake);
        // A ping of ours waits for an answer: the deadline, and when it went.
        let mut probe: Option<(Instant, SystemTime)> = None;
        loop {
            let due = self.retry.values().filter_map(|r| r.due).min();
            tokio::select! {
                () = wake.notified() => {
                    let quiet = self.heard.elapsed().unwrap_or(Duration::ZERO);
                    let pushed = self.pushed.swap(false, Ordering::Relaxed);
                    if (pushed || quiet > STALE) && probe.is_none() {
                        if ws.send(Message::Ping(Default::default())).await.is_err() {
                            return End::Lost(true);
                        }
                        probe = Some((Instant::now() + PROBE, SystemTime::now()));
                    }
                }
                () = sleep_until(probe.map(|(at, _)| at)) => {
                    // Any word from the relay since the ping (its pong, a frame) will do.
                    let asked = probe.take().map_or(SystemTime::now(), |(_, t)| t);
                    if self.heard < asked {
                        return End::Lost(true);
                    }
                }
                frame = out.recv() => {
                    let Some(outgoing) = frame else { return End::Stop };
                    let sent = match &outgoing.frame {
                        ClientFrame::Send { client_id, send_id, .. } => {
                            let to = *send_id;
                            if self.pending.insert(*client_id, outgoing).is_some() {
                                // The same id twice counts once.
                                self.unanswered.fetch_sub(1, Ordering::Relaxed);
                            }
                            self.pump(&mut ws, to).await
                        }
                        _ => write(&mut ws, &outgoing.frame).await,
                    };
                    if sent.is_none() {
                        return End::Lost(true);
                    }
                }
                frame = read(&mut ws, &mut self.heard) => {
                    let Some(frame) = frame else { return End::Lost(true) };
                    match self.answered(&frame) {
                        Answer::Other => {}
                        Answer::Retry | Answer::Unsolicited => continue,
                        Answer::Done(to) => {
                            if self.pump(&mut ws, to).await.is_none() {
                                return End::Lost(true);
                            }
                        }
                    }
                    if self.events.send(Event::Frame(frame)).await.is_err() {
                        return End::Stop;
                    }
                }
                () = sleep_until(due) => {
                    let now = Instant::now();
                    let mut ready = Vec::new();
                    for (to, r) in &mut self.retry {
                        if r.due.is_some_and(|at| at <= now) {
                            r.due = None;
                            ready.push(*to);
                        }
                    }
                    for to in ready {
                        if self.pump(&mut ws, to).await.is_none() {
                            return End::Lost(true);
                        }
                    }
                }
            }
        }
    }

    /// Bookkeeping for a relay answer to one of our `Send` frames.
    fn answered(&mut self, frame: &ServerFrame) -> Answer {
        let id = match frame {
            ServerFrame::Accepted { client_id }
            | ServerFrame::Error {
                client_id: Some(client_id),
                ..
            } => *client_id,
            _ => return Answer::Other,
        };
        let Some(to) = self.pending.get(&id).and_then(|o| queue(&o.frame)) else {
            return Answer::Unsolicited;
        };
        if self.in_flight.get(&to) != Some(&id) {
            return Answer::Unsolicited;
        }
        self.in_flight.remove(&to);
        if let ServerFrame::Error { code, .. } = frame
            && matches!(
                code,
                ErrorCode::RateLimited | ErrorCode::QueueFull | ErrorCode::Internal
            )
        {
            // Refused for now: the same bytes go again later, unseen by the engine.
            let r = self.retry.entry(to).or_insert(Retry {
                due: None,
                wait: RETRY_DELAY,
            });
            r.due = Some(Instant::now() + jitter(r.wait));
            r.wait = (r.wait * 2).min(MAX_DELAY);
            return Answer::Retry;
        }
        if self.pending.remove(&id).is_some() {
            self.unanswered.fetch_sub(1, Ordering::Relaxed);
        }
        self.retry.remove(&to);
        Answer::Done(to)
    }

    /// Puts the oldest unanswered frame to `to` on the wire, unless one is
    /// there already or a refused one waits for its retry.
    async fn pump<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        ws: &mut WebSocketStream<S>,
        to: QueueId,
    ) -> Option<()> {
        let waiting = self.retry.get(&to).is_some_and(|r| r.due.is_some());
        if waiting || self.in_flight.contains_key(&to) {
            return Some(());
        }
        loop {
            let next = self
                .pending
                .iter()
                .find(|(_, o)| queue(&o.frame) == Some(to));
            let Some((&id, outgoing)) = next else {
                return Some(());
            };
            // A move or revocation invalidates ciphertext still queued here.
            if !outgoing.cancelled() {
                self.in_flight.insert(to, id);
                return write(ws, &outgoing.frame).await;
            }
            self.pending.remove(&id);
            self.unanswered.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Nothing to wait for: a relay that refuses the version says so in a
    /// frame, which the engine shows.
    async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        ws: &mut WebSocketStream<S>,
    ) -> Option<()> {
        write(ws, &ClientFrame::Hello { version: VERSION }).await
    }
}

/// The relay's own frame limit (protocol.md, section 11) instead of
/// tungstenite's 64 MiB: a hostile relay must not make the client buffer that.
fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(termchat_proto::MAX_FRAME_BYTES))
        .max_frame_size(Some(termchat_proto::MAX_FRAME_BYTES))
}

async fn write<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    frame: &ClientFrame,
) -> Option<()> {
    ws.send(Message::Binary(frame.encode().into())).await.ok()
}

/// Next frame; `None` on close, error, garbage or silence longer than `IDLE`.
/// Pings are answered by tungstenite itself while reading; any of them sets `heard`.
async fn read<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    heard: &mut SystemTime,
) -> Option<ServerFrame> {
    loop {
        let msg = tokio::time::timeout(IDLE, ws.next()).await.ok()??;
        *heard = SystemTime::now();
        match msg {
            Ok(Message::Binary(bytes)) => return ServerFrame::decode(&bytes).ok(),
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            _ => return None,
        }
    }
}

/// The relay's version header, only if it reads `digits.digits.digits`:
/// nothing else from the relay is ever shown (ADR-010).
fn version(
    response: &tokio_tungstenite::tungstenite::handshake::client::Response,
) -> Option<String> {
    let v = response.headers().get(VERSION_HEADER)?.to_str().ok()?;
    parse_version(v).map(|_| v.to_owned())
}

/// `major.minor.patch` as numbers, for comparing.
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.split('.').map(|p| {
        p.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| p.parse().ok())
            .flatten()
    });
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

#[cfg(feature = "tor")]
type Tor = std::sync::Arc<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>;

/// One bootstrapped Tor client per process: Arti locks its state folder, and
/// bootstrapping once is much faster. Each link takes an isolated handle.
#[cfg(feature = "tor")]
type Shared = Option<(PathBuf, Bridges, Tor)>;
#[cfg(feature = "tor")]
static TOR: tokio::sync::Mutex<Shared> = tokio::sync::Mutex::const_new(None);

/// Lets go of the shared Tor client, so its folder can be erased: Arti keeps
/// bridge lines there in the clear (ADR-015). A client still bootstrapping
/// is left alone; its links are gone, so it stops on its own.
pub fn forget_tor() {
    #[cfg(feature = "tor")]
    if let Ok(mut tor) = TOR.try_lock() {
        *tor = None;
    }
}

#[cfg(feature = "tor")]
async fn shared_tor(dir: &std::path::Path, bridges: &Bridges) -> Option<Tor> {
    let mut tor = TOR.lock().await;
    if let Some((d, b, client)) = &*tor
        && d == dir
        && b == bridges
    {
        return Some(client.clone());
    }
    // A client with other bridges must let go of the state folder first.
    *tor = None;
    #[cfg_attr(not(target_os = "android"), allow(unused_mut))]
    let mut config = arti_client::config::TorClientConfigBuilder::from_directories(
        dir.join("state"),
        dir.join("cache"),
    );
    // Android's app folder is group-writable (drwxrwx--x), which Arti's
    // permission check refuses; the app sandbox keeps others out instead.
    #[cfg(target_os = "android")]
    config.storage().permissions().dangerously_trust_everyone();
    // Bridges only (ADR-015): with them set, Arti enters the network through
    // nothing else.
    for line in &bridges.lines {
        config.bridges().bridges().push(line.trim().parse().ok()?);
    }
    if let Some(program) = &bridges.program
        && !bridges.lines.is_empty()
    {
        // obfs4proxy, lyrebird's predecessor, knows obfs4 alone.
        let old = program
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("obfs4proxy"));
        let names = if old {
            &TRANSPORTS[..1]
        } else {
            &TRANSPORTS[..]
        };
        let mut pt = arti_client::config::pt::TransportConfigBuilder::default();
        pt.protocols(names.iter().filter_map(|t| t.parse().ok()).collect())
            .path(arti_client::config::CfgPath::new_literal(program))
            .run_on_startup(false);
        config.bridges().transports().push(pt);
    }
    let config = config.build().ok()?;
    let client = arti_client::TorClient::create_bootstrapped(config)
        .await
        .ok()?;
    *tor = Some((dir.to_owned(), bridges.clone(), client.clone()));
    Some(client)
}

fn queue(frame: &ClientFrame) -> Option<QueueId> {
    match frame {
        ClientFrame::Send { send_id, .. } => Some(*send_id),
        _ => None,
    }
}

/// Never, if there is nothing to wait for.
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

/// 50-100 % of `delay`, so clients do not reconnect in lockstep.
fn jitter(delay: Duration) -> Duration {
    let r = f64::from(getrandom::u32().unwrap_or(u32::MAX)) / f64::from(u32::MAX);
    delay.mul_f64(0.5 + r / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn invalidated_group_frames_are_not_sent_or_retried() {
        use termchat_proto::Envelope;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (relay, mut events) = Relay::connect(
            &format!("ws://{}/", listener.local_addr().unwrap()),
            Dial::Direct,
        )
        .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(server).await.unwrap();
        let next = |frame: Message| ClientFrame::decode(&frame.into_data()).unwrap();
        assert!(matches!(
            next(ws.next().await.unwrap().unwrap()),
            ClientFrame::Hello { .. }
        ));
        assert!(matches!(events.recv().await, Some(Event::Connected)));
        let send = |client_id| ClientFrame::Send {
            client_id,
            send_id: [7; 16],
            envelope: Envelope {
                version: VERSION,
                olm_type: 0,
                body: vec![1],
            },
        };
        let session = Arc::new(AtomicBool::new(true));
        relay.send_scoped(send(1), Some(session.clone())).unwrap();
        assert!(matches!(
            next(ws.next().await.unwrap().unwrap()),
            ClientFrame::Send { client_id: 1, .. }
        ));
        relay.send_scoped(send(2), Some(session.clone())).unwrap();
        session.store(false, Ordering::Release);
        let retry = ServerFrame::Error {
            client_id: Some(1),
            code: ErrorCode::RateLimited,
        };
        ws.send(Message::Binary(retry.encode().into()))
            .await
            .unwrap();
        relay.send(send(3)).unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .unwrap();
        assert!(matches!(
            next(frame.unwrap().unwrap()),
            ClientFrame::Send { client_id: 3, .. }
        ));
        let accepted = ServerFrame::Accepted { client_id: 3 };
        ws.send(Message::Binary(accepted.encode().into()))
            .await
            .unwrap();
        assert!(matches!(
            events.recv().await,
            Some(Event::Frame(ServerFrame::Accepted { client_id: 3 }))
        ));
        assert_eq!(relay.unanswered(), 0);
    }

    #[test]
    fn versions_are_three_plain_numbers() {
        assert_eq!(parse_version("0.1.11"), Some((0, 1, 11)));
        assert!(parse_version("0.1.11") > parse_version("0.1.9"));
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "+1.2.3",
            "1.2.3-rc1",
            " 1.2.3",
        ] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }
}

#[cfg(all(test, feature = "tor"))]
mod bridge_tests {
    use super::*;

    #[test]
    fn bridge_lines_are_checked_by_arti() {
        // Made up (from Arti's documentation): the format is all that matters.
        let obfs4 = "obfs4 192.0.2.55:38114 316E643333645F6D79216558614D3931657A5F5F cert=YXJlIGZyZXF1ZW50bHkgZnVsbCBvZiBsaXR0bGUgbWVzc2FnZXMgeW91IGNhbiBmaW5kLg iat-mode=0";
        assert_eq!(bridge_transport(obfs4).as_deref(), Some("obfs4"));
        assert_eq!(
            bridge_transport(&format!("Bridge {obfs4}")).as_deref(),
            Some("obfs4")
        );
        // A plain bridge hides nothing; meek is left out (ADR-015, section 3).
        assert_eq!(
            bridge_transport("192.0.2.55:443 316E643333645F6D79216558614D3931657A5F5F"),
            None
        );
        assert_eq!(
            bridge_transport(&obfs4.replacen("obfs4", "meek_lite", 1)),
            None
        );
        assert_eq!(bridge_transport("obfs4 not-an-address"), None);
        assert!(
            !format!(
                "{:?}",
                Bridges {
                    lines: vec![obfs4.into()],
                    program: None
                }
            )
            .contains("cert")
        );
    }
}
