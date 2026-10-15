//! Relay transport: authenticated WebSocket with reconnect.
//!
//! Knows nothing about encryption: frames in, frames out. Unacknowledged
//! `Send` frames are resent after every reconnect (protocol.md, section 13).

pub mod app;
pub mod data;
pub mod form;
pub mod password;
pub mod ui;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use termchat_proto::{ClientFrame, Identity, ServerFrame, Signature, VERSION};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::{WebSocketStream, connect_async};

const MIN_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(60);
/// The relay pings every 30 s; silence this long means the link is dead.
const IDLE: Duration = Duration::from_secs(90);

/// Signs the relay's challenge: `(relay_host, nonce) -> signature`.
pub type Signer = Box<dyn Fn(&str, &[u8; 32]) -> Signature + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Authenticated. Publish keys and claim what you need again.
    Connected,
    Disconnected,
    Frame(ServerFrame),
}

/// How to reach the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dial {
    Direct,
    /// Only through Tor, with Arti's state in this folder (ADR-004). Never falls
    /// back to a direct connection, so the IP cannot leak.
    Tor(PathBuf),
}

/// Handle to the background connection. Dropping it closes the connection.
#[derive(Clone)]
pub struct Relay {
    tx: mpsc::UnboundedSender<ClientFrame>,
}

impl Relay {
    /// Starts connecting in the background. `url` is `wss://host/` (or `ws://` for tests).
    pub fn connect(
        url: &str,
        identity: Identity,
        sign: Signer,
        dial: Dial,
    ) -> Result<(Self, mpsc::Receiver<Event>), Error> {
        let uri = url.parse::<Uri>().map_err(|_| Error::BadUrl)?;
        let host = uri.host().map(str::to_owned).ok_or(Error::BadUrl)?;
        #[cfg(feature = "tor")]
        let port = uri
            .port_u16()
            .unwrap_or(if uri.scheme_str() == Some("wss") {
                443
            } else {
                80
            });
        #[cfg(not(feature = "tor"))]
        if matches!(dial, Dial::Tor(_)) {
            return Err(Error::NoTor);
        }
        // Ignore the error: a provider may already be installed by the app.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (tx, rx) = mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::channel(256);
        let link = Link {
            url: url.to_owned(),
            host,
            #[cfg(feature = "tor")]
            port,
            dial,
            #[cfg(feature = "tor")]
            tor: None,
            identity,
            sign,
            events: events_tx,
            pending: BTreeMap::new(),
        };
        tokio::spawn(link.run(rx));
        Ok((Self { tx }, events))
    }

    /// Queues a frame. `Send` frames survive reconnects until the relay answers.
    pub fn send(&self, frame: ClientFrame) -> Result<(), Error> {
        self.tx.send(frame).map_err(|_| Error::Closed)
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
    host: String,
    #[cfg(feature = "tor")]
    port: u16,
    dial: Dial,
    /// Bootstrapped once, reused across reconnects.
    #[cfg(feature = "tor")]
    tor: Option<std::sync::Arc<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>>,
    identity: Identity,
    sign: Signer,
    events: mpsc::Sender<Event>,
    /// Unanswered `Send` frames by `client_id`.
    pending: BTreeMap<u64, ClientFrame>,
}

enum Ws {
    Direct(Box<WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>),
    #[cfg(feature = "tor")]
    Tor(Box<WebSocketStream<arti_client::DataStream>>),
}

enum End {
    /// The app dropped every `Relay` handle.
    Stop,
    /// Connection lost; `true` if it had authenticated.
    Lost(bool),
}

impl Link {
    async fn run(mut self, mut out: mpsc::UnboundedReceiver<ClientFrame>) {
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
            // Stop waiting as soon as the app is gone (e.g. locked): the signer
            // holds the keys and must not outlive it.
            tokio::select! {
                _ = tokio::time::sleep(jitter(delay)) => {}
                _ = self.events.closed() => return,
            }
            delay = (delay * 2).min(MAX_DELAY);
        }
    }

    /// Connecting can take long (TCP timeouts, Tor bootstrap). The app may lock
    /// meanwhile, and the signer must not keep the keys alive: give up on the
    /// connection attempt as soon as the event receiver is gone. Once connected,
    /// `talk` drains queued frames (acks) before it stops.
    async fn session(&mut self, out: &mut mpsc::UnboundedReceiver<ClientFrame>) -> End {
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
            Dial::Direct => connect_async(self.url.as_str())
                .await
                .ok()
                .map(|(ws, _)| Ws::Direct(Box::new(ws))),
            #[cfg(feature = "tor")]
            Dial::Tor(dir) => {
                let stream = self.tor_stream(&dir).await?;
                tokio_tungstenite::client_async(self.url.as_str(), stream)
                    .await
                    .ok()
                    .map(|(ws, _)| Ws::Tor(Box::new(ws)))
            }
            #[cfg(not(feature = "tor"))]
            Dial::Tor(_) => None,
        }
    }

    #[cfg(feature = "tor")]
    async fn tor_stream(&mut self, dir: &std::path::Path) -> Option<arti_client::DataStream> {
        if self.tor.is_none() {
            let config = arti_client::config::TorClientConfigBuilder::from_directories(
                dir.join("state"),
                dir.join("cache"),
            )
            .build()
            .ok()?;
            self.tor = Some(
                arti_client::TorClient::create_bootstrapped(config)
                    .await
                    .ok()?,
            );
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
        out: &mut mpsc::UnboundedReceiver<ClientFrame>,
    ) -> End {
        if self.handshake(&mut ws).await.is_none()
            || self.events.send(Event::Connected).await.is_err()
        {
            return End::Lost(false);
        }
        for frame in self.pending.values() {
            if write(&mut ws, frame).await.is_none() {
                return End::Lost(true);
            }
        }
        loop {
            tokio::select! {
                frame = out.recv() => {
                    let Some(frame) = frame else { return End::Stop };
                    if let ClientFrame::Send { client_id, .. } = &frame {
                        self.pending.insert(*client_id, frame.clone());
                    }
                    if write(&mut ws, &frame).await.is_none() {
                        return End::Lost(true);
                    }
                }
                frame = read(&mut ws) => {
                    let Some(frame) = frame else { return End::Lost(true) };
                    if let ServerFrame::Accepted { client_id } | ServerFrame::Error { client_id: Some(client_id), .. } = &frame {
                        self.pending.remove(client_id);
                    }
                    if self.events.send(Event::Frame(frame)).await.is_err() {
                        return End::Stop;
                    }
                }
            }
        }
    }

    async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        ws: &mut WebSocketStream<S>,
    ) -> Option<()> {
        write(ws, &ClientFrame::Hello { version: VERSION }).await?;
        let ServerFrame::Challenge { nonce } = read(ws).await? else {
            return None;
        };
        let signature = (self.sign)(&self.host, &nonce);
        write(
            ws,
            &ClientFrame::Auth {
                identity: self.identity.clone(),
                signature,
            },
        )
        .await?;
        match read(ws).await? {
            ServerFrame::AuthOk => Some(()),
            _ => None,
        }
    }
}

async fn write<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    frame: &ClientFrame,
) -> Option<()> {
    ws.send(Message::Binary(frame.encode().into())).await.ok()
}

/// Next frame; `None` on close, error, garbage or silence longer than `IDLE`.
/// Pings are answered by tungstenite itself while reading.
async fn read<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
) -> Option<ServerFrame> {
    loop {
        match tokio::time::timeout(IDLE, ws.next()).await.ok()?? {
            Ok(Message::Binary(bytes)) => return ServerFrame::decode(&bytes).ok(),
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            _ => return None,
        }
    }
}

/// 50-100 % of `delay`, so clients do not reconnect in lockstep.
fn jitter(delay: Duration) -> Duration {
    let r = f64::from(getrandom::u32().unwrap_or(u32::MAX)) / f64::from(u32::MAX);
    delay.mul_f64(0.5 + r / 2.0)
}
