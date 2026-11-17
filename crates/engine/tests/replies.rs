//! Two full engines answer, edit, delete and react through a real relay (ADR-025).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::Path;
use std::time::{Duration, Instant};

use termchat_engine::engine::{Fetched, OLDER_APP};
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

#[tokio::test(flavor = "multi_thread")]
async fn answers_edits_reactions_and_deletes_travel() {
    let dir = std::env::temp_dir().join(format!("termchat-replies-{}", std::process::id()));
    let (mut alice, mut bob) = strangers(&dir).await;
    // A stranger hears nothing from alice before she accepts, not even this.
    assert!(!bob.rich(0));
    assert_eq!(bob.reply(0, 0, "x"), Err(OLDER_APP.into()));
    alice.accept(0);
    pump(&mut alice, &mut bob, |a, b| a.rich(0) && b.rich(0)).await;

    bob.send(0, "helo").unwrap();
    pump(&mut alice, &mut bob, |a, _| has(a, "helo")).await;
    let m = at(&alice, "helo");
    alice.reply(0, m, "hi bob").unwrap();
    pump(&mut alice, &mut bob, |_, b| has(b, "hi bob")).await;
    // The answer points at bob's own message, seen from bob.
    let answer = &bob.data.contacts[0].messages[at(&bob, "hi bob")];
    let asked = &bob.data.contacts[0].messages[at(&bob, "helo")];
    let to = answer.reply_to.unwrap();
    assert_eq!((Some(to.id), to.theirs), (asked.id, false));

    // Only one's own messages change.
    assert!(alice.edit_message(0, m, "mine now").is_err());
    assert!(alice.delete_message(0, m).is_err());
    bob.edit_message(0, at(&bob, "helo"), "hello").unwrap();
    pump(&mut alice, &mut bob, |a, _| has(a, "hello")).await;
    assert!(alice.data.contacts[0].messages[m].edited);
    // Edited before `recv` took it: it comes once, as it is now.
    let got = alice.take_new(None);
    let got: Vec<_> = got.iter().map(|(_, m, w)| (m.text.as_str(), *w)).collect();
    assert_eq!(got, [("hello", Fetched::Message)]);

    alice.react(0, m, Some(6)).unwrap();
    assert!(alice.react(0, m, Some(7)).is_err());
    pump(&mut alice, &mut bob, |_, b| {
        b.data.contacts[0].messages[at(b, "hello")].their_reaction == Some(6)
    })
    .await;
    let got = bob.take_new(None);
    assert!(
        got.iter()
            .any(|(_, m, w)| *w == Fetched::React && m.text == "hello")
    );

    bob.delete_message(0, at(&bob, "hello")).unwrap();
    pump(&mut alice, &mut bob, |a, _| {
        a.data.contacts[0].messages[m].deleted
    })
    .await;
    let gone = &alice.data.contacts[0].messages[m];
    assert!(gone.text.is_empty() && gone.their_reaction.is_none());
    let got = alice.take_new(None);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].2, Fetched::Delete);
    // The answer stays; what it answered is gone.
    assert!(alice.reply(0, m, "to nothing").is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn understands_is_said_again_to_a_contact_who_updated_later() {
    let dir = std::env::temp_dir().join(format!("termchat-understands-{}", std::process::id()));
    let (mut alice, mut bob) = strangers(&dir).await;
    alice.accept(0);
    pump(&mut alice, &mut bob, |a, b| a.rich(0) && b.rich(0)).await;
    settle(&mut alice, &mut bob).await;

    alice.data.contacts[0].understands = 0;
    bob.data.contacts[0].understands = 0;
    bob.data.contacts[0].told = 0;
    bob.send(0, "updated").unwrap();
    pump(&mut alice, &mut bob, |a, b| a.rich(0) && b.rich(0)).await;

    settle(&mut alice, &mut bob).await;
    let told = |e: &Engine| e.data.contacts[0].told;
    let level = termchat_core::proto::UNDERSTANDS;
    assert_eq!((told(&alice), told(&bob)), (level, level));
    bob.send(0, "after").unwrap();
    pump(&mut alice, &mut bob, |a, _| has(a, "after")).await;
    alice.react(0, at(&alice, "after"), Some(0)).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}
