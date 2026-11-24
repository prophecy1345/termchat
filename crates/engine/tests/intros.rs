//! Three full engines through a real relay: Alice introduces Bob to Carol
//! (ADR-045).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::Path;
use std::time::{Duration, Instant};

use termchat_engine::engine::{IntroAnswer, PendingIntro};
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

/// Feeds relay events and ticks all engines until `done` holds.
async fn pump(e: &mut [Box<Engine>; 3], done: impl Fn(&[Box<Engine>; 3]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while !done(e) {
        assert!(Instant::now() < deadline, "timed out");
        let [a, b, c] = e;
        let wait = Duration::from_millis(50);
        tokio::select! {
            Some(ev) = a.events.recv() => a.on_relay(ev),
            Some(ev) = b.events.recv() => b.on_relay(ev),
            Some(ev) = c.events.recv() => c.on_relay(ev),
            () = tokio::time::sleep(wait) => {}
        }
        for x in e.iter_mut() {
            x.tick();
        }
    }
}

fn index(e: &Engine, name: &str) -> Option<usize> {
    e.data
        .contacts
        .iter()
        .position(|c| e.name(c).starts_with(name))
}

fn texts(e: &Engine, i: usize) -> Vec<String> {
    e.data.contacts[i]
        .messages
        .iter()
        .map(|m| m.text.clone())
        .collect()
}

/// Alice has Bob and Carol as accepted contacts, all at the current level.
async fn three(dir: &Path) -> [Box<Engine>; 3] {
    let url = start_relay().await;
    let mut e = [
        engine(dir, "alice", &url),
        engine(dir, "bob", &url),
        engine(dir, "carol", &url),
    ];
    pump(&mut e, |e| e[0].data.invite_queue.is_some()).await;
    let settle = Instant::now() + Duration::from_secs(1);
    pump(&mut e, |_| Instant::now() > settle).await;
    for n in [1, 2] {
        let invite = e[0].invite().unwrap();
        e[n].add(&invite, false).unwrap();
        let request = |x: &Engine| x.data.contacts.iter().position(|c| x.is_request(&c.key));
        pump(&mut e, |e| request(&e[0]).is_some()).await;
        let i = request(&e[0]).unwrap();
        e[0].accept(i);
    }
    let level = termchat_core::proto::UNDERSTANDS;
    // Notes (ADR-046) is no one to hear a level from.
    let ready = |x: &Engine| {
        x.data
            .contacts
            .iter()
            .all(|c| c.notes || c.understands >= level)
    };
    pump(&mut e, |e| ready(&e[0]) && ready(&e[1]) && ready(&e[2])).await;

    e
}

fn ask_id(e: &Engine, i: usize) -> Option<u64> {
    match e.pending_intro(i)? {
        PendingIntro::Ask { id, .. } => Some(id),
        PendingIntro::Offer { .. } => None,
    }
}

fn offer_id(e: &Engine, i: usize) -> Option<u64> {
    match e.pending_intro(i)? {
        PendingIntro::Offer { id, .. } => Some(id),
        PendingIntro::Ask { .. } => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn carol_says_yes_bob_adds_her_and_they_talk() {
    let dir = std::env::temp_dir().join(format!("termchat-intro-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = three(&dir).await;
    let (bob, carol) = (index(&e[0], "bob").unwrap(), index(&e[0], "carol").unwrap());
    assert!(e[0].introduce_contacts(bob, bob).is_err());
    e[0].introduce_contacts(bob, carol).unwrap();

    // Carol is asked; Bob has heard nothing yet.
    pump(&mut e, |e| ask_id(&e[2], 0).is_some()).await;
    assert!(e[1].pending_intro(0).is_none());
    let id = ask_id(&e[2], 0).unwrap();
    e[2].answer_intro(0, id, IntroAnswer::Allow).unwrap();
    assert!(e[2].answer_intro(0, id, IntroAnswer::Allow).is_err());

    // Bob gets her invite through Alice and adds her: no request to accept.
    pump(&mut e, |e| offer_id(&e[1], 0).is_some()).await;
    let id = offer_id(&e[1], 0).unwrap();
    let j = e[1].answer_offer(0, id, true).unwrap().unwrap();
    pump(&mut e, |e| {
        index(&e[2], "bob").is_some_and(|k| !e[2].is_request(&e[2].data.contacts[k].key))
            && texts(&e[1], j).iter().any(|t| t == "accepted your request")
    })
    .await;
    let k = index(&e[2], "bob").unwrap();
    assert!(
        texts(&e[2], k)
            .iter()
            .any(|t| t.starts_with("introduced by alice"))
    );
    assert!(!e[2].data.contacts[k].verified);
    e[1].send(j, "hi carol").unwrap();
    pump(&mut e, |e| texts(&e[2], k).iter().any(|t| t == "hi carol")).await;
    assert!(e[0].data.intros.is_empty() && e[1].data.intros.is_empty());
    assert!(e[2].data.intros.is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_no_reaches_alice_only_and_incognito_hides_carol() {
    let dir = std::env::temp_dir().join(format!("termchat-intro2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = three(&dir).await;
    let (bob, carol) = (index(&e[0], "bob").unwrap(), index(&e[0], "carol").unwrap());

    e[0].introduce_contacts(bob, carol).unwrap();
    pump(&mut e, |e| ask_id(&e[2], 0).is_some()).await;
    let id = ask_id(&e[2], 0).unwrap();
    e[2].answer_intro(0, id, IntroAnswer::Decline).unwrap();
    pump(&mut e, |e| e[0].data.intros.is_empty()).await;
    assert!(
        texts(&e[0], carol)
            .iter()
            .any(|t| t == "they declined the introduction")
    );
    assert!(e[1].pending_intro(0).is_none());

    // Yes, from a new identity: Bob meets that one, not Carol's main key.
    e[0].introduce_contacts(bob, carol).unwrap();
    pump(&mut e, |e| ask_id(&e[2], 0).is_some()).await;
    let id = ask_id(&e[2], 0).unwrap();
    e[2].answer_intro(0, id, IntroAnswer::AllowIncognito)
        .unwrap();
    pump(&mut e, |e| offer_id(&e[1], 0).is_some()).await;
    let id = offer_id(&e[1], 0).unwrap();
    let j = e[1].answer_offer(0, id, true).unwrap().unwrap();
    let main_key = e[0].data.contacts[carol].key;
    assert_ne!(e[1].data.contacts[j].key, main_key);
    pump(&mut e, |e| {
        texts(&e[1], j).iter().any(|t| t == "accepted your request")
    })
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
}
