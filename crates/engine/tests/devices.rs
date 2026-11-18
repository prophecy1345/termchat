//! Linking a second device (ADR-028) through a real relay: the link code,
//! the fingerprint on both screens, and the list the primary signs.
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

async fn pump(e: &mut [Box<Engine>], done: impl Fn(&[Box<Engine>]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(e) {
        assert!(Instant::now() < deadline, "timed out");
        for x in e.iter_mut() {
            while let Ok(ev) = x.events.try_recv() {
                x.on_relay(ev);
            }
            x.tick();
            x.happened.clear();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn settle(e: &mut [Box<Engine>], ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    pump(e, |_| Instant::now() > until).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_laptop_links_to_the_phone_once_both_screens_agree() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = vec![engine(&dir, "alice", &url), engine(&dir, "new", &url)];
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    let (phone, laptop) = (0, 1);

    let code = e[laptop].link_code().unwrap();
    assert!(code.starts_with("termchat-link:"));
    assert!(e[phone].add_device("termchat:not-a-link").is_err());
    let at_phone = e[phone].add_device(&code).unwrap();
    assert_eq!(e[phone].pending_link(), Some(at_phone));

    // The laptop hears from the phone, and both show the phone's fingerprint.
    pump(&mut e, |e| e[1].pending_link().is_some()).await;
    assert_eq!(e[phone].link_fingerprint(), e[laptop].link_fingerprint());
    // Nothing is linked before both screens are confirmed.
    settle(&mut e, 500).await;
    assert!(e[phone].data.devices.is_none() && e[laptop].data.devices.is_none());

    let at_laptop = e[laptop].pending_link().unwrap();
    e[laptop].confirm_link(at_laptop);
    settle(&mut e, 1000).await;
    assert!(
        e[phone].data.devices.is_none(),
        "the phone has not confirmed"
    );
    e[phone].confirm_link(at_phone);
    pump(&mut e, |e| e[1].data.devices.is_some()).await;

    let list = e[phone].data.devices.clone().unwrap();
    assert_eq!(e[laptop].data.devices.as_ref(), Some(&list));
    assert_eq!(list.identity, e[phone].me());
    assert_eq!(list.devices.len(), 2);
    assert_eq!(list.devices[1].ed25519, e[laptop].me());
    assert!(!e[laptop].is_primary());
    assert_eq!(e[laptop].data.nickname, "alice");
    // A linked device links no one, and asks for no code of its own.
    assert!(e[laptop].link_code().is_err());
    assert!(e[laptop].add_device(&code).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_the_device_did_not_give_links_nothing() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = vec![engine(&dir, "alice", &url), engine(&dir, "new", &url)];
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;

    // A code made, then forgotten: a new one replaces it.
    let old = e[1].link_code().unwrap();
    let _new = e[1].link_code().unwrap();
    e[0].add_device(&old).unwrap();
    settle(&mut e, 2000).await;
    assert_eq!(e[1].pending_link(), None);
    assert!(e[1].data.primary.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}
