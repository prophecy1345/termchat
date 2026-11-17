//! Two people on two relays (ADR-023, decision 4): each one's queue lives on
//! their own relay, so neither relay holds both sides of the conversation.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::Path;
use std::time::{Duration, Instant};

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
        assert!(
            Instant::now() < deadline,
            "timed out: {:?} {:?}",
            a.happened,
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

fn has(e: &Engine, text: &str) -> bool {
    e.data.contacts[0].messages.iter().any(|m| m.text == text)
}

#[tokio::test(flavor = "multi_thread")]
async fn each_side_keeps_its_queue_on_its_own_relay() {
    let (own, public) = (start_relay().await, start_relay().await);
    let dir = std::env::temp_dir().join(format!("termchat-two-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Alice runs her own relay; Bob stays on the public one.
    let mut alice = engine(&dir, "alice", &own);
    let mut bob = engine(&dir, "bob", &public);

    pump(&mut alice, &mut bob, |a, b| {
        a.data.invite_queue.is_some() && b.data.invite_queue.is_some()
    })
    .await;
    let until = Instant::now() + Duration::from_secs(1);
    pump(&mut alice, &mut bob, |_, _| Instant::now() > until).await;
    bob.add(&alice.invite().unwrap(), false).unwrap();
    pump(&mut alice, &mut bob, |a, _| !a.data.contacts.is_empty()).await;
    alice.accept(0);
    pump(&mut alice, &mut bob, |a, b| a.rich(0) && b.rich(0)).await;

    bob.send(0, "hi from the public relay").unwrap();
    pump(&mut alice, &mut bob, |a, _| {
        has(a, "hi from the public relay")
    })
    .await;
    let m = alice.data.contacts[0]
        .messages
        .iter()
        .position(|m| m.text == "hi from the public relay")
        .unwrap();
    alice.reply(0, m, "answered from my own relay").unwrap();
    alice.react(0, m, Some(1)).unwrap();
    pump(&mut alice, &mut bob, |_, b| {
        has(b, "answered from my own relay")
            && b.data.contacts[0]
                .messages
                .iter()
                .any(|m| m.their_reaction == Some(1))
    })
    .await;

    // Each queue sits on its owner's relay, and each side writes to the other's.
    let (a, b) = (&alice.data.contacts[0], &bob.data.contacts[0]);
    assert_eq!(a.mine.as_ref().unwrap().relay_url, own);
    assert_eq!(b.mine.as_ref().unwrap().relay_url, public);
    assert_eq!(a.theirs.as_ref().unwrap().relay_url, public);
    assert_eq!(b.theirs.as_ref().unwrap().relay_url, own);
    std::fs::remove_dir_all(&dir).unwrap();
}
