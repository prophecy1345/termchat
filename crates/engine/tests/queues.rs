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
    meet_with(url, name, false).await
}

/// `shared`: one connection per identity for all contacts (ADR-023, section 7).
async fn meet_with(url: &str, name: &str, shared: bool) -> (Box<Engine>, Box<Engine>, PathBuf) {
    let d = dir(name);
    let mut alice = engine(&d, "alice", url);
    let mut bob = engine(&d, "bob", url);
    if shared {
        alice.set_shared_link(true).unwrap();
        bob.set_shared_link(true).unwrap();
    }
    // Her keys must be on the relay before Bob claims one.
    pump(&mut alice, &mut bob, |a, _| a.data.fallback_published).await;
    let settle = Instant::now() + Duration::from_millis(300);
    pump(&mut alice, &mut bob, |_, _| Instant::now() > settle).await;
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

#[tokio::test(flavor = "multi_thread")]
async fn one_shared_connection_carries_every_queue() {
    let url = start_relay().await;
    let (mut alice, mut bob, d) = meet_with(&url, "shared", true).await;
    assert!(alice.data.contacts[0].mine.is_some() && bob.data.contacts[0].mine.is_some());
    bob.send(0, "over one link").unwrap();
    pump(&mut alice, &mut bob, |a, _| got("over one link")(a)).await;
    alice.send(0, "and back").unwrap();
    pump(&mut alice, &mut bob, |_, b| got("and back")(b)).await;

    // Blocking deletes the queue but keeps the shared link for the rest.
    alice.block(0, true);
    alice.block(0, false);
    let old = bob.data.contacts[0].theirs.clone();
    pump(&mut alice, &mut bob, |_, b| {
        b.data.contacts[0].theirs != old
    })
    .await;
    bob.send(0, "still here").unwrap();
    pump(&mut alice, &mut bob, |a, _| got("still here")(a)).await;

    // Back to a connection per contact: the same queues, new connections.
    alice.set_shared_link(false).unwrap();
    bob.send(0, "after the switch").unwrap();
    pump(&mut alice, &mut bob, |a, _| got("after the switch")(a)).await;
    let _ = std::fs::remove_dir_all(d);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contact_who_lost_us_learns_our_name_again() {
    // Bob deleted Alice (or lost his phone); Alice still has him. He adds her
    // invite again: she joins it to the contact she has, and he must learn her
    // name, which travels only in `ContactAccept`.
    let url = start_relay().await;
    let (mut alice, mut bob, d) = meet(&url, "lost").await;
    bob.delete(0);
    assert!(bob.data.contacts.is_empty());
    let invite = alice.invite().unwrap();
    bob.add(&invite, false).unwrap();
    pump(&mut alice, &mut bob, |_, b| {
        b.name(&b.data.contacts[0]).starts_with("alice")
    })
    .await;
    assert!(
        !alice.is_request(&alice.data.contacts[0].key),
        "no new request"
    );
    bob.send(0, "hi again").unwrap();
    pump(&mut alice, &mut bob, |a, _| got("hi again")(a)).await;
    let _ = std::fs::remove_dir_all(d);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_acceptance_that_did_not_go_out_goes_after_unlock() {
    // Android 0.8.0: accepting saved the state, then the link could not
    // start. The app is locked and unlocked: the engine opens from the file.
    let url = start_relay().await;
    let d = dir("unsent");
    let mut alice = engine(&d, "alice", &url);
    let mut bob = engine(&d, "bob", &url);
    pump(&mut alice, &mut bob, |a, _| a.data.fallback_published).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let invite = alice.invite().unwrap();
    bob.add(&invite, false).unwrap();
    pump(&mut alice, &mut bob, |a, _| {
        !a.data.contacts.is_empty() && a.data.contacts[0].theirs.is_some()
    })
    .await;
    // Accepted and saved, but nothing sent: as if the link never started.
    let key = alice.data.contacts[0].key;
    alice.core().accept(&key);
    alice.data.contacts[0].accept_pending = true;
    assert!(alice.persist());
    drop(alice);
    let mut alice = match Engine::unlock(&d.join("alice"), PASSWORD.as_bytes()).unwrap() {
        termchat_engine::Unlocked::Open(e) => e,
        termchat_engine::Unlocked::Wiped => panic!("wiped"),
    };
    // Links come up spread over 0-30 s after an unlock; the test does not wait for that.
    alice.connect_now().unwrap();
    pump(&mut alice, &mut bob, |_, b| got("accepted your request")(b)).await;
    let _ = std::fs::remove_dir_all(d);
}

/// The `termchat add` path: Bob's request goes, his app closes, and he writes
/// after Alice accepted. Nothing was lost, so Alice must not hear of a gap.
#[tokio::test(flavor = "multi_thread")]
async fn no_gap_is_reported_when_the_asker_restarts_before_the_accept() {
    let url = start_relay().await;
    let d = dir("restart-ask");
    let mut alice = engine(&d, "alice", &url);
    let mut bob = engine(&d, "bob", &url);
    pump(&mut alice, &mut bob, |a, _| a.data.fallback_published).await;
    let settle = Instant::now() + Duration::from_millis(300);
    pump(&mut alice, &mut bob, |_, _| Instant::now() > settle).await;
    let invite = alice.invite().unwrap();
    bob.add(&invite, false).unwrap();
    pump(&mut alice, &mut bob, |_, b| {
        let c = &b.data.contacts[0];
        !c.request_pending && !c.offer_pending && c.mine.is_some()
    })
    .await;
    // What `termchat add` waits for before it exits.
    pump(&mut alice, &mut bob, |_, b| b.all_answered()).await;
    drop(bob);
    let mut bob = match Engine::unlock(&d.join("bob"), PASSWORD.as_bytes()).unwrap() {
        termchat_engine::Unlocked::Open(e) => e,
        termchat_engine::Unlocked::Wiped => panic!("wiped"),
    };
    bob.connect_now().unwrap();
    pump(&mut alice, &mut bob, |a, _| {
        !a.data.contacts.is_empty() && a.data.contacts[0].theirs.is_some()
    })
    .await;
    alice.accept(0);
    pump(&mut alice, &mut bob, |_, b| got("accepted your request")(b)).await;
    bob.send(0, "hello").unwrap();
    pump(&mut alice, &mut bob, |a, _| got("hello")(a)).await;
    let gaps: Vec<String> = texts(&alice)
        .into_iter()
        .filter(|t| t.contains("missing"))
        .collect();
    assert!(gaps.is_empty(), "{gaps:?}");
    let _ = std::fs::remove_dir_all(d);
}
