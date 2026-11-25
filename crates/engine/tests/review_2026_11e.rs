//! Regression tests of the audit of 2026-11-25 (0.34.0, `42773b1`).
//! The harness is the one of `crates/engine/tests/review_2026_11d.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termchat_engine::{Engine, Setup};
use termchat_relay::{Config, Limits};
use tokio::net::TcpListener;

const PASSWORD: &str = "correct horse battery staple violin";
const ALICE: usize = 0;
const BOB: usize = 1;

async fn start_relay() -> String {
    start_relay_with(Limits::default()).await
}

async fn start_relay_with(limits: Limits) -> String {
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

/// Each engine in a folder of its own, so one can be made read-only.
fn state(dir: &Path, name: &str) -> PathBuf {
    let own = dir.join(name);
    std::fs::create_dir_all(&own).unwrap();
    own.join("state")
}

async fn world(name: &str) -> (Vec<Box<Engine>>, PathBuf) {
    world_on(name, start_relay().await).await
}

async fn world_on(name: &str, url: String) -> (Vec<Box<Engine>>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("termchat-11e-{name}-{}", std::process::id()));
    let e = ["alice", "bob"]
        .iter()
        .map(|n| {
            let setup = Setup {
                relay_url: &url,
                onion_url: None,
                tor: false,
                nickname: n,
                password: PASSWORD,
            };
            Engine::setup(&state(&dir, n), &setup).unwrap()
        })
        .collect();
    let mut e: Vec<Box<Engine>> = e;
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    (e, dir)
}

async fn pump(e: &mut [Box<Engine>], done: impl Fn(&[Box<Engine>]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while !done(e) {
        assert!(Instant::now() < deadline, "timed out");
        let mut any = false;
        for x in e.iter_mut() {
            while let Ok(ev) = x.events.try_recv() {
                x.on_relay(ev);
                any = true;
            }
            x.tick();
            x.happened.clear();
        }
        if !any {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

async fn befriend(e: &mut [Box<Engine>], a: usize, b: usize) {
    let invite = e[b].invite().unwrap();
    e[a].add(&invite, false).unwrap();
    pump(e, |e| {
        e[b].data.contacts.iter().any(|c| e[b].is_request(&c.key))
    })
    .await;
    let i = e[b]
        .data
        .contacts
        .iter()
        .position(|c| e[b].is_request(&c.key))
        .unwrap();
    e[b].accept(i);
    let (ka, kb) = (e[a].me(), e[b].me());
    pump(e, |e| {
        let rich = |x: &Engine, k| x.index(&k).is_some_and(|i| x.rich(i));
        rich(&e[a], kb) && rich(&e[b], ka)
    })
    .await;
}

fn has(e: &Engine, who: &[u8; 32], text: &str) -> bool {
    e.index(who)
        .is_some_and(|i| e.data.contacts[i].messages.iter().any(|m| m.text == text))
}

fn unlock(path: &Path) -> Box<Engine> {
    match Engine::unlock(path, PASSWORD.as_bytes()).unwrap() {
        termchat_engine::Unlocked::Open(x) => x,
        _ => panic!("state"),
    }
}

/// Finding 2: a message decrypted while the disk refuses the save is not
/// acked, so after a restart from disk it comes again; and once a save
/// works, it is acked and on disk.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_message_not_saved_is_not_lost() {
    use std::os::unix::fs::PermissionsExt;
    let (mut e, dir) = world("unsaved").await;
    befriend(&mut e, ALICE, BOB).await;
    let (ka, bob_dir) = (e[ALICE].me(), dir.join("bob"));
    let lock = |mode| std::fs::set_permissions(&bob_dir, std::fs::Permissions::from_mode(mode));

    lock(0o500).unwrap();
    let i = e[ALICE].index(&e[BOB].me()).unwrap();
    e[ALICE].send(i, "kept 1").unwrap();
    pump(&mut e, |e| has(&e[BOB], &ka, "kept 1")).await;
    // Bob stops without a save; on disk is the state before the message.
    lock(0o700).unwrap();
    e[BOB] = unlock(&state(&dir, "bob"));
    pump(&mut e, |e| has(&e[BOB], &ka, "kept 1")).await;

    // The same with the save coming back while Bob runs.
    lock(0o500).unwrap();
    e[ALICE].send(i, "kept 2").unwrap();
    pump(&mut e, |e| has(&e[BOB], &ka, "kept 2")).await;
    lock(0o700).unwrap();
    let settle = Instant::now() + Duration::from_secs(2);
    pump(&mut e, |_| Instant::now() > settle).await;
    e[BOB] = unlock(&state(&dir, "bob"));
    assert!(has(&e[BOB], &ka, "kept 2"), "saved once the disk took it");
    let settle = Instant::now() + Duration::from_secs(2);
    pump(&mut e, |_| Instant::now() > settle).await;
    let kept = |e: &Engine| {
        let i = e.index(&ka).unwrap();
        e.data.contacts[i]
            .messages
            .iter()
            .filter(|m| m.text == "kept 2")
            .count()
    };
    assert_eq!(kept(&e[BOB]), 1, "acked: not delivered again");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Finding 4: a large document goes on after either side restarts from
/// disk in the middle of it, and arrives whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_document_survives_restarts_on_both_sides() {
    let (mut e, dir) = world("large").await;
    befriend(&mut e, ALICE, BOB).await;
    let (ka, kb) = (e[ALICE].me(), e[BOB].me());
    let doc: Vec<u8> = (0..3_000_000u32).map(|n| (n % 253) as u8).collect();
    let to_bob = e[ALICE].index(&kb).unwrap();
    e[ALICE].send_file(to_bob, "big.bin", doc.clone()).unwrap();
    let offered = |e: &Engine| {
        let i = e.index(&ka)?;
        e.data.contacts[i]
            .messages
            .iter()
            .position(|m| m.file.as_ref().is_some_and(|f| f.offered()))
    };
    pump(&mut e, |e| offered(&e[BOB]).is_some()).await;
    let m = offered(&e[BOB]).unwrap();
    let from_alice = e[BOB].index(&ka).unwrap();
    e[BOB].download_file(from_alice, m).unwrap();
    let done = |e: &Engine| {
        e.data.contacts[e.index(&ka).unwrap()].messages[m]
            .file
            .as_ref()
            .map_or(0, |f| f.done)
    };
    pump(&mut e, |e| done(&e[BOB]) >= 5).await;
    // A part counted and saved, then lost with the app before it went out.
    let mine = e[ALICE].data.contacts[to_bob]
        .messages
        .iter()
        .position(|m| m.mine && m.file.is_some())
        .unwrap();
    if let Some(f) = e[ALICE].data.contacts[to_bob].messages[mine].file.as_mut() {
        f.done += 1;
    }
    assert!(e[ALICE].persist());
    e[ALICE] = unlock(&state(&dir, "alice"));
    pump(&mut e, |e| done(&e[BOB]) >= 20).await;
    e[BOB] = unlock(&state(&dir, "bob"));
    pump(&mut e, |e| e[BOB].unsaved_file(from_alice).is_some()).await;
    let saved = e[BOB].save_file(from_alice, m, &dir).unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), doc);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Finding 7: a queue the relay refused for its rate limit is asked for
/// again later, and every owner still gets its own queue.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_queue_is_asked_for_again() {
    let limits = Limits {
        queues_per_ip: termchat_relay::Rate {
            burst: 1.0,
            per_sec: 0.5,
        },
        ..Limits::default()
    };
    let (mut e, dir) = world_on("refused", start_relay_with(limits).await).await;
    befriend(&mut e, ALICE, BOB).await;
    let (ka, kb) = (e[ALICE].me(), e[BOB].me());
    let i = e[ALICE].index(&kb).unwrap();
    e[ALICE].send(i, "through").unwrap();
    pump(&mut e, |e| has(&e[BOB], &ka, "through")).await;
    std::fs::remove_dir_all(&dir).unwrap();
}
