//! Several admins in a group through a real relay (ADR-042): the main
//! admin makes a helper, the helper invites a stranger to the main admin,
//! the main admin moves everyone, and the role passes on.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termchat_engine::engine::Link;
use termchat_engine::{Engine, Event, Setup};
use termchat_proto::{
    AdminList, Body, Envelope, OLM_TYPE_MEGOLM, ReaderGrant, Roster, ServerFrame,
};
use termchat_relay::{Config, Limits};
use tokio::net::TcpListener;

const PASSWORD: &str = "correct horse battery staple violin";
const ALICE: usize = 0;
const BOB: usize = 1;
const CAROL: usize = 2;
const DAVE: usize = 3;

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
    let dir = std::env::temp_dir().join(format!("termchat-admins-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e: Vec<Box<Engine>> = ["alice", "bob", "carol", "dave"]
        .iter()
        .map(|n| engine(&dir, n, &url))
        .collect();
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    (e, url, dir)
}

async fn pump(e: &mut [Box<Engine>], done: impl Fn(&[Box<Engine>]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while !done(e) {
        if Instant::now() >= deadline {
            for (n, x) in e.iter().enumerate() {
                for g in &x.data.groups {
                    eprintln!(
                        "DIAG {n}: send={:?} members={} admins={} token={} moving={} left={} msgs={:?}",
                        &g.send_id[..2],
                        g.members().len(),
                        g.admin_keys().len(),
                        g.is_admin(),
                        g.moving.is_some(),
                        g.left,
                        g.messages
                            .iter()
                            .map(|m| m.text.clone())
                            .collect::<Vec<_>>()
                    );
                }
            }
            panic!("timed out");
        }
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

async fn settle(e: &mut [Box<Engine>]) {
    let until = Instant::now() + Duration::from_secs(1);
    pump(e, |_| Instant::now() > until).await;
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

fn said(e: &Engine, text: &str) -> bool {
    e.data
        .groups
        .first()
        .is_some_and(|g| g.messages.iter().any(|m| m.text == text))
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

/// `who` invites `invitee` (their contact) and `invitee` accepts.
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

/// `body` from `from`'s Megolm session straight into `to`'s group reader,
/// as the relay would hand it over: a forgery `from` could send.
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

#[tokio::test(flavor = "multi_thread")]
async fn a_helper_invites_a_stranger_who_follows_the_main_admins_move() {
    let (mut e, url, dir) = world("helper").await;
    group_with_helper(&mut e).await;
    // Dave is Bob's contact only.
    befriend(&mut e, DAVE, BOB).await;
    assert!(e[ALICE].index(&e[DAVE].me()).is_none());
    invite(&mut e, BOB, DAVE).await;
    pump(&mut e, |e| {
        members(&e[ALICE]) == 4 && members(&e[CAROL]) == 4
    })
    .await;
    e[DAVE].send_group(0, "hi from dave").unwrap();
    e[ALICE].send_group(0, "hi all").unwrap();
    pump(&mut e, |e| {
        said(&e[ALICE], "hi from dave") && said(&e[DAVE], "hi all")
    })
    .await;

    // A helper only invites (ADR-042, section 1).
    let (carol, dave) = (e[CAROL].me(), e[DAVE].me());
    assert!(e[BOB].remove_from_group(0, &carol).is_err());
    assert!(e[BOB].set_group_timer(0, Some(60)).is_err());
    assert!(e[BOB].make_admin(0, &dave).is_err());

    // Nor can Bob do it by hand: a roster without Carol, a list with Dave,
    // a new name, each signed by him, are refused by Carol herself.
    let current = e[BOB].data.groups[0].roster.clone().unwrap();
    let forged = |e: &[Box<Engine>], f: &dyn Fn(&mut Roster)| {
        let mut r = current.clone();
        r.no += 1;
        r.admin = e[BOB].me();
        f(&mut r);
        r.signature = e[BOB].core().sign_roster(&r.signed_bytes());
        r
    };
    let gid = current.group;
    let mut list = AdminList {
        group: gid,
        no: current.no + 1,
        signer: e[BOB].me(),
        admins: vec![e[ALICE].me(), e[BOB].me(), dave],
        resign: None,
        signature: [0; 64],
    };
    list.signature = e[BOB].core().sign_admins(&list);
    let without_carol = forged(&e, &|r| r.members.retain(|m| m.ed25519 != carol));
    let renamed = forged(&e, &|r| r.name = "mine now".into());
    let timer = forged(&e, &|r| r.expires_in = Some(1));
    let plain = forged(&e, &|_| {});
    for body in [
        Body::GroupRoster {
            roster: without_carol,
        },
        Body::GroupRoster { roster: renamed },
        Body::GroupRoster { roster: timer },
        Body::GroupAdmins {
            group: gid,
            chain: vec![list],
        },
        Body::GroupRoster { roster: plain },
    ] {
        inject(&mut e, &url, BOB, CAROL, body);
    }
    let carols = e[CAROL].data.groups[0].roster.clone().unwrap();
    assert_eq!(carols.name, "family");
    assert_eq!(carols.expires_in, None);
    assert!(carols.members.iter().any(|m| m.ed25519 == carol));
    assert!(
        !admins(&e[CAROL]).contains(&dave),
        "a helper makes no admins"
    );

    // A2: a helper cannot take the group away or send anyone off it.
    let queue = e[ALICE].data.groups[0].send_id;
    let away = forged(&e, &|_| {});
    let grant = ReaderGrant {
        recv_id: [1; 16],
        send_id: [2; 16],
        token: [3; 32],
    };
    for body in [
        Body::GroupMove {
            roster: away,
            send_id: [9; 16],
            reader: grant,
        },
        Body::GroupLeave { group: gid },
    ] {
        over_chat(&mut e, &url, BOB, ALICE, body);
    }
    assert_eq!(e[ALICE].data.groups[0].send_id, queue);
    assert!(!e[ALICE].data.groups[0].left);

    // The main admin removes Carol: the group moves, and Dave, no contact
    // of Alice's, hears of it through his reader in the old queue.
    e[ALICE].remove_from_group(0, &carol).unwrap();
    pump(&mut e, |e| {
        let sent = e[ALICE].data.groups[0].send_id;
        e[CAROL].data.groups[0].left
            && e[ALICE].data.groups[0].moving.is_none()
            && [BOB, DAVE]
                .iter()
                .all(|&x| e[x].data.groups[0].send_id == sent && members(&e[x]) == 3)
    })
    .await;
    e[ALICE].send_group(0, "after the move").unwrap();
    pump(&mut e, |e| {
        said(&e[DAVE], "after the move") && said(&e[BOB], "after the move")
    })
    .await;
    // Bob got the new queue's token; the old queue goes once Dave spoke.
    pump(&mut e, |e| e[BOB].data.groups[0].is_admin()).await;
    e[DAVE].send_group(0, "still here").unwrap();
    pump(&mut e, |e| {
        said(&e[ALICE], "still here") && e[ALICE].data.groups[0].retiring.is_none()
    })
    .await;
    assert!(!said(&e[CAROL], "after the move"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_main_admin_leaves_and_the_helper_takes_over() {
    let (mut e, _url, dir) = world("leave").await;
    group_with_helper(&mut e).await;
    let bob = e[BOB].me();
    e[ALICE].leave_group(0).unwrap();
    pump(&mut e, |e| {
        [BOB, CAROL].iter().all(|&x| {
            admins(&e[x]) == vec![bob] && members(&e[x]) == 2 && !e[x].data.groups[0].left
        }) && e[BOB].data.groups[0].moving.is_none()
            && e[CAROL].data.groups[0].send_id == e[BOB].data.groups[0].send_id
    })
    .await;
    e[BOB].send_group(0, "bob runs it").unwrap();
    pump(&mut e, |e| said(&e[CAROL], "bob runs it")).await;
    // The new main admin names the group's timer.
    e[BOB].set_group_timer(0, Some(600)).unwrap();
    pump(&mut e, |e| e[CAROL].data.groups[0].timer() == Some(600)).await;
    assert!(!said(&e[ALICE], "bob runs it"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hand_over_to_a_member_and_a_helper_who_steps_down() {
    let (mut e, _url, dir) = world("hand").await;
    group_with_helper(&mut e).await;
    let (alice, bob, carol) = (e[ALICE].me(), e[BOB].me(), e[CAROL].me());

    // Bob steps down; Alice moves the group so his token opens nothing.
    let before = e[ALICE].data.groups[0].send_id;
    e[BOB].step_down(0).unwrap();
    assert!(!e[BOB].data.groups[0].is_admin());
    pump(&mut e, |e| {
        let sent = e[ALICE].data.groups[0].send_id;
        sent != before
            && [BOB, CAROL]
                .iter()
                .all(|&x| admins(&e[x]) == vec![alice] && e[x].data.groups[0].send_id == sent)
    })
    .await;

    // Alice hands the group to Carol, who was no helper, and stays; Carol
    // moves it, so the token Alice knew opens nothing.
    let before = e[ALICE].data.groups[0].send_id;
    e[ALICE].hand_over_group(0, &carol, false).unwrap();
    pump(&mut e, |e| {
        let sent = e[CAROL].data.groups[0].send_id;
        sent != before
            && [ALICE, BOB, CAROL].iter().all(|&x| {
                admins(&e[x]) == vec![carol]
                    && members(&e[x]) == 3
                    && e[x].data.groups[0].send_id == sent
            })
            && e[CAROL].data.groups[0].is_admin()
            && e[CAROL].data.groups[0].moving.is_none()
    })
    .await;
    assert!(!e[ALICE].data.groups[0].is_admin());
    assert!(e[ALICE].remove_from_group(0, &bob).is_err());
    assert!(e[ALICE].set_group_timer(0, Some(60)).is_err());
    e[CAROL].send_group(0, "carol's now").unwrap();
    pump(&mut e, |e| {
        said(&e[ALICE], "carol's now") && said(&e[BOB], "carol's now")
    })
    .await;
    settle(&mut e).await;
    std::fs::remove_dir_all(&dir).unwrap();
}
