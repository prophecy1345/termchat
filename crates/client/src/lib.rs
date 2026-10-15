//! Relay transport: authenticated WebSocket with reconnect.
//!
//! Knows nothing about encryption: frames in, frames out. Unacknowledged
//! `Send` frames are resent after every reconnect (protocol.md, section 13).

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use termchat_proto::{ClientFrame, Identity, ServerFrame, Signature, VERSION};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

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
    ) -> Result<(Self, mpsc::Receiver<Event>), Error> {
        let host = url
            .parse::<Uri>()
            .ok()
            .and_then(|u| u.host().map(str::to_owned))
            .ok_or(Error::BadUrl)?;
        // Ignore the error: a provider may already be installed by the app.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (tx, rx) = mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::channel(256);
        let link = Link {
            url: url.to_owned(),
            host,
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
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Link {
    url: String,
    host: String,
    identity: Identity,
    sign: Signer,
    events: mpsc::Sender<Event>,
    /// Unanswered `Send` frames by `client_id`.
    pending: BTreeMap<u64, ClientFrame>,
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
            tokio::time::sleep(jitter(delay)).await;
            delay = (delay * 2).min(MAX_DELAY);
        }
    }

    async fn session(&mut self, out: &mut mpsc::UnboundedReceiver<ClientFrame>) -> End {
        let Ok((mut ws, _)) = connect_async(self.url.as_str()).await else {
            return End::Lost(false);
        };
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

    async fn handshake(&self, ws: &mut Ws) -> Option<()> {
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

async fn write(ws: &mut Ws, frame: &ClientFrame) -> Option<()> {
    ws.send(Message::Binary(frame.encode().into())).await.ok()
}

/// Next frame; `None` on close, error, garbage or silence longer than `IDLE`.
/// Pings are answered by tungstenite itself while reading.
async fn read(ws: &mut Ws) -> Option<ServerFrame> {
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
