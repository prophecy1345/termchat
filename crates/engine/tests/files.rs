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
    let deadline = Instant::now() + Duration::from_secs(60);
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
    pump(&mut alice, &mut bob, |a, _| a.data.invite_queue.is_some()).await;
    let settle = Instant::now() + Duration::from_secs(1);
    pump(&mut alice, &mut bob, |_, _| Instant::now() > settle).await;
    let invite = alice.invite().unwrap();
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
    // ADR-043: above 1 MiB a document is offered instead; past 25 MiB, refused.
    let big = vec![0; termchat_core::proto::MAX_LARGE_FILE_BYTES as usize + 1];
    assert!(bob.send_file(0, "big.bin", big).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A spool file of either engine, by its state folder.
fn spooled(dir: &Path, name: &str) -> usize {
    let spool = termchat_engine::data::spool_dir(&dir.join(name));
    std::fs::read_dir(spool).map_or(0, |d| d.count())
}

#[tokio::test(flavor = "multi_thread")]
async fn large_documents_go_only_when_asked_and_leave_nothing_behind() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-large-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut alice = engine(&dir, "alice", &url);
    let mut bob = engine(&dir, "bob", &url);
    pump(&mut alice, &mut bob, |a, _| a.data.invite_queue.is_some()).await;
    let settle = Instant::now() + Duration::from_secs(1);
    pump(&mut alice, &mut bob, |_, _| Instant::now() > settle).await;
    bob.add(&alice.invite().unwrap(), false).unwrap();
    pump(&mut alice, &mut bob, |a, _| !a.data.contacts.is_empty()).await;
    alice.accept(0);
    // Both sides must know the other reads offers (ADR-043, level 7).
    pump(&mut alice, &mut bob, |a, b| {
        a.data.contacts[0].understands >= 7 && b.data.contacts[0].understands >= 7
    })
    .await;

    let doc: Vec<u8> = (0..1_200_000u32).map(|n| (n % 253) as u8).collect();
    bob.send_file(0, "scan.pdf", doc.clone()).unwrap();
    assert_eq!(spooled(&dir, "bob"), 1);
    pump(&mut alice, &mut bob, |a, _| a.unsaved_file(0).is_some()).await;
    let m = alice.unsaved_file(0).unwrap();
    let offer = alice.data.contacts[0].messages[m].file.clone().unwrap();
    assert!(offer.offered());
    assert!(offer.label().ends_with("offered"), "{}", offer.label());
    // Nothing moves until she asks, and there is nothing to save yet.
    let settle = Instant::now() + Duration::from_secs(2);
    pump(&mut alice, &mut bob, |_, _| Instant::now() > settle).await;
    let sent = |b: &Engine| {
        b.data.contacts[0]
            .messages
            .iter()
            .rev()
            .find_map(|m| m.file.clone())
    };
    assert_eq!(sent(&bob).unwrap().done, 0);
    assert!(alice.file_bytes(0, m).is_none());

    alice.download_file(0, m).unwrap();
    assert!(alice.download_file(0, m).is_err());
    pump(&mut alice, &mut bob, |a, b| {
        a.data.contacts[0].messages[m]
            .file
            .as_ref()
            .is_some_and(|f| f.complete())
            && sent(b).is_some_and(|f| !f.held())
    })
    .await;
    assert_eq!(
        alice.file_bytes(0, m).unwrap(),
        ("scan.pdf".into(), doc.clone())
    );
    let saved = alice.save_file(0, m, &dir).unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), doc);
    assert_eq!(alice.unsaved_file(0), None);
    pump(&mut alice, &mut bob, |_, _| {
        spooled(&dir, "alice") == 0 && spooled(&dir, "bob") == 0
    })
    .await;

    // Declined: the sender's copy goes, both lines say so.
    bob.send_file(0, "photos.zip", vec![9; 1_100_000]).unwrap();
    pump(&mut alice, &mut bob, |a, _| a.unsaved_file(0).is_some()).await;
    let m = alice.unsaved_file(0).unwrap();
    alice.discard_file(0, m);
    pump(&mut alice, &mut bob, |_, b| {
        sent(b).is_some_and(|f| !f.held())
    })
    .await;
    assert!(sent(&bob).unwrap().label().ends_with("not taken"));
    assert_eq!(spooled(&dir, "bob"), 0);
    assert!(alice.download_file(0, m).is_err());

    // ADR-044: a received image can be viewed while it is held; the
    // large document above, not an image, never could.
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        &[200u8; 5 * 4 * 4],
        5,
        4,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
    bob.send_file(0, "photo.dat", png).unwrap();
    pump(&mut alice, &mut bob, |a, _| a.unsaved_file(0).is_some()).await;
    let m = alice.unsaved_file(0).unwrap();
    let p = termchat_engine::picture::decode(&alice.image_bytes(0, m).unwrap()).unwrap();
    assert_eq!((p.width, p.height, p.rgba.len()), (5, 4, 80));
    let first = alice.data.contacts[0]
        .messages
        .iter()
        .position(|m| m.text == "scan.pdf")
        .unwrap();
    assert!(alice.image_bytes(0, first).is_err());
    alice.discard_file(0, m);
    assert!(alice.image_bytes(0, m).is_err());

    // Over the new limit.
    let huge = vec![0; termchat_core::proto::MAX_LARGE_FILE_BYTES as usize + 1];
    assert!(bob.send_file(0, "huge.bin", huge).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
