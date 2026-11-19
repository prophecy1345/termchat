//! Linking a second device (ADR-028) through a real relay: the link code,
//! the fingerprint on both screens, and the list the primary signs.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::Path;
use std::time::{Duration, Instant};

use termchat_engine::{Engine, Setup};
use termchat_relay::{Config, Limits, Rate};
use tokio::net::TcpListener;

const PASSWORD: &str = "correct horse battery staple violin";

async fn start_relay() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let config = Config {
        trust_proxy: false,
        db: None,
        reports: None,
        // Every device here comes from 127.0.0.1: one IP's queue budget is
        // too small for four devices, their contacts and group moves.
        limits: Limits {
            queues_per_ip: Rate::per(1000.0, 3600.0),
            ..Limits::default()
        },
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
        if Instant::now() >= deadline {
            for (n, x) in e.iter().enumerate() {
                for g in &x.data.groups {
                    let texts: Vec<_> = g
                        .messages
                        .iter()
                        .map(|m| (m.text.as_str(), m.mine, m.author.map(|a| a[0])))
                        .collect();
                    let devs: Vec<_> = g.devices().iter().map(|d| d.ed25519[0]).collect();
                    eprintln!(
                        "#{n} left {} moving {:?} asking {} me {} devices {devs:?} shared {:?} early {} msgs {texts:?}",
                        g.left,
                        g.moving
                            .as_ref()
                            .map(|m| (m.stay.len(), m.grants.len(), m.removed.len())),
                        g.asking.len(),
                        x.me()[0],
                        g.shared.iter().map(|k| k[0]).collect::<Vec<_>>(),
                        g.early.len()
                    );
                }
            }
            panic!("timed out");
        }
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
    // ADR-030: every device on the list proved itself.
    termchat_core::verify_devices(&list).unwrap();

    // A state from before ADR-030: the list's devices carry no proofs. On
    // the next unlock the phone keeps itself, and the laptop must link again.
    let mut old = list.clone();
    old.devices.iter_mut().for_each(|d| d.proof = [0; 64]);
    old.signature = e[phone].core().sign_devices(&old).unwrap();
    e[phone].data.devices = Some(old);
    assert!(e[phone].persist());
    let phone_dir = dir.join("alice");
    e.remove(phone);
    let reopened = match termchat_engine::Engine::unlock(&phone_dir, PASSWORD.as_bytes()).unwrap() {
        termchat_engine::Unlocked::Open(x) => x,
        termchat_engine::Unlocked::Wiped => panic!("wiped"),
    };
    let fresh = reopened.data.devices.clone().unwrap();
    assert_eq!(fresh.devices.len(), 1);
    termchat_core::verify_devices(&fresh).unwrap();
    assert!(reopened.data.contacts.iter().all(|c| !c.own));
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

/// `a` adds `b`'s invite, `b` accepts; both apps read everything.
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
        let knows = |x: &Engine, k| {
            x.index(&k).is_some_and(|i| {
                x.data.contacts[i].understands >= termchat_core::proto::UNDERSTANDS
            })
        };
        knows(&e[a], kb) && knows(&e[b], ka)
    })
    .await;
}

/// Phone and laptop linked; bob a contact of the phone from before.
async fn linked_with_bob(dir: &Path, url: &str) -> Vec<Box<Engine>> {
    let mut e = vec![
        engine(dir, "alice", url),
        engine(dir, "new", url),
        engine(dir, "bob", url),
    ];
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    befriend(&mut e, 0, 2).await;
    let code = e[1].link_code().unwrap();
    let at_phone = e[0].add_device(&code).unwrap();
    pump(&mut e, |e| e[1].pending_link().is_some()).await;
    let at_laptop = e[1].pending_link().unwrap();
    e[1].confirm_link(at_laptop);
    e[0].confirm_link(at_phone);
    pump(&mut e, |e| e[1].data.devices.is_some()).await;
    e
}

#[tokio::test(flavor = "multi_thread")]
async fn the_new_device_gets_the_contacts_and_both_know_each_others_queues() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let (phone, laptop, bob) = (e[0].me(), e[1].me(), e[2].me());

    pump(&mut e, |e| {
        e[1].index(&bob).is_some_and(|i| {
            let c = &e[1].data.contacts[i];
            c.mine.is_some() && c.siblings.iter().any(|(k, _)| *k == phone)
        })
    })
    .await;
    let i = e[1].index(&bob).unwrap();
    let c = &e[1].data.contacts[i];
    assert!(e[1].name(c).starts_with("bob"), "{}", e[1].name(c));
    assert!(c.theirs.is_some() && !c.own);
    // The phone learns where the laptop reads bob.
    pump(&mut e, |e| {
        let i = e[0].index(&bob).unwrap();
        e[0].data.contacts[i]
            .siblings
            .iter()
            .any(|(k, _)| *k == laptop)
    })
    .await;
    // Bob heard the list from both devices.
    pump(&mut e, |e| {
        let i = e[0].index(&bob).unwrap();
        let j = e[1].index(&bob).unwrap();
        e[0].data.contacts[i].devices_told == 1 && e[1].data.contacts[j].devices_told == 1
    })
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
}

fn has(e: &Engine, contact: &[u8; 32], text: &str, mine: bool) -> bool {
    e.index(contact).is_some_and(|i| {
        e.data.contacts[i]
            .messages
            .iter()
            .any(|m| m.text == text && m.mine == mine)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contact_writes_to_both_devices_and_each_device_sees_what_the_other_sent() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices4-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let (alice, bob) = (e[0].me(), e[2].me());

    // Bob learns the laptop and where it reads him.
    pump(&mut e, |e| {
        let i = e[2].index(&alice).unwrap();
        let c = &e[2].data.contacts[i];
        c.their_devices
            .as_ref()
            .is_some_and(|d| d.devices.len() == 2)
            && c.device_addrs.len() == 1
    })
    .await;

    let i = e[2].index(&alice).unwrap();
    e[2].send(i, "hi alice").unwrap();
    pump(&mut e, |e| {
        has(&e[0], &bob, "hi alice", false) && has(&e[1], &bob, "hi alice", false)
    })
    .await;

    let j = e[1].index(&bob).unwrap();
    e[1].send(j, "from the laptop").unwrap();
    pump(&mut e, |e| {
        has(&e[2], &alice, "from the laptop", false) && has(&e[0], &bob, "from the laptop", true)
    })
    .await;

    let k = e[0].index(&bob).unwrap();
    e[0].send(k, "from the phone").unwrap();
    pump(&mut e, |e| {
        has(&e[2], &alice, "from the phone", false) && has(&e[1], &bob, "from the phone", true)
    })
    .await;

    // An edit made on the laptop reaches bob and the phone.
    let m = e[1].data.contacts[j]
        .messages
        .iter()
        .position(|m| m.text == "from the laptop")
        .unwrap();
    e[1].edit_message(j, m, "from the laptop, fixed").unwrap();
    pump(&mut e, |e| {
        has(&e[2], &alice, "from the laptop, fixed", false)
            && has(&e[0], &bob, "from the laptop, fixed", true)
    })
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_device_hears_so_and_contacts_stop_writing_to_it() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices5-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let (alice, laptop, bob) = (e[0].me(), e[1].me(), e[2].me());
    pump(&mut e, |e| {
        let i = e[2].index(&alice).unwrap();
        e[2].data.contacts[i].device_addrs.len() == 1
    })
    .await;

    assert!(e[1].remove_device(&laptop).is_err(), "only the primary");
    assert!(
        e[0].remove_device(&alice).is_err(),
        "not the primary itself"
    );
    e[0].remove_device(&laptop).unwrap();
    pump(&mut e, |e| {
        let i = e[2].index(&alice).unwrap();
        let c = &e[2].data.contacts[i];
        e[1].removed()
            && c.their_devices
                .as_ref()
                .is_some_and(|d| d.devices.len() == 1)
            && c.device_addrs.is_empty()
    })
    .await;

    let j = e[1].index(&bob).unwrap();
    assert!(e[1].send(j, "still here?").is_err());
    let i = e[2].index(&alice).unwrap();
    e[2].send(i, "after").unwrap();
    pump(&mut e, |e| has(&e[0], &bob, "after", false)).await;
    settle(&mut e, 1500).await;
    assert!(
        !has(&e[1], &bob, "after", false),
        "the removed laptop reads nothing new"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn labels_reach_our_devices_and_only_the_primary_sets_them() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices6-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let laptop = e[1].me();
    assert!(e[1].set_device_label(&laptop, "laptop").is_err());
    assert!(e[0].set_device_label(&laptop, "a\u{1b}[2J").is_err());
    e[0].set_device_label(&laptop, "laptop").unwrap();
    pump(&mut e, |e| {
        e[1].device_rows()
            .iter()
            .any(|(k, l)| *k == laptop && l == "laptop")
    })
    .await;
    e[0].set_device_label(&laptop, " ").unwrap();
    pump(&mut e, |e| {
        e[1].device_rows().iter().all(|(_, l)| l.is_empty())
    })
    .await;
    // Bob never hears a label: it is not in what contacts get.
    let i = e[2].index(&e[0].me()).unwrap();
    assert!(e[2].data.contacts[i].their_devices.is_some());
    assert!(e[2].data.device_labels.is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

fn has_file(e: &Engine, contact: &[u8; 32], name: &str, mine: bool) -> bool {
    e.index(contact).is_some_and(|i| {
        e.data.contacts[i].messages.iter().any(|m| {
            m.mine == mine
                && m.file
                    .as_ref()
                    .is_some_and(|f| f.name == name && f.complete())
        })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn documents_reach_every_device_and_ours_see_what_we_sent() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices7-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let (alice, bob) = (e[0].me(), e[2].me());
    pump(&mut e, |e| {
        let i = e[2].index(&alice).unwrap();
        e[2].data.contacts[i].device_addrs.len() == 1
    })
    .await;

    // Two chunks and a bit, to both of alice's devices.
    let i = e[2].index(&alice).unwrap();
    e[2].send_file(i, "notes.txt", vec![7u8; 33_000]).unwrap();
    pump(&mut e, |e| {
        has_file(&e[0], &bob, "notes.txt", false) && has_file(&e[1], &bob, "notes.txt", false)
    })
    .await;

    // From the laptop: bob gets it whole, the phone sees it as sent.
    let j = e[1].index(&bob).unwrap();
    e[1].send_file(j, "plan.pdf", vec![1u8; 20_000]).unwrap();
    pump(&mut e, |e| {
        has_file(&e[2], &alice, "plan.pdf", false) && has_file(&e[0], &bob, "plan.pdf", true)
    })
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_devices_meet_each_other_and_see_each_others_copies() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices8-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    e.push(engine(&dir, "tablet", &url));
    pump(&mut e, |e| e[3].data.invite_queue.is_some()).await;
    let code = e[3].link_code().unwrap();
    let at_phone = e[0].add_device(&code).unwrap();
    pump(&mut e, |e| e[3].pending_link().is_some()).await;
    let at_tablet = e[3].pending_link().unwrap();
    e[3].confirm_link(at_tablet);
    e[0].confirm_link(at_phone);
    pump(&mut e, |e| {
        e[3].data
            .devices
            .as_ref()
            .is_some_and(|d| d.devices.len() == 3)
    })
    .await;

    // Laptop and tablet find each other through the phone.
    let (laptop, tablet, bob) = (e[1].me(), e[3].me(), e[2].me());
    pump(&mut e, |e| {
        let met = |x: &Engine, k| {
            x.index(&k)
                .is_some_and(|i| x.data.contacts[i].theirs.is_some())
        };
        met(&e[1], tablet) && met(&e[3], laptop)
    })
    .await;
    // And each learns where the other reads bob.
    pump(&mut e, |e| {
        let knows = |x: &Engine, k| {
            x.index(&bob)
                .is_some_and(|i| x.data.contacts[i].siblings.iter().any(|(s, _)| *s == k))
        };
        knows(&e[1], tablet) && knows(&e[3], laptop)
    })
    .await;
    let j = e[1].index(&bob).unwrap();
    e[1].send(j, "from the laptop, seen on the tablet").unwrap();
    pump(&mut e, |e| {
        has(&e[3], &bob, "from the laptop, seen on the tablet", true)
            && has(&e[0], &bob, "from the laptop, seen on the tablet", true)
    })
    .await;
    // None of our devices shows up as a person.
    assert!(
        e.iter()
            .take(2)
            .chain(e.iter().skip(3))
            .all(|x| x.data.people() == 1)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Bob makes a group and invites alice, who has a phone and a laptop: each
/// device gets a reader; alice accepts on the phone, the laptop follows.
async fn group_with_alice_on_two_devices(e: &mut [Box<Engine>]) -> usize {
    let alice = e[0].me();
    pump(e, |e| {
        let i = e[2].index(&alice).unwrap();
        e[2].data.contacts[i]
            .their_devices
            .as_ref()
            .is_some_and(|d| d.devices.len() == 2)
    })
    .await;
    let g = e[2].create_group("trip").unwrap();
    pump(e, |e| e[2].data.groups[0].reader.is_some()).await;
    let i = e[2].index(&alice).unwrap();
    e[2].invite_to_group(g, i).unwrap();
    pump(e, |e| e[0].data.group_invites.len() == 1).await;
    e[0].accept_group(0).unwrap();
    pump(e, |e| {
        e[1].data.groups.first().is_some_and(|g| g.reader.is_some())
    })
    .await;
    g
}

#[tokio::test(flavor = "multi_thread")]
async fn each_device_of_a_member_gets_a_reader_and_the_laptop_follows_the_phone() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices9-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    group_with_alice_on_two_devices(&mut e).await;
    let (alice, laptop) = (e[0].me(), e[1].me());
    assert!(
        e[1].data.group_invites.is_empty(),
        "the laptop took its invite itself"
    );
    // Bob and alice's phone know alice's devices and the laptop's reader.
    pump(&mut e, |e| {
        [&e[0], &e[2]].iter().all(|x| {
            x.data.groups.first().is_some_and(|g| {
                g.member_devices
                    .iter()
                    .any(|(m, _, r)| *m == alice && r.iter().any(|(d, _)| *d == laptop))
            })
        })
    })
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
}

fn in_group(e: &Engine, text: &str, mine: bool, author: Option<[u8; 32]>) -> bool {
    e.data.groups.first().is_some_and(|g| {
        g.messages
            .iter()
            .any(|m| m.text == text && m.mine == mine && m.author == author)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn every_device_of_a_member_reads_and_writes_in_the_group() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices10-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let g = group_with_alice_on_two_devices(&mut e).await;
    let (alice, bob) = (e[0].me(), e[2].me());

    e[2].send_group(g, "hi both of you").unwrap();
    pump(&mut e, |e| {
        in_group(&e[0], "hi both of you", false, Some(bob))
            && in_group(&e[1], "hi both of you", false, Some(bob))
    })
    .await;

    e[1].send_group(0, "from alice's laptop").unwrap();
    pump(&mut e, |e| {
        in_group(&e[2], "from alice's laptop", false, Some(alice))
            && in_group(&e[0], "from alice's laptop", true, None)
    })
    .await;

    // A contact's new device is not a restarted conversation.
    let restarted = |x: &Engine| {
        x.data
            .contacts
            .iter()
            .flat_map(|c| &c.messages)
            .any(|m| m.text.contains("restarted"))
    };
    assert!(!e.iter().any(|x| restarted(x)));
    e[0].send_group(0, "and from her phone").unwrap();
    pump(&mut e, |e| {
        in_group(&e[2], "and from her phone", false, Some(alice))
            && in_group(&e[1], "and from her phone", true, None)
    })
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Alice's phone links one more device, a tablet, as e[3].
#[allow(clippy::vec_box)] // the engines the other helpers make
async fn link_tablet(e: &mut Vec<Box<Engine>>, dir: &Path, url: &str) {
    e.push(engine(dir, "tablet", url));
    pump(e, |e| e[3].data.invite_queue.is_some()).await;
    let code = e[3].link_code().unwrap();
    let at_phone = e[0].add_device(&code).unwrap();
    pump(e, |e| e[3].pending_link().is_some()).await;
    let at_tablet = e[3].pending_link().unwrap();
    e[3].confirm_link(at_tablet);
    e[0].confirm_link(at_phone);
    pump(e, |e| {
        e[3].data
            .devices
            .as_ref()
            .is_some_and(|d| d.devices.len() == 3)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_linked_later_joins_and_a_removed_one_reads_no_more() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices11-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let g = group_with_alice_on_two_devices(&mut e).await;
    let (alice, bob, laptop) = (e[0].me(), e[2].me(), e[1].me());
    link_tablet(&mut e, &dir, &url).await;

    // The tablet joins by itself: alice is in the group already.
    pump(&mut e, |e| {
        e[3].data.groups.first().is_some_and(|g| g.reader.is_some())
    })
    .await;
    e[2].send_group(g, "welcome, tablet").unwrap();
    pump(&mut e, |e| {
        (0..4)
            .filter(|&n| n != 2)
            .all(|n| in_group(&e[n], "welcome, tablet", false, Some(bob)))
    })
    .await;
    e[3].send_group(0, "from the tablet").unwrap();
    pump(&mut e, |e| {
        in_group(&e[2], "from the tablet", false, Some(alice))
            && in_group(&e[1], "from the tablet", true, None)
    })
    .await;

    // The laptop is removed: the group moves without it.
    e[0].remove_device(&laptop).unwrap();
    pump(&mut e, |e| {
        e[2].data.groups[0].moving.is_none()
            && e[2].data.groups[0]
                .member_devices
                .iter()
                .all(|(_, l, _)| l.devices.iter().all(|d| d.ed25519 != laptop))
            && e[1].data.groups[0].left
    })
    .await;
    e[2].send_group(g, "after the laptop went").unwrap();
    pump(&mut e, |e| {
        in_group(&e[0], "after the laptop went", false, Some(bob))
            && in_group(&e[3], "after the laptop went", false, Some(bob))
    })
    .await;
    assert!(!in_group(&e[1], "after the laptop went", false, Some(bob)));

    // Bob removes alice: her devices leave too.
    e[2].remove_from_group(g, &alice).unwrap();
    pump(&mut e, |e| {
        e[0].data.groups[0].left && e[3].data.groups[0].left
    })
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_admins_own_linked_device_joins_its_groups() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices12-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let (alice, bob) = (e[0].me(), e[2].me());
    pump(&mut e, |e| {
        let i = e[2].index(&alice).unwrap();
        e[2].data.contacts[i].understands >= 3
            && e[0]
                .index(&bob)
                .is_some_and(|j| e[0].data.contacts[j].understands >= 3)
    })
    .await;

    // Alice makes a group on her phone; the laptop joins without asking.
    let g = e[0].create_group("ours").unwrap();
    pump(&mut e, |e| {
        e[1].data.groups.first().is_some_and(|g| g.reader.is_some())
    })
    .await;
    assert!(e[1].data.group_invites.is_empty());
    let i = e[0].index(&bob).unwrap();
    e[0].invite_to_group(g, i).unwrap();
    pump(&mut e, |e| e[2].data.group_invites.len() == 1).await;
    e[2].accept_group(0).unwrap();
    pump(&mut e, |e| e[1].data.groups[0].members().len() == 2).await;

    e[1].send_group(0, "from the admin's laptop").unwrap();
    pump(&mut e, |e| {
        in_group(&e[2], "from the admin's laptop", false, Some(alice))
            && in_group(&e[0], "from the admin's laptop", true, None)
    })
    .await;
    e[2].send_group(0, "bob to both").unwrap();
    pump(&mut e, |e| {
        in_group(&e[0], "bob to both", false, Some(bob))
            && in_group(&e[1], "bob to both", false, Some(bob))
    })
    .await;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Contacts made on the phone after the laptop was linked, either way round,
/// reach the laptop too, and their messages with them.
#[tokio::test(flavor = "multi_thread")]
async fn a_contact_made_after_linking_reaches_the_linked_device() {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices-late-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = vec![
        engine(&dir, "alice", &url),
        engine(&dir, "new", &url),
        engine(&dir, "bob", &url),
        engine(&dir, "carol", &url),
    ];
    pump(&mut e, |e| e.iter().all(|x| x.data.invite_queue.is_some())).await;
    let code = e[1].link_code().unwrap();
    let at_phone = e[0].add_device(&code).unwrap();
    pump(&mut e, |e| e[1].pending_link().is_some()).await;
    let at_laptop = e[1].pending_link().unwrap();
    e[1].confirm_link(at_laptop);
    e[0].confirm_link(at_phone);
    pump(&mut e, |e| e[1].data.devices.is_some()).await;

    // Bob asks the phone; the phone asks carol.
    befriend(&mut e, 2, 0).await;
    befriend(&mut e, 0, 3).await;
    let (alice, laptop) = (e[0].me(), e[1].me());
    for (who, text) in [(2, "hello"), (3, "hi")] {
        let them = e[who].me();
        // The laptop has them, and they have the laptop's address.
        pump(&mut e, |e| {
            let i = e[who].index(&alice).unwrap();
            e[1].index(&them).is_some()
                && e[who].data.contacts[i]
                    .device_addrs
                    .iter()
                    .any(|(k, _)| *k == laptop)
        })
        .await;
        let i = e[who].index(&alice).unwrap();
        e[who].send(i, text).unwrap();
        pump(&mut e, |e| has(&e[1], &them, text, false)).await;
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// ADR-030, section 4 (review 2026-11c, finding 4): a copy from our own device
/// is filed only under the chat it names, and only if it came in that chat's
/// queue; a bare one, as a relay could move it, is not filed at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_lands_only_in_the_chat_it_names() {
    use termchat_core::proto::{Body, ServerFrame};
    use termchat_engine::Event;
    use termchat_engine::engine::Link;

    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-devices-copy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = linked_with_bob(&dir, &url).await;
    let (laptop, bob) = (e[1].me(), e[2].me());
    pump(&mut e, |e| {
        e[1].index(&bob)
            .is_some_and(|i| e[1].data.contacts[i].mine.is_some())
            && e[0].core().has_session(&laptop)
    })
    .await;
    let i = e[1].index(&bob).unwrap();
    let recv_id = e[1].data.contacts[i].mine.as_ref().unwrap().recv_id;
    let text = |t: &str| Body::Text {
        sent_at: 0,
        expires_in: None,
        text: t.into(),
    };
    let bodies = [
        text("bare"),
        Body::Copy {
            contact: [9; 32],
            body: Box::new(text("for someone else")),
        },
        Body::Copy {
            contact: bob,
            body: Box::new(text("for bob")),
        },
    ];
    for body in bodies {
        let envelope = e[0].core().encrypt(&laptop, body).unwrap();
        let link = Link {
            who: bob,
            relay: url.clone(),
        };
        let frame = ServerFrame::Deliver {
            recv_id,
            relay_id: u64::MAX,
            envelope,
        };
        e[1].on_relay((link, Event::Frame(frame)));
    }
    assert!(has(&e[1], &bob, "for bob", true));
    assert!(!has(&e[1], &bob, "bare", true) && !has(&e[1], &bob, "bare", false));
    assert!(!has(&e[1], &bob, "for someone else", true));
    std::fs::remove_dir_all(&dir).unwrap();
}
