#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::{Duration, Instant};

use termchat_core::Client;
use termchat_core::proto::{
    Body, Device, DeviceList, Envelope, Inner, Invite, ReaderGrant, Roster, ServerFrame, VERSION,
};
use termchat_engine::engine::Link;
use termchat_engine::{Engine, Event, Setup, Unlocked};
use termchat_relay::{Config, Limits, Rate};
use tokio::net::TcpListener;

const PASSWORD: &str = "correct horse battery staple violin";
const ALICE: usize = 0;
const BOB: usize = 1;
const MALLORY: usize = 2;

async fn start_relay() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let config = Config {
        trust_proxy: false,
        db: None,
        reports: None,
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
                .is_some_and(|i| x.rich(i) && x.data.contacts[i].understands >= 2)
        };
        rich(&e[a], kb) && rich(&e[b], ka)
    })
    .await;
}

async fn world(tag: &str) -> (Vec<Box<Engine>>, String, std::path::PathBuf) {
    let url = start_relay().await;
    let dir = std::env::temp_dir().join(format!("termchat-audit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = vec![
        engine(&dir, "alice", &url),
        engine(&dir, "bob", &url),
        engine(&dir, "mallory", &url),
    ];
    pump(&mut e, |e| {
        e.iter()
            .all(|x| x.data.invite_queue.is_some() && x.data.fallback_published)
    })
    .await;
    befriend(&mut e, BOB, ALICE).await;
    befriend(&mut e, MALLORY, ALICE).await;
    (e, url, dir)
}

fn from_mallory(e: &mut [Box<Engine>], url: &str, body: Body) {
    let (alice, mallory) = (e[ALICE].me(), e[MALLORY].me());
    let envelope = e[MALLORY].core().encrypt(&alice, body).unwrap();
    let i = e[ALICE].index(&mallory).unwrap();
    let recv_id = e[ALICE].data.contacts[i].mine.as_ref().unwrap().recv_id;
    let link = Link {
        who: mallory,
        relay: url.to_owned(),
    };
    let frame = ServerFrame::Deliver {
        recv_id,
        relay_id: u64::MAX,
        envelope,
    };
    e[ALICE].on_relay((link, Event::Frame(frame)));
}

fn devices_of_mallory(e: &[Box<Engine>], no: u64, extra: Option<Device>) -> DeviceList {
    let id = e[MALLORY].core().identity();
    let mut list = DeviceList {
        identity: id.ed25519,
        no,
        devices: vec![Device {
            ed25519: id.ed25519,
            curve25519: id.curve25519,
            proof: e[MALLORY].core().device_proof(&id.ed25519),
        }],
        signature: [0; 64],
    };
    list.devices.extend(extra);
    list.signature = e[MALLORY].core().sign_devices(&list).unwrap();
    list
}

#[tokio::test(flavor = "multi_thread")]
async fn poc_a_contact_wipes_our_sessions_with_another_contact() {
    let (mut e, url, dir) = world("a").await;
    let bob = e[BOB].me();
    assert!(e[ALICE].core().has_session(&bob));
    let fake = Device {
        ed25519: bob,
        curve25519: [9; 32],
        // No proof: the key's holder never agreed (ADR-030).
        proof: [0; 64],
    };
    let list = devices_of_mallory(&e, 1, Some(fake));
    from_mallory(
        &mut e,
        &url,
        Body::Devices {
            list,
            queues: Vec::new(),
        },
    );
    assert!(e[ALICE].core().has_session(&bob));
    let list = devices_of_mallory(&e, 2, None);
    from_mallory(
        &mut e,
        &url,
        Body::Devices {
            list,
            queues: Vec::new(),
        },
    );
    assert!(
        e[ALICE].core().has_session(&bob),
        "PoC A: Mallory's device list erased Alice's sessions with Bob"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn poc_b_a_contact_plants_a_contact_with_someone_elses_fingerprint() {
    use vodozemac::Curve25519PublicKey;
    use vodozemac::olm::{Account, SessionConfig};

    let (mut e, url, dir) = world("b").await;
    let victim = Client::default().identity();
    let attacker = Account::new();
    let attacker_curve = attacker.curve25519_key().to_bytes();
    assert_ne!(attacker_curve, victim.curve25519);

    let fake = Device {
        ed25519: victim.ed25519,
        curve25519: attacker_curve,
        // No proof: Victor never agreed (ADR-030).
        proof: [0; 64],
    };
    let list = devices_of_mallory(&e, 1, Some(fake));
    from_mallory(
        &mut e,
        &url,
        Body::Devices {
            list,
            queues: Vec::new(),
        },
    );

    let invite = Invite::decode(&e[ALICE].public_invite(false).unwrap()).unwrap();
    let alice = e[ALICE].core().identity();
    let prekey = e[ALICE].core().one_time_keys(1).remove(0);
    let mut session = attacker
        .create_outbound_session(
            SessionConfig::version_1(),
            Curve25519PublicKey::from_bytes(alice.curve25519),
            Curve25519PublicKey::from_bytes(prekey.key),
        )
        .unwrap();
    let plain = Inner {
        sender: victim.ed25519,
        recipient: alice.ed25519,
        seq: 1,
        body: Body::ContactRequestV2 {
            identity: victim.clone(),
            nickname: "Victor".into(),
            note: String::new(),
            code: invite.code,
        },
    }
    .to_padded()
    .unwrap();
    let (olm_type, body) = session.encrypt(&plain).unwrap().to_parts();
    let envelope = Envelope {
        version: VERSION,
        olm_type: olm_type as u8,
        body,
    };
    let recv_id = e[ALICE].data.invite_queue.as_ref().unwrap().recv_id;
    let link = Link {
        who: alice.ed25519,
        relay: url.clone(),
    };
    let frame = ServerFrame::Deliver {
        recv_id,
        relay_id: u64::MAX,
        envelope,
    };
    e[ALICE].on_relay((link, Event::Frame(frame)));

    let planted = e[ALICE].index(&victim.ed25519);
    if let Some(i) = planted {
        let c = &e[ALICE].data.contacts[i];
        eprintln!(
            "PoC B: contact \"{}\" fingerprint {} request={} accept_pending={} curve_is_attackers={}",
            e[ALICE].name(c),
            termchat_core::fingerprint(&c.key),
            e[ALICE].is_request(&c.key),
            c.accept_pending,
            e[ALICE].core().curve_of(&c.key) == Some(attacker_curve),
        );
    }
    assert!(
        planted.is_none(),
        "PoC B: a contact with the victim's Ed25519 key (and fingerprint) but the attacker's Curve25519 key was added, accepted without a request"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn poc_c_an_accepted_group_invite_locks_the_account_out() {
    let (mut e, url, dir) = world("c").await;
    let mallory = e[MALLORY].me();
    let mut roster = Roster {
        group: [7; 16],
        no: 1,
        admin: mallory,
        name: "book club".into(),
        members: Vec::new(),
        expires_in: None,
        signature: [0; 64],
    };
    roster.signature = e[MALLORY].core().sign_roster(&roster.signed_bytes());
    from_mallory(
        &mut e,
        &url,
        Body::GroupInvite {
            roster,
            relay_url: "ws://relay.attacker.example/".into(),
            onion_url: None,
            send_id: [1; 16],
            reader: ReaderGrant {
                recv_id: [2; 16],
                send_id: [3; 16],
                token: [4; 32],
            },
        },
    );
    // Refused at receipt: an invite to an address that cannot be used.
    assert!(e[ALICE].data.group_invites.is_empty());
    let reopened = Engine::unlock(&dir.join("alice"), PASSWORD.as_bytes());
    let why = match &reopened {
        Ok(Unlocked::Open(_)) => None,
        Ok(Unlocked::Wiped) => Some("wiped".to_string()),
        Err(why) => Some(why.clone()),
    };
    assert!(
        why.is_none(),
        "PoC C: the state no longer opens with the right password: {why:?}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn poc_d_a_queue_offer_locks_a_tor_user_out() {
    let (mut e, url, dir) = world("d").await;
    e[ALICE].data.tor = true;
    e[ALICE].data.onion_url = Some("ws://aaaaaaaaaaaaaaaa.onion/".into());
    from_mallory(
        &mut e,
        &url,
        Body::QueueOffer {
            relay_url: "wss://relay.attacker.example/".into(),
            onion_url: None,
            send_id: [1; 16],
        },
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let path = dir.join("alice");
    let why = std::thread::spawn(move || {
        let _guard = rt.enter();
        match Engine::unlock(&path, PASSWORD.as_bytes()) {
            Ok(Unlocked::Open(_)) => None,
            Ok(Unlocked::Wiped) => Some("wiped".to_string()),
            Err(why) => Some(why),
        }
    })
    .join()
    .unwrap();
    assert!(
        why.is_none(),
        "PoC D: the state no longer opens with the right password: {why:?}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn poc_e_the_relay_moves_bobs_message_into_mallorys_chat() {
    let (mut e, url, dir) = world("e").await;
    let (alice, bob, mallory) = (e[ALICE].me(), e[BOB].me(), e[MALLORY].me());
    let fake = Device {
        ed25519: bob,
        curve25519: [9; 32],
        // No proof: the key's holder never agreed (ADR-030).
        proof: [0; 64],
    };
    let list = devices_of_mallory(&e, 1, Some(fake));
    from_mallory(
        &mut e,
        &url,
        Body::Devices {
            list,
            queues: Vec::new(),
        },
    );
    let envelope = e[BOB]
        .core()
        .encrypt(
            &alice,
            Body::Text {
                sent_at: 0,
                expires_in: None,
                text: "yes, I agree to everything".into(),
            },
        )
        .unwrap();
    let m = e[ALICE].index(&mallory).unwrap();
    let recv_id = e[ALICE].data.contacts[m].mine.as_ref().unwrap().recv_id;
    let link = Link {
        who: mallory,
        relay: url.clone(),
    };
    let frame = ServerFrame::Deliver {
        recv_id,
        relay_id: u64::MAX,
        envelope,
    };
    e[ALICE].on_relay((link, Event::Frame(frame)));
    let shown = e[ALICE].data.contacts[m]
        .messages
        .iter()
        .any(|x| !x.mine && x.text == "yes, I agree to everything");
    assert!(
        !shown,
        "PoC E: a message Bob wrote to Alice is shown in Alice's chat with Mallory, as Mallory's"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Finding 5: a member who is not the admin cannot remove another member by
/// listing their key as a device and then dropping it.
#[tokio::test(flavor = "multi_thread")]
async fn poc_f_a_member_cannot_remove_another_through_a_device_list() {
    let (mut e, url, dir) = world("f").await;
    let g = e[ALICE].create_group("club").unwrap();
    pump(&mut e, |e| e[ALICE].data.groups[0].reader.is_some()).await;
    for who in [BOB, MALLORY] {
        let k = e[who].me();
        let i = e[ALICE].index(&k).unwrap();
        e[ALICE].invite_to_group(g, i).unwrap();
    }
    pump(&mut e, |e| {
        e[BOB].data.group_invites.len() == 1 && e[MALLORY].data.group_invites.len() == 1
    })
    .await;
    e[BOB].accept_group(0).unwrap();
    e[MALLORY].accept_group(0).unwrap();
    let bob = e[BOB].me();
    pump(&mut e, |e| {
        e.iter().all(|x| {
            x.data
                .groups
                .first()
                .is_some_and(|g| g.members().len() == 3)
        })
    })
    .await;
    let fake = Device {
        ed25519: bob,
        curve25519: [9; 32],
        // No proof: the key's holder never agreed (ADR-030).
        proof: [0; 64],
    };
    for (no, extra) in [(1, Some(fake)), (2, None)] {
        let list = devices_of_mallory(&e, no, extra);
        from_mallory(
            &mut e,
            &url,
            Body::Devices {
                list,
                queues: Vec::new(),
            },
        );
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        pump(&mut e, |_| true).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        e[ALICE].data.groups[0]
            .members()
            .iter()
            .any(|m| m.ed25519 == bob),
        "PoC F: Mallory's device lists removed Bob from Alice's group"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
