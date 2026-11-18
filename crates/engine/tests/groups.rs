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

#[tokio::test(flavor = "multi_thread")]
async fn the_admins_timer_holds_for_all_and_a_pending_invite_survives_a_move() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-groups2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = [
        engine(&dir, "alice", &url),
        engine(&dir, "bob", &url),
        engine(&dir, "carol", &url),
    ];
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    settle(&mut e).await;
    befriend(&mut e, 1, 0).await;
    befriend(&mut e, 2, 0).await;
    let g = e[0].create_group("team").unwrap();
    pump(&mut e, |e| e[0].data.groups[0].reader.is_some()).await;
    for who in [1, 2] {
        let i = e[0].index(&e[who].me()).unwrap();
        e[0].invite_to_group(g, i).unwrap();
    }
    pump(&mut e, |e| {
        e[1].data.group_invites.len() == 1 && e[2].data.group_invites.len() == 1
    })
    .await;
    // Bob joins; Carol waits. Then Bob is removed: the group moves, and
    // Carol's invite is made again for the new queue.
    e[1].accept_group(0).unwrap();
    pump(&mut e, |e| e[0].data.groups[0].members().len() == 2).await;
    let bob = e[1].me();
    e[0].remove_from_group(0, &bob).unwrap();
    pump(&mut e, |e| {
        e[1].data.groups[0].left && e[0].data.groups[0].moving.is_none()
    })
    .await;
    pump(&mut e, |e| {
        e[2].data.group_invites.len() == 1 && e[0].data.groups[0].invited.len() == 1
    })
    .await;
    e[2].accept_group(0).unwrap();
    pump(&mut e, |e| {
        e[2].data
            .groups
            .first()
            .is_some_and(|g| g.members().len() == 2)
            && e[0].data.groups[0].members().len() == 2
    })
    .await;

    // The admin's timer reaches the member and holds for what they send.
    e[0].set_group_timer(0, Some(3600)).unwrap();
    pump(&mut e, |e| e[2].data.groups[0].timer() == Some(3600)).await;
    assert!(
        e[2].set_group_timer(0, None).is_err(),
        "only the admin sets it"
    );
    e[2].send_group(0, "for an hour").unwrap();
    pump(&mut e, |e| said(&e[0], 0, "for an hour")).await;
    let m = e[0].data.groups[0]
        .messages
        .iter()
        .find(|m| m.text == "for an hour")
        .unwrap();
    assert!(m.expires_at.is_some());

    // After a restart the group still talks, and a goodbye right away arrives.
    e[2].persist();
    let state = dir.join("carol");
    e[2] = match Engine::unlock(&state, PASSWORD.as_bytes()).unwrap() {
        termchat_engine::Unlocked::Open(x) => x,
        _ => panic!("carol's state"),
    };
    e[2].leave_group(0).unwrap();
    pump(&mut e, |e| said(&e[0], 0, "carol left the group")).await;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Like `pump`, but only the engines marked in `on` run, for `secs`.
async fn pump_only(e: &mut [Box<Engine>; 3], on: [bool; 3], secs: u64) {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        let [a, b, c] = e;
        tokio::select! {
            Some(ev) = a.events.recv(), if on[0] => a.on_relay(ev),
            Some(ev) = b.events.recv(), if on[1] => b.on_relay(ev),
            Some(ev) = c.events.recv(), if on[2] => c.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        for (x, run) in e.iter_mut().zip(on) {
            if run {
                x.tick();
                x.happened.clear();
            }
        }
    }
}

/// A new member's key can reach a member over their chat before the admin's
/// roster that names the newcomer. It must wait for the roster, not be lost.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_that_comes_before_the_roster_is_kept() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-groups3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = [
        engine(&dir, "alice", &url),
        engine(&dir, "bob", &url),
        engine(&dir, "carol", &url),
    ];
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    settle(&mut e).await;
    // Everyone knows everyone: Carol's key goes to Bob over their chat.
    befriend(&mut e, 1, 0).await;
    befriend(&mut e, 2, 0).await;
    befriend(&mut e, 2, 1).await;

    let g = e[0].create_group("trip").unwrap();
    pump(&mut e, |e| e[0].data.groups[0].reader.is_some()).await;
    let i = e[0].index(&e[1].me()).unwrap();
    e[0].invite_to_group(g, i).unwrap();
    pump(&mut e, |e| e[1].data.group_invites.len() == 1).await;
    e[1].accept_group(0).unwrap();
    pump(&mut e, |e| {
        e[..2].iter().all(|x| {
            x.data
                .groups
                .first()
                .is_some_and(|g| g.members().len() == 2)
        })
    })
    .await;

    let i = e[0].index(&e[2].me()).unwrap();
    e[0].invite_to_group(g, i).unwrap();
    pump(&mut e, |e| e[2].data.group_invites.len() == 1).await;
    e[2].accept_group(0).unwrap();
    e[2].send_group(0, "hi, I'm in").unwrap();
    // The admin is slow: Bob hears from Carol before the new roster.
    pump_only(&mut e, [false, true, true], 2).await;
    pump(&mut e, |e| said(&e[1], 0, "hi, I'm in")).await;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A contact who just added our invite says what their app reads only once
/// our acceptance reaches them. An invite to a group in between waits for
/// that instead of calling their app old.
#[tokio::test(flavor = "multi_thread")]
async fn an_invite_right_after_accepting_waits_for_their_level() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-groups4-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = [
        engine(&dir, "alice", &url),
        engine(&dir, "bob", &url),
        engine(&dir, "carol", &url),
    ];
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    settle(&mut e).await;
    let g = e[0].create_group("now").unwrap();
    pump(&mut e, |e| e[0].data.groups[0].reader.is_some()).await;

    // Bob adds Alice's invite; Alice accepts and invites him at once.
    let invite = e[0].invite().unwrap();
    e[1].add(&invite, false).unwrap();
    pump(&mut e, |e| {
        e[0].data
            .contacts
            .iter()
            .any(|c| e[0].is_request(&c.key) && c.theirs.is_some())
    })
    .await;
    let i = e[0].index(&e[1].me()).unwrap();
    e[0].accept(i);
    assert_eq!(e[0].data.contacts[i].understands, 0);
    e[0].invite_to_group(g, i).unwrap();
    pump(&mut e, |e| e[1].data.group_invites.len() == 1).await;
    std::fs::remove_dir_all(&dir).unwrap();
}
