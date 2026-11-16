//! Two full engines meeting and talking through anonymous queues (ADR-023).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termchat_engine::data::Status;
use termchat_engine::engine::NEEDS_INVITE;
use termchat_engine::{Engine, Happened, Setup};
use termchat_relay::{Config, Limits};
use tokio::net::TcpListener;

const PASSWORD: &str = "correct horse battery staple violin";

async fn start_relay() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let config = Config {
        trust_proxy: false,
        db: None,
        reports: None,
        limits: Limits::default(),
    };
    tokio::spawn(termchat_relay::serve(listener, None, config));
    url
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("termchat-q-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn engine(dir: &Path, name: &str, url: &str) -> Box<Engine> {
    let setup = Setup {
        relay_url: url,
        onion_url: None,
        tor: false,
        nickname: name,
        password: PASSWORD,
    };
    Engine::setup(&dir.join(name), &setup).unwrap()
}

/// Feeds relay events and ticks both engines until `done` holds.
async fn pump(a: &mut Engine, b: &mut Engine, done: impl Fn(&Engine, &Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(a, b) {
        let dump = |e: &Engine| -> Vec<String> {
            e.data
                .contacts
                .iter()
                .map(|c| {
                    format!(
                        "mine={} theirs={:?} invite={:?} req={} offer={} accept={} msgs={:?}",
                        c.mine.is_some(),
                        c.theirs.as_ref().map(|a| a.send_id[0]),
                        c.their_invite.as_ref().map(|a| a.send_id[0]),
                        c.request_pending,
                        c.offer_pending,
                        c.accept_pending,
                        c.messages
                            .iter()
                            .map(|m| (&m.text, &m.status))
                            .collect::<Vec<_>>()
                    )
                })
                .collect()
        };
        assert!(
            Instant::now() < deadline,
            "timed out\na: {:?} {:?}\nb: {:?} {:?}",
            dump(a),
            a.happened,
            dump(b),
            b.happened
        );
        tokio::select! {
            Some(ev) = a.events.recv() => a.on_relay(ev),
            Some(ev) = b.events.recv() => b.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        a.tick();
        b.tick();
    }
}

fn texts(e: &Engine) -> Vec<String> {
    e.data.contacts[0]
        .messages
        .iter()
        .map(|m| m.text.clone())
        .collect()
}

fn got(text: &'static str) -> impl Fn(&Engine) -> bool {
    move |e| !e.data.contacts.is_empty() && texts(e).iter().any(|t| t == text)
}

/// Alice hands Bob an invite, Bob asks, Alice accepts.
async fn meet(url: &str, name: &str) -> (Box<Engine>, Box<Engine>, PathBuf) {
    let d = dir(name);
    let mut alice = engine(&d, "alice", url);
    let mut bob = engine(&d, "bob", url);
    pump(&mut alice, &mut bob, |a, _| a.data.invite_queue.is_some()).await;
    let invite = alice.invite().unwrap();
    bob.add(&invite, false).unwrap();
    pump(&mut alice, &mut bob, |a, _| {
        !a.data.contacts.is_empty() && a.data.contacts[0].theirs.is_some()
    })
    .await;
    alice.accept(0);
    pump(&mut alice, &mut bob, |_, b| got("accepted your request")(b)).await;
    (alice, bob, d)
}

#[tokio::test(flavor = "multi_thread")]
async fn contacts_meet_through_queues_and_talk_both_ways() {
    let url = start_relay().await;
    let (mut alice, mut bob, d) = meet(&url, "meet").await;
    let (ca, cb) = (&alice.data.contacts[0], &bob.data.contacts[0]);
    // Each writes to the queue the other made for them, not to an invite queue.
    assert_eq!(cb.theirs, ca.mine.as_ref().map(|q| q.addr()));
    assert_eq!(ca.theirs, cb.mine.as_ref().map(|q| q.addr()));
    assert_eq!(cb.their_invite, None);
    assert_ne!(
        ca.mine.as_ref().map(|q| q.send_id),
        alice.data.invite_queue.as_ref().map(|q| q.send_id)
    );

    bob.send(0, "hi alice").unwrap();
    pump(&mut alice, &mut bob, |a, _| got("hi alice")(a)).await;
    alice.send(0, "hi bob").unwrap();
    pump(&mut alice, &mut bob, |_, b| got("hi bob")(b)).await;
    let _ = std::fs::remove_dir_all(d);
}

#[tokio::test(flavor = "multi_thread")]
async fn blocking_deletes_the_queue_and_unblocking_makes_a_new_one() {
    let url = start_relay().await;
    let (mut alice, mut bob, d) = meet(&url, "block").await;
    let old = alice.data.contacts[0].mine.as_ref().unwrap().send_id;
    alice.block(0, true);
    assert!(alice.data.contacts[0].mine.is_none());
    // Bob's message finds no queue: refused for good.
    pump(&mut alice, &mut bob, |_, _| true).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    bob.send(0, "let me in").unwrap();
    pump(&mut alice, &mut bob, |_, b| {
        b.data.contacts[0]
            .messages
            .iter()
            .any(|m| matches!(&m.status, Status::Failed(_)))
    })
    .await;

    alice.block(0, false);
    pump(&mut alice, &mut bob, |_, b| {
        b.data.contacts[0]
            .theirs
            .as_ref()
            .is_some_and(|a| a.send_id != old)
    })
    .await;
    bob.send(0, "back again").unwrap();
    pump(&mut alice, &mut bob, |a, _| got("back again")(a)).await;
    let _ = std::fs::remove_dir_all(d);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contact_from_before_queues_needs_an_invite_and_keeps_its_history() {
    let url = start_relay().await;
    let (mut alice, mut bob, d) = meet(&url, "migrate").await;
    bob.send(0, "before the move").unwrap();
    pump(&mut alice, &mut bob, |a, _| got("before the move")(a)).await;
    let bob_name = alice.name(&alice.data.contacts[0]);

    // What a state file from version 1 looks like: no queues either way.
    for e in [&mut alice, &mut bob] {
        let c = &mut e.data.contacts[0];
        (c.mine, c.theirs, c.their_invite) = (None, None, None);
        e.persist();
    }
    assert!(alice.needs_invite(&alice.data.contacts[0]));
    assert_eq!(alice.send(0, "hello?"), Err(NEEDS_INVITE.into()));

    // Bob sends a new invite; Alice adds it, and the old chat goes on.
    pump(&mut alice, &mut bob, |_, b| b.data.invite_queue.is_some()).await;
    let invite = bob.invite().unwrap();
    alice.add(&invite, false).unwrap();
    assert_eq!(
        alice.data.contacts.len(),
        1,
        "the same contact, not a new one"
    );
    pump(&mut alice, &mut bob, |a, b| {
        a.data.contacts[0].theirs.is_some() && b.data.contacts[0].theirs.is_some()
    })
    .await;
    assert!(!bob.is_request(&bob.data.contacts[0].key), "no new request");
    alice.send(0, "after the move").unwrap();
    pump(&mut alice, &mut bob, |_, b| got("after the move")(b)).await;
    assert!(texts(&alice).iter().any(|t| t == "before the move"));
    assert_eq!(alice.name(&alice.data.contacts[0]), bob_name);
    let _ = std::fs::remove_dir_all(d);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_incognito_invite_comes_once_its_queue_exists() {
    let url = start_relay().await;
    let d = dir("incognito");
    let mut alice = engine(&d, "alice", &url);
    let mut bob = engine(&d, "bob", &url);
    alice.incognito_invite("").unwrap();
    pump(&mut alice, &mut bob, |a, _| {
        a.happened.iter().any(|h| matches!(h, Happened::Invite(_)))
    })
    .await;
    let invite = alice
        .happened
        .iter()
        .find_map(|h| match h {
            Happened::Invite(i) => Some(i.clone()),
            _ => None,
        })
        .unwrap();
    bob.add(&invite, false).unwrap();
    pump(&mut alice, &mut bob, |a, _| !a.data.contacts.is_empty()).await;
    let c = &alice.data.contacts[0];
    assert_eq!(c.profile, Some(alice.data.profiles[0].key));
    let _ = std::fs::remove_dir_all(d);
}
