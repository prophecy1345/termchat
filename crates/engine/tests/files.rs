//! Two full engines send documents through a real relay (ADR-017).
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
        hosts: vec!["127.0.0.1".into()],
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

/// Feeds relay events and ticks both engines until `done` holds.
async fn pump(a: &mut Engine, b: &mut Engine, done: impl Fn(&Engine, &Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(a, b) {
        if Instant::now() > deadline {
            let dump = |e: &Engine| -> Vec<(String, String)> {
                e.data
                    .contacts
                    .iter()
                    .flat_map(|c| &c.messages)
                    .map(|m| (m.text.clone(), format!("{:?}", m.status)))
                    .collect()
            };
            panic!(
                "timed out\na: {:?} {:?}\nb: {:?} {:?}",
                dump(a),
                a.happened,
                dump(b),
                b.happened
            );
        }
        let wait = Duration::from_millis(50);
        tokio::select! {
            Some(ev) = a.events.recv() => a.on_relay(ev),
            Some(ev) = b.events.recv() => b.on_relay(ev),
            () = tokio::time::sleep(wait) => {}
        }
        a.tick();
        b.tick();
    }
}

fn last_text(e: &Engine) -> String {
    let c = &e.data.contacts[0];
    c.messages
        .last()
        .map(|m| m.text.clone())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn documents_travel_both_ways_but_not_before_acceptance() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut alice = engine(&dir, "alice", &url);
    let mut bob = engine(&dir, "bob", &url);

    // Alice's one-time keys must be on the relay before Bob claims one.
    pump(&mut alice, &mut bob, |a, b| a.connected && b.connected).await;
    let settle = Instant::now() + Duration::from_secs(1);
    pump(&mut alice, &mut bob, |_, _| Instant::now() > settle).await;
    let invite = alice.invite();
    bob.add(&invite, false).unwrap();
    // Sent while the request is still open: alice drops it unread.
    bob.send_file(0, "early.txt", b"too soon".to_vec()).unwrap();
    pump(&mut alice, &mut bob, |a, b| {
        !a.data.contacts.is_empty()
            && b.data.contacts[0]
                .messages
                .iter()
                .all(|m| m.file.as_ref().is_none_or(|f| f.data.is_none()))
    })
    .await;
    let settle = Instant::now() + Duration::from_secs(1);
    pump(&mut alice, &mut bob, |_, _| Instant::now() > settle).await;
    alice.accept(0);
    pump(&mut alice, &mut bob, |_, b| {
        last_text(b) == "accepted your request"
    })
    .await;
    assert!(
        alice.data.contacts[0]
            .messages
            .iter()
            .all(|m| m.file.is_none())
    );

    let doc: Vec<u8> = (0..170_000u32).map(|n| (n % 251) as u8).collect();
    bob.send_file(0, "report.pdf", doc.clone()).unwrap();
    assert_eq!(
        bob.send_file(0, "second.pdf", vec![1]),
        Err("wait until the previous file is sent".into())
    );
    pump(&mut alice, &mut bob, |a, _| a.unsaved_file(0).is_some()).await;
    let m = alice.unsaved_file(0).unwrap();
    let saved = alice.save_file(0, m, &dir).unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), doc);
    assert_eq!(alice.unsaved_file(0), None);
    assert!(alice.save_file(0, m, &dir).is_err());

    alice.send_file(0, "notes.txt", b"hi bob".to_vec()).unwrap();
    pump(&mut alice, &mut bob, |_, b| b.unsaved_file(0).is_some()).await;
    let m = bob.unsaved_file(0).unwrap();
    assert_eq!(
        bob.file_bytes(0, m).unwrap(),
        ("notes.txt".into(), b"hi bob".to_vec())
    );
    bob.discard_file(0, m);
    assert_eq!(bob.unsaved_file(0), None);

    assert!(bob.send_file(0, "../etc/passwd", vec![1]).is_err());
    let big = vec![0; termchat_core::proto::MAX_FILE_BYTES as usize + 1];
    assert!(bob.send_file(0, "big.bin", big).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
