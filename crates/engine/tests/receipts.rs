//! Delivery and read receipts between two engines through a real relay (ADR-041).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::Path;
use std::time::{Duration, Instant};

use termchat_engine::engine::Fetched;
use termchat_engine::{Engine, Setup};
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

async fn pump(a: &mut Engine, b: &mut Engine, done: impl Fn(&Engine, &Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(a, b) {
        assert!(Instant::now() < deadline, "timed out");
        tokio::select! {
            Some(ev) = a.events.recv() => a.on_relay(ev),
            Some(ev) = b.events.recv() => b.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        a.tick();
        b.tick();
    }
}

async fn settle(a: &mut Engine, b: &mut Engine) {
    let until = Instant::now() + Duration::from_secs(1);
    pump(a, b, |_, _| Instant::now() > until).await;
}

/// Index of the message with `text` in the only chat.
fn at(e: &Engine, text: &str) -> usize {
    e.data.contacts[0]
        .messages
        .iter()
        .position(|m| m.text == text)
        .unwrap_or_else(|| panic!("no {text:?}"))
}

fn has(e: &Engine, text: &str) -> bool {
    e.data.contacts[0].messages.iter().any(|m| m.text == text)
}

async fn strangers(dir: &Path) -> (Box<Engine>, Box<Engine>) {
    let url = start_relay().await;
    std::fs::create_dir_all(dir).unwrap();
    let mut alice = engine(dir, "alice", &url);
    let mut bob = engine(dir, "bob", &url);

    pump(&mut alice, &mut bob, |a, _| a.data.invite_queue.is_some()).await;
    settle(&mut alice, &mut bob).await;
    bob.add(&alice.invite().unwrap(), false).unwrap();
    pump(&mut alice, &mut bob, |a, _| !a.data.contacts.is_empty()).await;
    settle(&mut alice, &mut bob).await;
    (alice, bob)
}

fn mine<'a>(e: &'a Engine, text: &str) -> &'a termchat_engine::data::Msg {
    &e.data.contacts[0].messages[at(e, text)]
}

/// The random wait is over now.
fn hurry(e: &mut Engine) {
    if e.data.contacts[0].receipts.due.is_some() {
        e.data.contacts[0].receipts.due = Some(0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn receipts_travel_only_when_allowed() {
    let dir = std::env::temp_dir().join(format!("termchat-receipts-{}", std::process::id()));
    let (mut alice, mut bob) = strangers(&dir).await;
    // A stranger learns nothing: no receipt for a request not yet accepted.
    bob.send(0, "knock").unwrap();
    settle(&mut alice, &mut bob).await;
    let r = &alice.data.contacts[0].receipts;
    assert!(r.delivered.is_empty() && r.due.is_none());

    alice.accept(0);
    pump(&mut alice, &mut bob, |a, b| a.rich(0) && b.rich(0)).await;
    bob.send(0, "one").unwrap();
    pump(&mut alice, &mut bob, |a, _| has(a, "one")).await;
    assert!(alice.data.contacts[0].receipts.due.is_some());
    hurry(&mut alice);
    pump(&mut alice, &mut bob, |_, b| mine(b, "one").delivered).await;
    assert!(!mine(&bob, "one").read, "read receipts are off by default");
    let got = bob.take_new(None);
    assert!(
        got.iter()
            .any(|(_, m, w)| *w == Fetched::Delivered && m.text == "one")
    );

    // Gathered receipts go right before our own message, without the wait.
    bob.send(0, "two").unwrap();
    pump(&mut alice, &mut bob, |a, _| has(a, "two")).await;
    alice.send(0, "back").unwrap();
    pump(&mut alice, &mut bob, |_, b| {
        has(b, "back") && mine(b, "two").delivered
    })
    .await;

    // Read: mutual. Alice sends hers, bob shows them only once his are on.
    alice.read_receipts = true;
    alice.open = Some(alice.data.contacts[0].key);
    bob.send(0, "three").unwrap();
    pump(&mut alice, &mut bob, |a, _| {
        a.data.contacts[0]
            .messages
            .iter()
            .any(|m| m.text == "three" && m.read)
    })
    .await;
    hurry(&mut alice);
    pump(&mut alice, &mut bob, |_, b| mine(b, "three").delivered).await;
    assert!(!mine(&bob, "three").read);
    bob.read_receipts = true;
    bob.send(0, "four").unwrap();
    pump(&mut alice, &mut bob, |a, _| has(a, "four")).await;
    hurry(&mut alice);
    pump(&mut alice, &mut bob, |_, b| mine(b, "four").read).await;

    // Delivery receipts off: none go.
    alice.receipts = false;
    alice.open = None;
    bob.send(0, "five").unwrap();
    pump(&mut alice, &mut bob, |a, _| has(a, "five")).await;
    assert!(alice.data.contacts[0].receipts.due.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}
