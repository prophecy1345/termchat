//! Proofs of concept for review-2026-11d.md, findings 1 and 2. Each test
//! states the safe behaviour and fails on `b5a49c8` (0.28.0). To run them,
//! copy this file to `crates/engine/tests/review_2026_11d.rs` and run
//! `cargo test -p termchat-engine --test review_2026_11d -- --test-threads=1`.
//! The harness is the one of `crates/engine/tests/admins.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termchat_engine::engine::Link;
use termchat_engine::{Engine, Event, Setup};
use termchat_proto::{AdminGrant, Body, Envelope, OLM_TYPE_MEGOLM, Roster, ServerFrame};
use termchat_relay::{Config, Limits};
use tokio::net::TcpListener;

const PASSWORD: &str = "correct horse battery staple violin";
const ALICE: usize = 0;
const BOB: usize = 1;
const CAROL: usize = 2;

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

async fn world(name: &str) -> (Vec<Box<Engine>>, String, PathBuf) {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-11d-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e: Vec<Box<Engine>> = ["alice", "bob", "carol"]
        .iter()
        .map(|n| engine(&dir, n, &url))
        .collect();
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    (e, url, dir)
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
        let rich = |x: &Engine, k| {
            x.index(&k)
                .is_some_and(|i| x.rich(i) && x.data.contacts[i].understands >= 6)
        };
        rich(&e[a], kb) && rich(&e[b], ka)
    })
    .await;
}

fn admins(e: &Engine) -> Vec<[u8; 32]> {
    e.data
        .groups
        .first()
        .map(|g| g.admin_keys())
        .unwrap_or_default()
}

fn members(e: &Engine) -> usize {
    e.data.groups.first().map_or(0, |g| g.members().len())
}

async fn invite(e: &mut [Box<Engine>], who: usize, invitee: usize) {
    let n = members(&e[who]);
    let i = e[who].index(&e[invitee].me()).unwrap();
    e[who].invite_to_group(0, i).unwrap();
    pump(e, |e| e[invitee].data.group_invites.len() == 1).await;
    e[invitee].accept_group(0).unwrap();
    pump(e, |e| {
        members(&e[who]) == n + 1 && members(&e[invitee]) == n + 1
    })
    .await;
}

/// Alice's group with Bob and Carol, Bob her helper.
async fn group_with_helper(e: &mut [Box<Engine>]) {
    befriend(e, BOB, ALICE).await;
    befriend(e, CAROL, ALICE).await;
    e[ALICE].create_group("family").unwrap();
    pump(e, |e| e[ALICE].data.groups[0].reader.is_some()).await;
    invite(e, ALICE, BOB).await;
    invite(e, ALICE, CAROL).await;
    let bob = e[BOB].me();
    e[ALICE].make_admin(0, &bob).unwrap();
    let want = vec![e[ALICE].me(), bob];
    pump(e, |e| {
        [ALICE, BOB, CAROL].iter().all(|&x| admins(&e[x]) == want)
            && e[BOB].data.groups[0].is_admin()
    })
    .await;
}

/// `body` from `from`'s Megolm session straight into `to`'s group reader.
fn inject(e: &mut [Box<Engine>], url: &str, from: usize, to: usize, body: Body) {
    let gid = e[to].data.groups[0].id;
    let bytes = e[from].core().group_encrypt(gid, body).unwrap();
    let recv_id = e[to].data.groups[0].reader.as_ref().unwrap().recv_id;
    let mut who = [0xff; 32];
    who[..16].copy_from_slice(&gid);
    let link = Link {
        who,
        relay: url.to_owned(),
    };
    let frame = ServerFrame::Deliver {
        recv_id,
        relay_id: u64::MAX - 1,
        envelope: Envelope {
            version: termchat_proto::VERSION,
            olm_type: OLM_TYPE_MEGOLM,
            body: bytes,
        },
    };
    e[to].on_relay((link, Event::Frame(frame)));
}

/// `body` from contact `from` over their chat into `to`'s queue.
fn over_chat(e: &mut [Box<Engine>], url: &str, from: usize, to: usize, body: Body) {
    let (them, me) = (e[to].me(), e[from].me());
    let envelope = e[from].core().encrypt(&them, body).unwrap();
    let i = e[to].index(&me).unwrap();
    let recv_id = e[to].data.contacts[i].mine.as_ref().unwrap().recv_id;
    let link = Link {
        who: me,
        relay: url.to_owned(),
    };
    let frame = ServerFrame::Deliver {
        recv_id,
        relay_id: u64::MAX - 2,
        envelope,
    };
    e[to].on_relay((link, Event::Frame(frame)));
}

fn signed_by_bob(e: &[Box<Engine>], change: &dyn Fn(&mut Roster)) -> Roster {
    let mut r = e[BOB].data.groups[0].roster.clone().unwrap();
    r.no += 1;
    r.admin = e[BOB].me();
    change(&mut r);
    r.signature = e[BOB].core().sign_roster(&r.signed_bytes());
    r
}

/// Finding 1: whoever signed the last admin list stays a "giver" of the
/// queue token for good, also when that list took their own role away.
#[tokio::test(flavor = "multi_thread")]
async fn finding_1_a_former_helper_cannot_replace_the_main_admins_token() {
    let (mut e, url, dir) = world("token").await;
    group_with_helper(&mut e).await;
    let alice = e[ALICE].me();
    let before = e[ALICE].data.groups[0].send_id;
    e[BOB].step_down(0).unwrap();
    pump(&mut e, |e| {
        let g = &e[ALICE].data.groups[0];
        g.send_id != before
            && g.moving.is_none()
            && admins(&e[ALICE]) == vec![alice]
            && e[BOB].data.groups[0].send_id == g.send_id
    })
    .await;
    assert!(!e[BOB].data.groups[0].is_admin());
    let real = e[ALICE].data.groups[0].admin;
    let gid = e[ALICE].data.groups[0].id;
    let forged = Body::GroupAdminKeys {
        group: gid,
        roster_no: 0,
        grant: AdminGrant {
            queue: [0xaa; 16],
            token: [0xbb; 32],
        },
    };
    over_chat(&mut e, &url, BOB, ALICE, forged);
    assert!(
        e[ALICE].data.groups[0].admin == real,
        "a member who is no admin replaced the main admin's queue token"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Finding 2: a helper's roster may only add members, yet nothing bounds
/// its number: after `u64::MAX` no roster can follow, from anyone.
#[tokio::test(flavor = "multi_thread")]
async fn finding_2_a_helper_cannot_take_the_last_roster_number() {
    let (mut e, url, dir) = world("number").await;
    group_with_helper(&mut e).await;
    let last = signed_by_bob(&e, &|r| r.no = u64::MAX);
    for to in [CAROL, ALICE] {
        inject(
            &mut e,
            &url,
            BOB,
            to,
            Body::GroupRoster {
                roster: last.clone(),
            },
        );
    }
    for who in [CAROL, ALICE] {
        let no = e[who].data.groups[0].roster.as_ref().unwrap().no;
        assert!(no < u64::MAX, "the group can never change again");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Finding 2: nor may it name a key twice. The main admin cannot remove an
/// entry under their own key, and 32 entries make the group full for good.
#[tokio::test(flavor = "multi_thread")]
async fn finding_2_a_helper_cannot_list_a_member_twice() {
    let (mut e, url, dir) = world("twice").await;
    group_with_helper(&mut e).await;
    let alice = e[ALICE].me();
    let doubled = signed_by_bob(&e, &|r| {
        let mut again = r
            .members
            .iter()
            .find(|m| m.ed25519 == alice)
            .cloned()
            .unwrap();
        again.reader = [9; 16];
        r.members.push(again);
    });
    for to in [CAROL, ALICE] {
        inject(
            &mut e,
            &url,
            BOB,
            to,
            Body::GroupRoster {
                roster: doubled.clone(),
            },
        );
    }
    for who in [CAROL, ALICE] {
        assert_eq!(members(&e[who]), 3, "one key, two places on the roster");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Finding 4: a Megolm session is found by its id alone, so a member who
/// passes on someone else's key as their own gets that session filed under
/// their name, and the real owner's messages are then refused.
#[test]
fn finding_4_a_session_is_not_filed_under_whoever_brings_its_key_first() {
    use termchat_core::Client;
    const G: [u8; 16] = [7; 16];
    let mut alice = Client::default();
    let mallory = Client::default();
    let mut victor = Client::default();
    let key = alice.group_start(G, 0).unwrap();
    victor.group_start(G, 0).unwrap();
    // Mallory, a member, got Alice's key like everyone and sends it on first.
    victor
        .group_member_key(&G, mallory.identity().ed25519, &key)
        .unwrap();
    victor
        .group_member_key(&G, alice.identity().ed25519, &key)
        .unwrap();
    let body = Body::Text {
        sent_at: 0,
        expires_in: None,
        text: "hi".into(),
    };
    let bytes = alice.group_encrypt(G, body).unwrap();
    assert_eq!(
        victor.group_decrypt(&G, &bytes).map(|(from, _)| from),
        Ok(alice.identity().ed25519)
    );
}
