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

    /// No frame arrives for a moment. Pings are the relay's keepalive, not
    /// frames: its first one comes right after connecting.
    async fn quiet(&mut self) {
        let until = tokio::time::Instant::now() + Duration::from_millis(300);
        loop {
            match tokio::time::timeout_at(until, self.0.next()).await {
                Err(_) => return,
                Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
                next => panic!("unexpected {next:?}"),
            }
        }
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
    // The owner reads nothing while far more than its socket buffers and the
    // relay's channel can hold arrives: 400 envelopes of 16 KB, about 6 MB.
    // Small ones would all fit in the buffers and never fill the channel.
    const N: u64 = 400;
    let mut w = Ws::new(&url).await;
    for id in 0..N {
        w.send(ClientFrame::Send {
            client_id: id,
            send_id: q.send_id,
            envelope: Envelope {
                body: vec![1; 16_000],
                ..env(1)
            },
        })
        .await;
        assert_eq!(w.recv().await, ServerFrame::Accepted { client_id: id });
    }
    let (mut ended, mut seen) = (false, 0);
    for _ in 0..N + 100 {
        match tokio::time::timeout(WAIT, owner.0.next()).await.unwrap() {
            Some(Ok(Message::Binary(_))) => seen += 1,
            _ => {
                ended = true;
                break;
            }
        }
    }
    assert!(ended, "the lagging connection was closed");
    assert!(seen < N, "cut off before everything came: {seen} of {N}");
    let mut again = Ws::new(&url).await;
    assert_eq!(again.subscribe(&q).await.len(), N as usize);
}

// ADR-027: a group queue hands each message to every reader's own queue.

struct Group {
    id: QueueId,
    send_id: QueueId,
    token: QueueToken,
}

impl Ws {
    async fn group(&mut self) -> Group {
        self.send(ClientFrame::CreateGroup).await;
        let ServerFrame::GroupCreated {
            group_id,
            send_id,
            token,
        } = self.recv().await
        else {
            panic!("no group");
        };
        Group {
            id: group_id,
            send_id,
            token,
        }
    }

    async fn reader(&mut self, g: &Group) -> Queue {
        self.send(ClientFrame::AddReader {
            group_id: g.id,
            token: g.token,
        })
        .await;
        let ServerFrame::ReaderAdded {
            group_id,
            recv_id,
            send_id,
            token,
        } = self.recv().await
        else {
            panic!("no reader");
        };
        assert_eq!(group_id, g.id);
        Queue {
            recv_id,
            send_id,
            token,
        }
    }
}

#[tokio::test]
async fn a_group_message_reaches_every_reader_until_each_acknowledges() {
    let url = start(Limits::default()).await;
    let mut admin = Ws::new(&url).await;
    let g = admin.group().await;
    let (r1, r2) = (admin.reader(&g).await, admin.reader(&g).await);

    let mut member = Ws::new(&url).await;
    assert_eq!(
        member.put(1, g.send_id, 7).await,
        ServerFrame::Accepted { client_id: 1 }
    );

    let mut one = Ws::new(&url).await;
    let got = one.subscribe(&r1).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, env(7));
    one.send(ClientFrame::Ack {
        recv_id: r1.recv_id,
        relay_id: got[0].0,
    })
    .await;
    one.quiet().await;
    // Acknowledged by the first reader, still there for the second.
    let mut two = Ws::new(&url).await;
    assert_eq!(two.subscribe(&r2).await.len(), 1);
    drop(one);
    let mut again = Ws::new(&url).await;
    assert!(
        again.subscribe(&r1).await.is_empty(),
        "acked once is gone for that reader only"
    );

    // A reader's own send_id reaches that reader alone.
    assert_eq!(
        member.put(2, r2.send_id, 9).await,
        ServerFrame::Accepted { client_id: 2 }
    );
    let ServerFrame::Deliver { envelope, .. } = two.recv().await else {
        panic!("no direct message");
    };
    assert_eq!(envelope, env(9));
    again.quiet().await;
}

#[tokio::test]
async fn only_the_admin_changes_readers_and_a_removed_reader_hears_nothing() {
    let url = start(Limits::default()).await;
    let mut admin = Ws::new(&url).await;
    let g = admin.group().await;
    let r = admin.reader(&g).await;

    // A reader's token, or any other, does not open the group.
    let mut other = Ws::new(&url).await;
    other
        .send(ClientFrame::AddReader {
            group_id: g.id,
            token: r.token,
        })
        .await;
    assert_eq!(other.recv().await, err(None, ErrorCode::UnknownQueue));
    other
        .send(ClientFrame::RemoveReader {
            group_id: g.id,
            token: [0; 32],
            recv_id: r.recv_id,
        })
        .await;
    assert_eq!(other.recv().await, err(None, ErrorCode::UnknownQueue));

    admin
        .send(ClientFrame::RemoveReader {
            group_id: g.id,
            token: g.token,
            recv_id: r.recv_id,
        })
        .await;
    assert_eq!(
        admin.recv().await,
        ServerFrame::ReaderRemoved { recv_id: r.recv_id }
    );
    // Its queue is gone: subscribing gets the unknown-queue answer.
    let mut gone = Ws::new(&url).await;
    gone.send(ClientFrame::Subscribe {
        recv_id: r.recv_id,
        token: r.token,
    })
    .await;
    assert_eq!(gone.recv().await, err(None, ErrorCode::UnknownQueue));

    admin
        .send(ClientFrame::DeleteGroup {
            group_id: g.id,
            token: g.token,
        })
        .await;
    assert_eq!(
        admin.recv().await,
        ServerFrame::GroupDeleted { group_id: g.id }
    );
    assert_eq!(
        other.put(3, g.send_id, 1).await,
        err(Some(3), ErrorCode::UnknownQueue)
    );
}

#[tokio::test]
async fn a_group_holds_at_most_32_readers() {
    let limits = Limits {
        queues_per_ip: Rate {
            burst: 100.0,
            per_sec: 1.0,
        },
        ..Limits::default()
    };
    let url = start(limits).await;
    let mut admin = Ws::new(&url).await;
    let g = admin.group().await;
    for _ in 0..termchat_proto::MAX_GROUP_MEMBERS {
        admin.reader(&g).await;
    }
    admin
        .send(ClientFrame::AddReader {
            group_id: g.id,
            token: g.token,
        })
        .await;
    assert_eq!(admin.recv().await, err(None, ErrorCode::QueueFull));
}
