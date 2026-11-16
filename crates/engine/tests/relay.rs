//! The transport against a real relay on localhost: queues, reconnects,
//! wake-ups, retries (ADR-012, ADR-020, ADR-023).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::sync::{Arc, Mutex};
use std::time::Duration;

use termchat_core::Client;
use termchat_core::proto::{Body, ClientFrame, ErrorCode, QueueId, QueueToken, ServerFrame};
use termchat_engine::{Event, Relay};
use termchat_relay::{Config, Limits, Rate};
use tokio::net::TcpListener;
use tokio::sync::mpsc::Receiver;

const WAIT: Duration = Duration::from_secs(5);

fn config(limits: Limits) -> Config {
    Config {
        trust_proxy: false,
        db: None,
        reports: None,
        limits,
    }
}

async fn start_relay(limits: Limits) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    tokio::spawn(termchat_relay::serve(listener, None, config(limits)));
    url
}

/// One side: a crypto state, a link, and the queue it reads.
struct Peer {
    core: Arc<Mutex<Client>>,
    relay: Relay,
    events: Receiver<Event>,
    queue: Option<(QueueId, QueueId, QueueToken)>,
}

impl Peer {
    fn new(url: &str, core: Arc<Mutex<Client>>) -> Self {
        let (relay, events) = Relay::connect(url, termchat_engine::Dial::Direct).unwrap();
        Self {
            core,
            relay,
            events,
            queue: None,
        }
    }

    /// The same state and queue over a new link.
    fn again(&self, url: &str) -> Self {
        Self {
            queue: self.queue,
            ..Self::new(url, Arc::clone(&self.core))
        }
    }

    fn key(&self) -> [u8; 32] {
        self.core.lock().unwrap().identity().ed25519
    }

    fn send_id(&self) -> QueueId {
        self.queue.unwrap().1
    }

    async fn next(&mut self) -> Event {
        tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("timed out")
            .expect("closed")
    }

    /// Next server frame, skipping connection events, key counts and subscribe answers.
    async fn frame(&mut self) -> ServerFrame {
        loop {
            match self.next().await {
                Event::Frame(ServerFrame::KeyCount { .. } | ServerFrame::Subscribed { .. })
                | Event::Connected
                | Event::RelayVersion(_)
                | Event::Disconnected => {}
                Event::Frame(f) => return f,
            }
        }
    }

    async fn connected(&mut self) {
        while self.next().await != Event::Connected {}
    }

    async fn subscribe(&mut self) {
        let (recv_id, _, token) = self.queue.unwrap();
        self.relay
            .send(ClientFrame::Subscribe { recv_id, token })
            .unwrap();
    }

    /// Makes a queue, reads it, and puts one-time keys on it.
    async fn open_queue(&mut self) {
        self.relay.send(ClientFrame::CreateQueue).unwrap();
        let ServerFrame::QueueCreated {
            recv_id,
            send_id,
            token,
        } = self.frame().await
        else {
            panic!("no queue");
        };
        self.queue = Some((recv_id, send_id, token));
        self.subscribe().await;
        let keys = self.core.lock().unwrap().prekeys(0, true);
        self.relay
            .send(ClientFrame::PublishKeys {
                recv_id,
                token,
                one_time: keys.one_time,
                fallback: keys.fallback,
            })
            .unwrap();
        while !matches!(
            self.next().await,
            Event::Frame(ServerFrame::KeyCount {
                one_time_left: 50,
                ..
            })
        ) {}
    }

    async fn start_session(&mut self, with: [u8; 32], send_id: QueueId) {
        self.relay.send(ClientFrame::ClaimKey { send_id }).unwrap();
        // Deliveries may arrive first; they are not what we wait for.
        let key = loop {
            match self.frame().await {
                ServerFrame::ClaimedKey { key, .. } => break key,
                ServerFrame::Deliver { .. } => {}
                other => panic!("no key: {other:?}"),
            }
        };
        self.core
            .lock()
            .unwrap()
            .start_session(&with, &key)
            .unwrap();
    }

    fn put(&self, to: [u8; 32], send_id: QueueId, id: u64, text: &str) {
        let envelope = self.core.lock().unwrap().encrypt(&to, body(text)).unwrap();
        self.relay
            .send(ClientFrame::Send {
                client_id: id,
                send_id,
                envelope,
            })
            .unwrap();
    }

    async fn send_text(&mut self, to: [u8; 32], send_id: QueueId, id: u64, text: &str) {
        self.put(to, send_id, id, text);
        loop {
            match self.frame().await {
                ServerFrame::Accepted { client_id } if client_id == id => break,
                ServerFrame::Deliver { .. } => {}
                other => panic!("not accepted: {other:?}"),
            }
        }
    }

    /// Receives, decrypts and (optionally) acknowledges one message from `from`.
    async fn recv_text(&mut self, from: [u8; 32], ack: bool) -> String {
        let ServerFrame::Deliver {
            recv_id,
            relay_id,
            envelope,
        } = self.frame().await
        else {
            panic!("no delivery")
        };
        assert_eq!(Some(recv_id), self.queue.map(|q| q.0));
        assert!(
            !envelope.body.windows(6).any(|w| w == b"secret"),
            "relay sees plaintext"
        );
        let got = self
            .core
            .lock()
            .unwrap()
            .decrypt(Some(&from), &envelope)
            .unwrap();
        if ack {
            self.relay
                .send(ClientFrame::Ack { recv_id, relay_id })
                .unwrap();
        }
        let Body::Text { text, .. } = got.body else {
            panic!("not text")
        };
        text
    }
}

fn body(t: &str) -> Body {
    Body::Text {
        sent_at: 0,
        expires_in: None,
        text: t.into(),
    }
}

/// Alice and Bob, each knowing the other's identity (as if from an invite).
fn pair() -> (Arc<Mutex<Client>>, Arc<Mutex<Client>>) {
    let (mut a, mut b) = (Client::default(), Client::default());
    a.add_contact(&b.identity()).unwrap();
    b.add_contact(&a.identity()).unwrap();
    (Arc::new(Mutex::new(a)), Arc::new(Mutex::new(b)))
}

/// Bob reads a queue with keys on it; Alice has a session with him.
async fn talking(url: &str, bob_url: &str) -> (Peer, Peer) {
    let (a, b) = pair();
    let mut bob = Peer::new(bob_url, b);
    bob.connected().await;
    bob.open_queue().await;
    let mut alice = Peer::new(url, a);
    alice.connected().await;
    alice.start_session(bob.key(), bob.send_id()).await;
    (alice, bob)
}

#[tokio::test]
async fn online_offline_and_redelivery() {
    let url = start_relay(Limits::default()).await;
    let (mut alice, mut bob) = talking(&url, &url).await;
    let (a, b, to) = (alice.key(), bob.key(), bob.send_id());

    alice.send_text(b, to, 1, "secret online").await;
    assert_eq!(bob.recv_text(a, true).await, "secret online");

    // Not acknowledged: it comes again once Bob reads the queue again.
    alice.send_text(b, to, 2, "secret twice").await;
    assert_eq!(bob.recv_text(a, false).await, "secret twice");
    let mut bob = bob.again(&url);
    bob.connected().await;
    bob.subscribe().await;
    let ServerFrame::Deliver {
        recv_id,
        relay_id,
        envelope,
    } = bob.frame().await
    else {
        panic!("not delivered again");
    };
    // The same envelope: Olm refuses it as a duplicate. Now it is acknowledged.
    assert!(
        bob.core
            .lock()
            .unwrap()
            .decrypt(Some(&a), &envelope)
            .is_err()
    );
    bob.relay
        .send(ClientFrame::Ack { recv_id, relay_id })
        .unwrap();
    // The ack goes out before the link is dropped below.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Sent while Bob is away: waits in the queue, then live ones follow.
    let (core, queue) = (Arc::clone(&bob.core), bob.queue);
    drop(bob);
    alice.send_text(b, to, 3, "secret offline").await;
    let mut bob = Peer {
        queue,
        ..Peer::new(&url, core)
    };
    bob.connected().await;
    bob.subscribe().await;
    assert_eq!(bob.recv_text(a, true).await, "secret offline");
    alice.send_text(b, to, 4, "secret live").await;
    assert_eq!(bob.recv_text(a, true).await, "secret live");
}

#[tokio::test]
async fn reconnects_when_relay_comes_up_later() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let url = format!("ws://{addr}/");
    let (a, _) = pair();
    let mut alice = Peer::new(&url, a);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let listener = TcpListener::bind(addr).await.unwrap();
    tokio::spawn(termchat_relay::serve(
        listener,
        None,
        config(Limits::default()),
    ));
    alice.connected().await;
}

#[tokio::test]
async fn a_wake_up_cuts_the_reconnect_wait_short() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let url = format!("ws://{addr}/");
    let (a, _) = pair();
    let mut alice = Peer::new(&url, a);

    // Four failures in: the link now waits 4-8 s (plus its spread) before the next try.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let listener = TcpListener::bind(addr).await.unwrap();
    tokio::spawn(termchat_relay::serve(
        listener,
        None,
        config(Limits::default()),
    ));
    alice.relay.reconnect();
    let connected = async { while alice.next().await != Event::Connected {} };
    tokio::time::timeout(Duration::from_secs(1), connected)
        .await
        .expect("still waiting after the wake-up");
}

/// A proxy to the relay whose `cut` drops the relay's side of every
/// connection so far but keeps the client's socket open and silent: the
/// half-open link a phone keeps after its sleep.
async fn half_open_proxy(relay: &str) -> (String, tokio::sync::watch::Sender<u32>) {
    let upstream = relay
        .trim_start_matches("ws://")
        .trim_end_matches('/')
        .to_owned();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let (cut, cuts) = tokio::sync::watch::channel(0u32);
    tokio::spawn(async move {
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut cuts = cuts.clone();
            cuts.mark_unchanged();
            let upstream = upstream.clone();
            tokio::spawn(async move {
                let mut relay = tokio::net::TcpStream::connect(upstream).await.unwrap();
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut client, &mut relay) => {}
                    _ = cuts.changed() => {
                        drop(relay);
                        std::future::pending::<()>().await;
                    }
                }
            });
        }
    });
    (url, cut)
}

#[tokio::test]
async fn a_push_redials_a_link_whose_ping_goes_unanswered() {
    let url = start_relay(Limits::default()).await;
    let (proxy, cut) = half_open_proxy(&url).await;
    let (mut alice, mut bob) = talking(&url, &proxy).await;
    let (a, b, to) = (alice.key(), bob.key(), bob.send_id());
    // Alice took one of Bob's keys; the relay tells him before the cut.
    while !matches!(
        bob.next().await,
        Event::Frame(ServerFrame::KeyCount {
            one_time_left: 49,
            ..
        })
    ) {}

    // The relay lets Bob go; Bob's socket stays open and heard a frame just now.
    cut.send_modify(|n| *n += 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    alice.send_text(b, to, 1, "secret queued").await;

    // Coming on screen trusts a link heard within 35 s: nothing arrives.
    bob.relay.reconnect();
    let early = tokio::time::timeout(Duration::from_secs(1), bob.events.recv()).await;
    assert!(
        early.is_err(),
        "the on-screen check dropped a fresh-looking link: {early:?}"
    );

    // A push: Bob pings; nothing answers within 5 s, so he dials again.
    bob.relay.check();
    let dropped = async { while bob.events.recv().await != Some(Event::Disconnected) {} };
    tokio::time::timeout(Duration::from_secs(10), dropped)
        .await
        .expect("the unanswered link was kept");
    bob.connected().await;
    bob.subscribe().await;
    assert_eq!(bob.recv_text(a, true).await, "secret queued");
}

/// ADR-012, the sleeping link: a push for a link that still answers reads
/// what came and keeps the link; a new connection would cost a Tor circuit.
#[tokio::test]
async fn a_push_keeps_a_link_that_answers() {
    let url = start_relay(Limits::default()).await;
    let (mut alice, mut bob) = talking(&url, &url).await;
    let (a, b, to) = (alice.key(), bob.key(), bob.send_id());
    alice.send_text(b, to, 1, "secret one").await;
    assert_eq!(bob.recv_text(a, true).await, "secret one");

    bob.relay.check();
    let mut seen = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout(Duration::from_secs(7), bob.events.recv()).await {
        seen.push(e);
    }
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, Event::Disconnected | Event::Connected)),
        "a push dropped a live link: {seen:?}"
    );
}

#[tokio::test]
async fn refused_sends_are_retried_in_order_and_an_unknown_queue_fails() {
    let url = start_relay(Limits {
        sends_per_queue: Rate {
            burst: 2.0,
            per_sec: 1.0,
        },
        ..Limits::default()
    })
    .await;
    let (mut alice, mut bob) = talking(&url, &url).await;
    let (a, b, to) = (alice.key(), bob.key(), bob.send_id());

    // ADR-020: past the burst the relay refuses; the link sends the same
    // envelopes again, one at a time, and the app only sees them accepted.
    for id in 1..=5 {
        alice.put(b, to, id, &id.to_string());
    }
    for id in 1..=5 {
        assert_eq!(alice.frame().await, ServerFrame::Accepted { client_id: id });
    }
    for id in 1..=5 {
        assert_eq!(bob.recv_text(a, true).await, id.to_string());
    }

    // Nobody made this queue.
    alice
        .relay
        .send(ClientFrame::ClaimKey { send_id: [9; 16] })
        .unwrap();
    assert_eq!(
        alice.frame().await,
        ServerFrame::Error {
            client_id: None,
            code: ErrorCode::UnknownQueue
        }
    );
}

#[tokio::test]
async fn relay_says_its_version_before_connected() {
    let url = start_relay(Limits::default()).await;
    let (a, _) = pair();
    let mut alice = Peer::new(&url, a);
    let version = env!("CARGO_PKG_VERSION").to_string();
    assert_eq!(alice.next().await, Event::RelayVersion(version));
    assert_eq!(alice.next().await, Event::Connected);
}

#[tokio::test]
async fn transport_refuses_plain_ws_across_a_network() {
    let connect = |url: &str| Relay::connect(url, termchat_engine::Dial::Direct).is_ok();
    assert!(!connect("ws://relay.example.org/"));
    assert!(!connect("http://relay.example.org/"));
    assert!(connect("wss://relay.example.org/"));
    assert!(connect("ws://127.0.0.1:1/"));
    // ADR-004: an onion host directly would go through plain DNS.
    for onion in ["ws", "wss"] {
        let url =
            format!("{onion}://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuv.onion/");
        assert!(!connect(&url), "{url}");
    }
}

#[tokio::test]
async fn a_long_queue_arrives_once_and_in_order() {
    // More than the relay reads from its queue at a time (32).
    const N: u64 = 80;
    let url = start_relay(Limits {
        sends_per_queue: Rate::per(1000.0, 1.0),
        ..Limits::default()
    })
    .await;
    let (mut alice, bob) = talking(&url, &url).await;
    let (a, b, to) = (alice.key(), bob.key(), bob.send_id());
    let (core, queue) = (Arc::clone(&bob.core), bob.queue);
    drop(bob);
    for id in 0..N {
        alice.send_text(b, to, id, &id.to_string()).await;
    }
    let mut bob = Peer {
        queue,
        ..Peer::new(&url, core)
    };
    bob.connected().await;
    bob.subscribe().await;
    for id in 0..N {
        assert_eq!(bob.recv_text(a, true).await, id.to_string());
    }
    // Live again after the queue, and nothing from it comes twice.
    alice.send_text(b, to, N, "live").await;
    assert_eq!(bob.recv_text(a, true).await, "live");
}
