//! A small private group of three through a real relay (ADR-027): the admin
//! knows both members, the members do not know each other.
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

async fn pump(e: &mut [Box<Engine>; 3], done: impl Fn(&[Box<Engine>; 3]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(e) {
        assert!(
            Instant::now() < deadline,
            "timed out: {:?} / {:?} / {:?}",
            e[0].happened,
            e[1].happened,
            e[2].happened
        );
        let [a, b, c] = e;
        tokio::select! {
            Some(ev) = a.events.recv() => a.on_relay(ev),
            Some(ev) = b.events.recv() => b.on_relay(ev),
            Some(ev) = c.events.recv() => c.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        for x in e.iter_mut() {
            x.tick();
            x.happened.clear();
        }
    }
}

async fn settle(e: &mut [Box<Engine>; 3]) {
    let until = Instant::now() + Duration::from_secs(1);
    pump(e, |_| Instant::now() > until).await;
}

/// `a` adds `b`'s invite and `b` accepts: contacts who read groups.
async fn befriend(e: &mut [Box<Engine>; 3], a: usize, b: usize) {
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
        let rich = |x: &Engine, k| {
            x.index(&k)
                .is_some_and(|i| x.rich(i) && x.data.contacts[i].understands >= 2)
        };
        rich(&e[a], kb) && rich(&e[b], ka)
    })
    .await;
}

fn said(e: &Engine, g: usize, text: &str) -> bool {
    e.data
        .groups
        .get(g)
        .is_some_and(|g| g.messages.iter().any(|m| m.text == text))
}

#[tokio::test(flavor = "multi_thread")]
async fn three_people_talk_in_a_group_and_strangers_read_each_other() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-groups-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = [
        engine(&dir, "alice", &url),
        engine(&dir, "bob", &url),
        engine(&dir, "carol", &url),
    ];
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    settle(&mut e).await;
    // Alice knows Bob and Carol; Bob and Carol do not know each other.
    befriend(&mut e, 1, 0).await;
    befriend(&mut e, 2, 0).await;
    assert!(e[1].index(&e[2].me()).is_none());

    let g = e[0].create_group("weekend").unwrap();
    pump(&mut e, |e| e[0].data.groups[0].reader.is_some()).await;
    for who in [1, 2] {
        let k = e[who].me();
        let i = e[0].index(&k).unwrap();
        e[0].invite_to_group(g, i).unwrap();
    }
    pump(&mut e, |e| {
        e[1].data.group_invites.len() == 1 && e[2].data.group_invites.len() == 1
    })
    .await;
    e[1].accept_group(0).unwrap();
    e[2].accept_group(0).unwrap();
    pump(&mut e, |e| {
        e.iter().all(|x| {
            x.data
                .groups
                .first()
                .is_some_and(|g| g.members().len() == 3)
        })
    })
    .await;
    assert_eq!(e[1].data.groups[0].name(), "weekend");

    e[0].send_group(0, "hi both").unwrap();
    e[1].send_group(0, "hi from bob").unwrap();
    e[2].send_group(0, "hi from carol").unwrap();
    pump(&mut e, |e| {
        said(&e[0], 0, "hi from bob")
            && said(&e[0], 0, "hi from carol")
            && said(&e[1], 0, "hi both")
            && said(&e[1], 0, "hi from carol")
            && said(&e[2], 0, "hi both")
            && said(&e[2], 0, "hi from bob")
    })
    .await;
    // Each message names its author; ours are ours.
    let at_bob = &e[1].data.groups[0].messages;
    let carols = at_bob.iter().find(|m| m.text == "hi from carol").unwrap();
    assert_eq!(carols.author, Some(e[2].me()));
    assert!(
        at_bob
            .iter()
            .filter(|m| m.mine)
            .all(|m| m.text == "hi from bob")
    );

    // Answers, reactions, edits and deletes, as in a chat; a member changes
    // only their own messages.
    let at = |e: &Engine, text: &str| {
        e.data.groups[0]
            .messages
            .iter()
            .position(|m| m.text == text)
            .unwrap()
    };
    let m = at(&e[1], "hi from carol");
    e[1].reply_group(0, m, "welcome, carol").unwrap();
    e[0].react_group(0, at(&e[0], "hi from carol"), Some(1))
        .unwrap();
    assert!(
        e[1].edit_group_message(0, at(&e[1], "hi from carol"), "mine now")
            .is_err()
    );
    pump(&mut e, |e| {
        let g = &e[2].data.groups[0];
        let carols = g
            .messages
            .iter()
            .find(|m| m.text == "hi from carol")
            .and_then(|m| m.id);
        said(&e[2], 0, "welcome, carol") && carols.is_some_and(|id| g.reactions_to(id).count() == 1)
    })
    .await;
    let g = &e[2].data.groups[0];
    let answer = g
        .messages
        .iter()
        .find(|m| m.text == "welcome, carol")
        .unwrap();
    let carols = g
        .messages
        .iter()
        .find(|m| m.text == "hi from carol")
        .unwrap();
    assert_eq!(answer.reply_to.map(|r| r.id), carols.id);
    e[2].edit_group_message(0, at(&e[2], "hi from carol"), "hi from carol!")
        .unwrap();
    pump(&mut e, |e| {
        said(&e[0], 0, "hi from carol!") && said(&e[1], 0, "hi from carol!")
    })
    .await;
    e[2].delete_group_message(0, at(&e[2], "hi from carol!"))
        .unwrap();
    pump(&mut e, |e| {
        e[..2].iter().all(|x| {
            x.data.groups[0]
                .messages
                .iter()
                .any(|m| m.deleted && m.author == Some(e[2].me()))
        })
    })
    .await;
    assert!(
        e[0].data.groups[0].reactions.is_empty(),
        "reactions go with the message"
    );

    // Alice removes Carol: the group moves, Carol hears so and reads nothing more.
    let carol = e[2].me();
    e[0].remove_from_group(0, &carol).unwrap();
    pump(&mut e, |e| {
        e[2].data.groups[0].left
            && e[0].data.groups[0].members().len() == 2
            && e[1].data.groups[0].members().len() == 2
    })
    .await;
    e[1].send_group(0, "just us now").unwrap();
    pump(&mut e, |e| said(&e[0], 0, "just us now")).await;
    settle(&mut e).await;
    assert!(!said(&e[2], 0, "just us now"));

    // Bob leaves; Alice is told, and her next message no longer reaches him.
    e[1].leave_group(0).unwrap();
    pump(&mut e, |e| said(&e[0], 0, "bob left the group")).await;
    pump(&mut e, |e| e[0].data.groups[0].members().len() == 1).await;
    e[0].send_group(0, "anyone?").unwrap();
    settle(&mut e).await;
    assert!(!said(&e[1], 0, "anyone?"));
    std::fs::remove_dir_all(&dir).unwrap();
}
