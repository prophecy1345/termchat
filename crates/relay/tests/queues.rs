//! The relay over a real WebSocket: queues, no logins, no senders (ADR-023).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use termchat_proto::{
    ClientFrame, Envelope, ErrorCode, QueueId, QueueToken, ServerFrame, SignedKey, VERSION,
};
use termchat_relay::{Config, Limits, Rate};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const WAIT: Duration = Duration::from_secs(5);

async fn start(limits: Limits) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let config = Config {
        trust_proxy: false,
        db: None,
        reports: None,
        limits,
    };
    tokio::spawn(termchat_relay::serve(listener, None, config));
    url
}

struct Ws(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Ws {
    async fn raw(url: &str) -> Self {
        Self(tokio_tungstenite::connect_async(url).await.unwrap().0)
    }

    async fn new(url: &str) -> Self {
        let mut ws = Self::raw(url).await;
        ws.send(ClientFrame::Hello { version: VERSION }).await;
        ws
    }

    async fn send(&mut self, f: ClientFrame) {
        self.0
            .send(Message::Binary(f.encode().into()))
            .await
            .unwrap();
    }

    async fn bytes(&mut self) -> Vec<u8> {
        loop {
            match tokio::time::timeout(WAIT, self.0.next()).await.unwrap() {
                Some(Ok(Message::Binary(b))) => return b.to_vec(),
                Some(Ok(_)) => {}
                other => panic!("connection ended: {other:?}"),
            }
        }
    }

    async fn recv(&mut self) -> ServerFrame {
        ServerFrame::decode(&self.bytes().await).unwrap()
    }

    /// Nothing arrives for a moment.
    async fn quiet(&mut self) {
        let next = tokio::time::timeout(Duration::from_millis(300), self.0.next()).await;
        assert!(next.is_err(), "unexpected {next:?}");
    }

    async fn create(&mut self) -> Queue {
        self.send(ClientFrame::CreateQueue).await;
        let ServerFrame::QueueCreated {
            recv_id,
            send_id,
            token,
        } = self.recv().await
        else {
            panic!("no queue");
        };
        assert_ne!(recv_id, send_id);
        Queue {
            recv_id,
            send_id,
            token,
        }
    }

    /// Subscribes and returns the stored envelopes, which come before the key count.
    async fn subscribe(&mut self, q: &Queue) -> Vec<(u64, Envelope)> {
        self.send(ClientFrame::Subscribe {
            recv_id: q.recv_id,
            token: q.token,
        })
        .await;
        assert_eq!(
            self.recv().await,
            ServerFrame::Subscribed { recv_id: q.recv_id }
        );
        let mut out = Vec::new();
        loop {
            match self.recv().await {
                ServerFrame::Deliver {
                    recv_id,
                    relay_id,
                    envelope,
                } => {
                    assert_eq!(recv_id, q.recv_id);
                    out.push((relay_id, envelope));
                }
                ServerFrame::KeyCount { recv_id, .. } => {
                    assert_eq!(recv_id, q.recv_id);
                    return out;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    async fn put(&mut self, client_id: u64, send_id: QueueId, n: u8) -> ServerFrame {
        self.send(ClientFrame::Send {
            client_id,
            send_id,
            envelope: env(n),
        })
        .await;
        self.recv().await
    }
}

#[derive(Clone, Copy)]
struct Queue {
    recv_id: QueueId,
    send_id: QueueId,
    token: QueueToken,
}

fn env(n: u8) -> Envelope {
    Envelope {
        version: VERSION,
        olm_type: 1,
        body: vec![n; 64],
    }
}

fn err(client_id: Option<u64>, code: ErrorCode) -> ServerFrame {
    ServerFrame::Error { client_id, code }
}

#[tokio::test]
async fn a_version_1_client_reads_unsupported_version() {
    let url = start(Limits::default()).await;
    let mut old = Ws::raw(&url).await;
    old.send(ClientFrame::Hello { version: 1 }).await;
    // Version 1's `ServerFrame::Error { client_id: None, code: UnsupportedVersion }`.
    assert_eq!(old.bytes().await, [7, 0, 0]);
}

#[tokio::test]
async fn anyone_with_the_send_id_writes_and_only_the_owner_reads() {
    let url = start(Limits::default()).await;
    let mut alice = Ws::new(&url).await;
    let q = alice.create().await;
    assert!(alice.subscribe(&q).await.is_empty());

    // Bob never logs in and never says who he is.
    let mut bob = Ws::new(&url).await;
    assert_eq!(
        bob.put(1, q.send_id, 5).await,
        ServerFrame::Accepted { client_id: 1 }
    );
    let ServerFrame::Deliver {
        recv_id, envelope, ..
    } = alice.recv().await
    else {
        panic!("no delivery");
    };
    assert_eq!((recv_id, envelope), (q.recv_id, env(5)));

    // The recv_id does not work as an address, nor does an unknown one.
    assert_eq!(
        bob.put(2, q.recv_id, 6).await,
        err(Some(2), ErrorCode::UnknownQueue)
    );
    // Neither does reading without the token.
    bob.send(ClientFrame::Subscribe {
        recv_id: q.recv_id,
        token: [0; 32],
    })
    .await;
    assert_eq!(bob.recv().await, err(None, ErrorCode::UnknownQueue));
    bob.send(ClientFrame::Subscribe {
        recv_id: [9; 16],
        token: q.token,
    })
    .await;
    assert_eq!(
        bob.recv().await,
        err(None, ErrorCode::UnknownQueue),
        "same answer for no queue"
    );
    alice.quiet().await;
}

#[tokio::test]
async fn the_queue_waits_until_acknowledged_by_its_reader() {
    let url = start(Limits::default()).await;
    let mut owner = Ws::new(&url).await;
    let q = owner.create().await;
    let mut writer = Ws::new(&url).await;
    for n in 1..=3 {
        writer.put(u64::from(n), q.send_id, n).await;
    }
    let stored = owner.subscribe(&q).await;
    assert_eq!(
        stored.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
        [env(1), env(2), env(3)]
    );

    // An ack from a connection that does not read the queue is ignored.
    writer
        .send(ClientFrame::Ack {
            recv_id: q.recv_id,
            relay_id: stored[0].0,
        })
        .await;
    owner
        .send(ClientFrame::Ack {
            recv_id: q.recv_id,
            relay_id: stored[1].0,
        })
        .await;
    drop(owner);
    let mut again = Ws::new(&url).await;
    let left: Vec<_> = again
        .subscribe(&q)
        .await
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert_eq!(left, [env(1), env(3)]);
}

#[tokio::test]
async fn contact_queue_keys_run_out_instead_of_falling_back() {
    let url = start(Limits::default()).await;
    let mut owner = Ws::new(&url).await;
    let q = owner.create().await;
    owner.subscribe(&q).await;
    let key = |n| SignedKey {
        key: [n; 32],
        signature: [n; 64],
    };
    owner
        .send(ClientFrame::PublishKeys {
            recv_id: q.recv_id,
            token: q.token,
            one_time: vec![key(1), key(2)],
            fallback: None,
        })
        .await;
    assert!(matches!(
        owner.recv().await,
        ServerFrame::KeyCount {
            one_time_left: 2,
            ..
        }
    ));

    let mut contact = Ws::new(&url).await;
    for n in [1, 2] {
        contact
            .send(ClientFrame::ClaimKey { send_id: q.send_id })
            .await;
        assert_eq!(
            contact.recv().await,
            ServerFrame::ClaimedKey {
                send_id: q.send_id,
                key: key(n)
            }
        );
        // The owner hears it, to top the keys up.
        assert!(matches!(owner.recv().await, ServerFrame::KeyCount { .. }));
    }
    contact
        .send(ClientFrame::ClaimKey { send_id: q.send_id })
        .await;
    assert_eq!(contact.recv().await, err(None, ErrorCode::NoKeys));

    // Someone without the token cannot publish keys into the queue.
    contact
        .send(ClientFrame::PublishKeys {
            recv_id: q.recv_id,
            token: [0; 32],
            one_time: vec![key(7)],
            fallback: Some(key(8)),
        })
        .await;
    assert_eq!(contact.recv().await, err(None, ErrorCode::UnknownQueue));
}

#[tokio::test]
async fn a_deleted_queue_takes_nothing_more() {
    let url = start(Limits::default()).await;
    let mut owner = Ws::new(&url).await;
    let q = owner.create().await;
    let mut blocked = Ws::new(&url).await;
    blocked.put(1, q.send_id, 1).await;

    owner
        .send(ClientFrame::DeleteQueue {
            recv_id: q.recv_id,
            token: [0; 32],
        })
        .await;
    assert_eq!(owner.recv().await, err(None, ErrorCode::UnknownQueue));
    owner
        .send(ClientFrame::DeleteQueue {
            recv_id: q.recv_id,
            token: q.token,
        })
        .await;
    assert_eq!(
        owner.recv().await,
        ServerFrame::QueueDeleted { recv_id: q.recv_id }
    );
    assert_eq!(
        blocked.put(2, q.send_id, 2).await,
        err(Some(2), ErrorCode::UnknownQueue)
    );
}

#[tokio::test]
async fn writes_are_limited_per_queue_whoever_writes() {
    let url = start(Limits {
        sends_per_queue: Rate {
            burst: 2.0,
            per_sec: 0.001,
        },
        ..Limits::default()
    })
    .await;
    let mut owner = Ws::new(&url).await;
    let q = owner.create().await;
    let other = owner.create().await;
    // Fresh connections do not get a fresh budget.
    for id in 1..=2 {
        let mut w = Ws::new(&url).await;
        assert_eq!(
            w.put(id, q.send_id, 1).await,
            ServerFrame::Accepted { client_id: id }
        );
    }
    let mut w = Ws::new(&url).await;
    assert_eq!(
        w.put(3, q.send_id, 1).await,
        err(Some(3), ErrorCode::RateLimited)
    );
    assert_eq!(
        w.put(4, other.send_id, 1).await,
        ServerFrame::Accepted { client_id: 4 },
        "other queues are not affected"
    );
}

#[tokio::test]
async fn queue_creation_is_limited_per_ip() {
    let url = start(Limits {
        queues_per_ip: Rate {
            burst: 2.0,
            per_sec: 0.001,
        },
        ..Limits::default()
    })
    .await;
    let mut a = Ws::new(&url).await;
    a.create().await;
    let mut b = Ws::new(&url).await;
    b.create().await;
    b.send(ClientFrame::CreateQueue).await;
    assert_eq!(b.recv().await, err(None, ErrorCode::RateLimited));
}

#[tokio::test]
async fn a_lagging_reader_is_cut_off_and_finds_everything_queued() {
    let url = start(Limits {
        sends_per_queue: Rate {
            burst: 1000.0,
            per_sec: 1000.0,
        },
        ..Limits::default()
    })
    .await;
    let mut owner = Ws::new(&url).await;
    let q = owner.create().await;
    owner.subscribe(&q).await;
    // The owner reads nothing while more than a channel's worth arrives.
    let mut w = Ws::new(&url).await;
    for id in 0..100 {
        w.put(id, q.send_id, 1).await;
    }
    let mut ended = false;
    for _ in 0..200 {
        match tokio::time::timeout(WAIT, owner.0.next()).await.unwrap() {
            Some(Ok(Message::Binary(_))) => {}
            _ => {
                ended = true;
                break;
            }
        }
    }
    assert!(ended, "the lagging connection was closed");
    let mut again = Ws::new(&url).await;
    assert_eq!(again.subscribe(&q).await.len(), 100);
}
