//! Two clients talking through a real relay on localhost (PLAN.md, M2).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use termchat::{Event, Relay};
use termchat_core::Client;
use termchat_core::proto::{Body, ClientFrame, ErrorCode, ServerFrame, VERSION};
use termchat_relay::{Config, Limits, Rate};
use tokio::net::TcpListener;
use tokio::sync::mpsc::Receiver;
use tokio_tungstenite::tungstenite::Message;

const WAIT: Duration = Duration::from_secs(5);

async fn start_relay(limits: Limits) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let config = Config {
        hosts: vec!["127.0.0.1".into()],
        trust_proxy: false,
        db: None,
        reports: None,
        limits,
    };
    tokio::spawn(termchat_relay::serve(listener, None, config));
    url
}

struct Peer {
    core: Arc<Mutex<Client>>,
    relay: Relay,
    events: Receiver<Event>,
}

impl Peer {
    fn new(url: &str, core: Arc<Mutex<Client>>) -> Self {
        let identity = core.lock().unwrap().identity();
        let signer = Arc::clone(&core);
        let sign = Box::new(move |host: &str, nonce: &[u8; 32]| {
            signer.lock().unwrap().auth_signature(host, nonce)
        });
        let (relay, events) = Relay::connect(url, identity, sign, termchat::Dial::Direct).unwrap();
        Self {
            core,
            relay,
            events,
        }
    }

    fn key(&self) -> [u8; 32] {
        self.core.lock().unwrap().identity().ed25519
    }

    async fn next(&mut self) -> Event {
        tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("timed out")
            .expect("closed")
    }

    /// Next server frame, skipping connection events and key counts.
    async fn frame(&mut self) -> ServerFrame {
        loop {
            match self.next().await {
                Event::Frame(ServerFrame::KeyCount { .. })
                | Event::Connected
                | Event::Disconnected => {}
                Event::Frame(f) => return f,
            }
        }
    }

    async fn connected(&mut self) {
        while self.next().await != Event::Connected {}
    }

    async fn publish_keys(&mut self) {
        let keys = self.core.lock().unwrap().prekeys(0, true);
        self.relay
            .send(ClientFrame::PublishKeys {
                one_time: keys.one_time,
                fallback: keys.fallback,
            })
            .unwrap();
        loop {
            // The first count arrives right after AuthOk, before our keys.
            if let Event::Frame(ServerFrame::KeyCount {
                one_time_left: 50, ..
            }) = self.next().await
            {
                return;
            }
        }
    }

    async fn start_session(&mut self, with: [u8; 32]) {
        self.relay
            .send(ClientFrame::ClaimKey { target: with })
            .unwrap();
        let ServerFrame::ClaimedKey { target, key } = self.frame().await else {
            panic!("no key")
        };
        assert_eq!(target, with);
        self.core
            .lock()
            .unwrap()
            .start_session(&with, &key)
            .unwrap();
    }

    async fn send_text(&mut self, to: [u8; 32], id: u64, text: &str) {
        let body = Body::Text {
            sent_at: 0,
            expires_in: None,
            text: text.into(),
        };
        let envelope = self.core.lock().unwrap().encrypt(&to, body).unwrap();
        self.relay
            .send(ClientFrame::Send {
                client_id: id,
                envelope,
            })
            .unwrap();
        assert_eq!(self.frame().await, ServerFrame::Accepted { client_id: id });
    }

    /// Receives, decrypts and (optionally) acknowledges one message.
    async fn recv_text(&mut self, ack: bool) -> String {
        let ServerFrame::Deliver {
            relay_id,
            sender,
            envelope,
        } = self.frame().await
        else {
            panic!("no delivery")
        };
        assert!(
            !envelope.body.windows(6).any(|w| w == b"secret"),
            "relay sees plaintext"
        );
        let got = self
            .core
            .lock()
            .unwrap()
            .decrypt(&sender, &envelope)
            .unwrap();
        if ack {
            self.relay.send(ClientFrame::Ack { relay_id }).unwrap();
        }
        let Body::Text { text, .. } = got.body else {
            panic!("not text")
        };
        text
    }
}

/// Alice and Bob, each knowing the other's identity (as if from an invite).
fn pair() -> (Arc<Mutex<Client>>, Arc<Mutex<Client>>) {
    let (mut a, mut b) = (Client::default(), Client::default());
    a.add_contact(&b.identity()).unwrap();
    b.add_contact(&a.identity()).unwrap();
    (Arc::new(Mutex::new(a)), Arc::new(Mutex::new(b)))
}

#[tokio::test]
async fn online_offline_and_redelivery() {
    let url = start_relay(Limits::default()).await;
    let (a, b) = pair();
    let mut bob = Peer::new(&url, Arc::clone(&b));
    bob.connected().await;
    bob.publish_keys().await;
    let mut alice = Peer::new(&url, a);
    alice.connected().await;
    let bob_key = bob.key();
    alice.start_session(bob_key).await;

    // Online.
    alice.send_text(bob_key, 1, "secret one").await;
    assert_eq!(bob.recv_text(true).await, "secret one");

    // Offline: Bob is gone, Alice keeps writing, Bob gets everything on return.
    drop(bob);
    alice.send_text(bob_key, 2, "secret two").await;
    alice.send_text(bob_key, 3, "secret three").await;
    let mut bob = Peer::new(&url, Arc::clone(&b));
    assert_eq!(bob.recv_text(false).await, "secret two");
    assert_eq!(bob.recv_text(true).await, "secret three");

    // "secret two" was not acknowledged: it comes again after a reconnect and
    // core rejects the replay instead of showing it twice.
    drop(bob);
    let mut bob = Peer::new(&url, b);
    let ServerFrame::Deliver {
        relay_id,
        sender,
        envelope,
    } = bob.frame().await
    else {
        panic!()
    };
    assert!(
        bob.core
            .lock()
            .unwrap()
            .decrypt(&sender, &envelope)
            .is_err()
    );
    bob.relay.send(ClientFrame::Ack { relay_id }).unwrap();
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
    let config = Config {
        hosts: vec!["127.0.0.1".into()],
        trust_proxy: false,
        db: None,
        reports: None,
        limits: Limits::default(),
    };
    tokio::spawn(termchat_relay::serve(listener, None, config));
    alice.connected().await;
}

/// Raw WebSocket client, to speak to the relay without the honest transport.
type Raw =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn raw(url: &str) -> Raw {
    tokio_tungstenite::connect_async(url).await.unwrap().0
}

async fn raw_send(ws: &mut Raw, f: ClientFrame) {
    ws.send(Message::Binary(f.encode().into())).await.unwrap();
}

async fn raw_recv(ws: &mut Raw) -> ServerFrame {
    loop {
        match tokio::time::timeout(WAIT, ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Binary(b) => return ServerFrame::decode(&b).unwrap(),
            _ => continue,
        }
    }
}

#[tokio::test]
async fn impersonation_and_wrong_host_are_rejected() {
    let url = start_relay(Limits::default()).await;
    let alice = Client::default();
    let mallory = Client::default();

    for (signer, host) in [(&mallory, "127.0.0.1"), (&alice, "other.relay")] {
        let mut ws = raw(&url).await;
        raw_send(&mut ws, ClientFrame::Hello { version: VERSION }).await;
        let ServerFrame::Challenge { nonce } = raw_recv(&mut ws).await else {
            panic!()
        };
        let signature = signer.auth_signature(host, &nonce);
        raw_send(
            &mut ws,
            ClientFrame::Auth {
                identity: alice.identity(),
                signature,
            },
        )
        .await;
        assert_eq!(
            raw_recv(&mut ws).await,
            ServerFrame::Error {
                client_id: None,
                code: ErrorCode::BadSignature
            }
        );
    }
}

#[tokio::test]
async fn silent_client_times_out() {
    let limits = Limits {
        auth_timeout: Duration::from_millis(200),
        ..Limits::default()
    };
    let url = start_relay(limits).await;
    let mut ws = raw(&url).await;
    assert_eq!(
        raw_recv(&mut ws).await,
        ServerFrame::Error {
            client_id: None,
            code: ErrorCode::ChallengeExpired
        }
    );
}

#[tokio::test]
async fn send_rate_limit_and_unknown_recipient() {
    let limits = Limits {
        sends_per_key: Rate {
            burst: 2.0,
            per_sec: 0.0,
        },
        ..Limits::default()
    };
    let url = start_relay(limits).await;
    let (a, b) = pair();
    let mut bob = Peer::new(&url, b);
    bob.connected().await;
    bob.publish_keys().await;
    let mut alice = Peer::new(&url, a);
    alice.connected().await;
    let bob_key = bob.key();
    alice.start_session(bob_key).await;

    alice.send_text(bob_key, 1, "a").await;
    alice.send_text(bob_key, 2, "b").await;
    let envelope = alice
        .core
        .lock()
        .unwrap()
        .encrypt(
            &bob_key,
            Body::ContactAccept {
                nickname: "alice".into(),
            },
        )
        .unwrap();
    alice
        .relay
        .send(ClientFrame::Send {
            client_id: 3,
            envelope: envelope.clone(),
        })
        .unwrap();
    assert_eq!(
        alice.frame().await,
        ServerFrame::Error {
            client_id: Some(3),
            code: ErrorCode::RateLimited
        }
    );

    // Nobody registered this key.
    alice
        .relay
        .send(ClientFrame::ClaimKey { target: [9; 32] })
        .unwrap();
    assert_eq!(
        alice.frame().await,
        ServerFrame::Error {
            client_id: None,
            code: ErrorCode::UnknownRecipient
        }
    );
}

#[tokio::test]
async fn per_target_claim_limit_hands_out_fallback() {
    let limits = Limits {
        claims_per_target: Rate {
            burst: 1.0,
            per_sec: 0.0,
        },
        ..Limits::default()
    };
    let url = start_relay(limits).await;
    let (a, b) = pair();
    let mut bob = Peer::new(&url, b);
    bob.connected().await;
    bob.publish_keys().await;
    let fallback = bob.core.lock().unwrap().prekeys(50, false).fallback;
    assert!(fallback.is_none(), "fallback was already published");

    let mut alice = Peer::new(&url, a);
    alice.connected().await;
    let bob_key = bob.key();
    let mut keys = Vec::new();
    for _ in 0..2 {
        alice
            .relay
            .send(ClientFrame::ClaimKey { target: bob_key })
            .unwrap();
        let ServerFrame::ClaimedKey { key, .. } = alice.frame().await else {
            panic!()
        };
        keys.push(key);
    }
    assert_ne!(keys[0], keys[1]);
    // Bob learns that his fallback key went out and should rotate it.
    loop {
        if let Event::Frame(ServerFrame::KeyCount {
            fallback_used: true,
            one_time_left,
        }) = bob.next().await
        {
            assert_eq!(one_time_left, 49);
            break;
        }
    }
}

#[tokio::test]
async fn three_real_recipients_reporting_ban_the_sender() {
    let url = start_relay(Limits::default()).await;
    let mallory_core = Arc::new(Mutex::new(Client::default()));
    let mut mallory = Peer::new(&url, Arc::clone(&mallory_core));
    mallory.connected().await;
    let mut victims = Vec::new();
    for _ in 0..3 {
        let core = Arc::new(Mutex::new(Client::default()));
        let mut v = Peer::new(&url, core);
        v.connected().await;
        v.publish_keys().await;
        let id = v.core.lock().unwrap().identity();
        mallory.core.lock().unwrap().add_contact(&id).unwrap();
        mallory.start_session(id.ed25519).await;
        victims.push(v);
    }
    for (i, v) in victims.iter().enumerate() {
        mallory
            .send_text(v.key(), i as u64, "buy something illegal")
            .await;
    }
    let report = |sender| ClientFrame::Report {
        sender,
        text: "buy something illegal".into(),
    };
    let m = mallory.key();

    // People Mallory never wrote to cannot count, however many there are.
    for _ in 0..5 {
        let mut stranger = Peer::new(&url, Arc::new(Mutex::new(Client::default())));
        stranger.connected().await;
        stranger.relay.send(report(m)).unwrap();
        assert_eq!(stranger.frame().await, ServerFrame::Reported);
    }
    for v in &mut victims[..2] {
        v.relay.send(report(m)).unwrap();
        while !matches!(v.frame().await, ServerFrame::Reported) {}
    }
    let mut ws = raw(&url).await;
    assert!(auth(&mut ws, &mallory_core).await == ServerFrame::AuthOk);

    // The third real recipient tips it over.
    victims[2].relay.send(report(m)).unwrap();
    while !matches!(victims[2].frame().await, ServerFrame::Reported) {}
    let mut ws = raw(&url).await;
    let refused = auth(&mut ws, &mallory_core).await;
    assert_eq!(
        refused,
        ServerFrame::Error {
            client_id: None,
            code: ErrorCode::Banned
        }
    );
}

async fn auth(ws: &mut Raw, client: &Mutex<Client>) -> ServerFrame {
    raw_send(ws, ClientFrame::Hello { version: VERSION }).await;
    let ServerFrame::Challenge { nonce } = raw_recv(ws).await else {
        panic!()
    };
    let (identity, signature) = {
        let c = client.lock().unwrap();
        (c.identity(), c.auth_signature("127.0.0.1", &nonce))
    };
    raw_send(
        ws,
        ClientFrame::Auth {
            identity,
            signature,
        },
    )
    .await;
    raw_recv(ws).await
}

#[tokio::test]
async fn queue_and_prekeys_survive_a_relay_restart() {
    let path = std::env::temp_dir().join(format!("termchat-restart-{}.db", std::process::id()));
    let start = |path: std::path::PathBuf| async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
        let config = Config {
            hosts: vec!["127.0.0.1".into()],
            trust_proxy: false,
            db: Some(path),
            reports: None,
            limits: Limits::default(),
        };
        (
            url,
            tokio::spawn(termchat_relay::serve(listener, None, config)),
        )
    };
    let (a, b) = pair();
    let (url, relay) = start(path.clone()).await;
    let mut bob = Peer::new(&url, Arc::clone(&b));
    bob.connected().await;
    bob.publish_keys().await;
    let bob_key = bob.key();
    drop(bob);
    let mut alice = Peer::new(&url, Arc::clone(&a));
    alice.connected().await;
    alice.start_session(bob_key).await;
    alice.send_text(bob_key, 1, "secret across restarts").await;
    drop(alice);

    relay.abort();
    let (url, _relay) = start(path.clone()).await;

    // Bob was offline the whole time: the envelope waited on disk.
    let mut bob = Peer::new(&url, b);
    assert_eq!(bob.recv_text(true).await, "secret across restarts");
    // Bob's prekeys were kept too: someone new can still start a session.
    let mut carol = Peer::new(&url, Arc::new(Mutex::new(Client::default())));
    carol.connected().await;
    carol
        .relay
        .send(ClientFrame::ClaimKey { target: bob_key })
        .unwrap();
    assert!(matches!(
        carol.frame().await,
        ServerFrame::ClaimedKey { .. }
    ));

    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
    }
}
