//! Two clients talking in-process, including negative cases.
#![allow(clippy::unwrap_used)] // test-only file; helpers are not #[test] fns

use termchat_core::proto::{Body, Envelope};
use termchat_core::{Client, Error, KdfParams, create_file_key};

const FAST: KdfParams = KdfParams {
    m_kib: 64,
    t: 1,
    p: 1,
};

fn text(s: &str) -> Body {
    Body::Text {
        sent_at: 0,
        expires_in: None,
        text: s.into(),
    }
}

fn ed(c: &Client) -> [u8; 32] {
    c.identity().ed25519
}

/// Alice has Bob's invite and starts a session while Bob is offline.
fn pair() -> (Client, Client) {
    let mut alice = Client::default();
    let mut bob = Client::default();
    let otk = bob.prekeys(0, true).one_time.remove(0);
    alice.add_contact(&bob.identity()).unwrap();
    bob.add_contact(&alice.identity()).unwrap();
    alice.start_session(&ed(&bob), &otk).unwrap();
    (alice, bob)
}

#[test]
fn offline_session_and_reply() {
    let (mut alice, mut bob) = pair();
    let (a, b) = (ed(&alice), ed(&bob));

    let env = alice.encrypt(&b, text("hi bob")).unwrap();
    let got = bob.decrypt(&a, &env).unwrap();
    assert!(got.new_session);
    assert_eq!(got.missed, 0);
    assert_eq!(got.body, text("hi bob"));

    let reply = bob.encrypt(&a, text("hi alice")).unwrap();
    assert_eq!(alice.decrypt(&b, &reply).unwrap().body, text("hi alice"));
    let again = alice.encrypt(&b, text("normal message now")).unwrap();
    assert_eq!(again.olm_type, 1);
    assert_eq!(
        bob.decrypt(&a, &again).unwrap().body,
        text("normal message now")
    );
}

#[test]
fn tampered_ciphertext_is_rejected() {
    let (mut alice, mut bob) = pair();
    let (a, b) = (ed(&alice), ed(&bob));
    let env = alice.encrypt(&b, text("x")).unwrap();
    for i in (0..env.body.len()).step_by(13) {
        let mut bad = env.clone();
        bad.body[i] ^= 1;
        assert!(bob.decrypt(&a, &bad).is_err(), "byte {i}");
    }
    // The original still decrypts: failed attempts did not corrupt state.
    assert!(bob.decrypt(&a, &env).is_ok());
}

#[test]
fn replay_is_rejected() {
    let (mut alice, mut bob) = pair();
    let (a, b) = (ed(&alice), ed(&bob));
    let env = alice.encrypt(&b, text("once")).unwrap();
    bob.decrypt(&a, &env).unwrap();
    assert!(bob.decrypt(&a, &env).is_err());
}

#[test]
fn dropped_and_reordered_messages_are_detected() {
    let (mut alice, mut bob) = pair();
    let (a, b) = (ed(&alice), ed(&bob));
    let m: Vec<Envelope> = (1..=4)
        .map(|i| alice.encrypt(&b, text(&i.to_string())).unwrap())
        .collect();

    assert_eq!(bob.decrypt(&a, &m[0]).unwrap().missed, 0);
    assert_eq!(bob.decrypt(&a, &m[3]).unwrap().missed, 2);
    let late = bob.decrypt(&a, &m[1]).unwrap();
    assert_eq!((late.body, late.missed), (text("2"), 1));
}

#[test]
fn wrong_recipient_and_sender_are_rejected() {
    let (mut alice, mut bob) = pair();
    let (a, b) = (ed(&alice), ed(&bob));
    let env = alice.encrypt(&b, text("x")).unwrap();

    let mut misaddressed = env.clone();
    misaddressed.recipient = a;
    assert_eq!(bob.decrypt(&a, &misaddressed), Err(Error::WrongRecipient));

    // Relay claims the message came from someone else.
    let carol = Client::default();
    assert!(bob.decrypt(&ed(&carol), &env).is_err());
}

#[test]
fn stranger_needs_valid_contact_request() {
    let mut bob = Client::default();
    let mut carol = Client::default();
    let (b, c) = (ed(&bob), ed(&carol));
    let mut keys = bob.prekeys(0, true).one_time;
    carol.add_contact(&bob.identity()).unwrap();

    // Plain text from a stranger is refused.
    carol.start_session(&b, &keys.remove(0)).unwrap();
    let env = carol.encrypt(&b, text("hello?")).unwrap();
    assert_eq!(bob.decrypt(&c, &env), Err(Error::UnknownSender));

    // A contact request with someone else's identity is refused.
    let alice = Client::default();
    carol.start_session(&b, &keys.remove(0)).unwrap();
    let forged = Body::ContactRequest {
        identity: alice.identity(),
        nickname: "alice".into(),
        note: "it's alice".into(),
    };
    let env = carol.encrypt(&b, forged).unwrap();
    assert_eq!(bob.decrypt(&c, &env), Err(Error::SenderMismatch));
    // Relay re-labels it as Alice: still refused (the one-time key is already spent).
    assert!(bob.decrypt(&ed(&alice), &env).is_err());
    assert_eq!(bob.is_request(&ed(&alice)), None);

    // A genuine request lands in requests.
    carol.start_session(&b, &keys.remove(0)).unwrap();
    let req = Body::ContactRequest {
        identity: carol.identity(),
        nickname: "carol".into(),
        note: "hi".into(),
    };
    let env = carol.encrypt(&b, req).unwrap();
    assert!(bob.decrypt(&c, &env).unwrap().new_session);
    assert_eq!(bob.is_request(&c), Some(true));
    assert_eq!(bob.nickname(&c), Some("carol"));
    bob.add_contact(&carol.identity()).unwrap();
    assert_eq!(bob.is_request(&c), Some(false));

    // Bob answers with his nickname; it survives a reload on Carol's side.
    let env = bob
        .encrypt(
            &c,
            Body::ContactAccept {
                nickname: "bob".into(),
            },
        )
        .unwrap();
    carol.decrypt(&b, &env).unwrap();
    let key = create_file_key(b"pw", FAST).unwrap();
    let carol = Client::load(b"pw", &carol.save(&key, "").unwrap())
        .unwrap()
        .0;
    assert_eq!(carol.nickname(&b), Some("bob"));
}

#[test]
fn forged_prekey_and_identity_are_rejected() {
    let mut alice = Client::default();
    let mut bob = Client::default();
    let mut otk = bob.prekeys(0, false).one_time.remove(0);
    alice.add_contact(&bob.identity()).unwrap();
    otk.key[0] ^= 1;
    assert_eq!(
        alice.start_session(&ed(&bob), &otk),
        Err(Error::BadSignature)
    );

    let mut id = bob.identity();
    id.curve25519 = alice.identity().curve25519;
    assert_eq!(Client::default().add_contact(&id), Err(Error::BadSignature));
}

#[test]
fn prekeys_top_up_only_below_half() {
    let mut bob = Client::default();
    let first = bob.prekeys(0, true);
    assert_eq!(first.one_time.len(), 50);
    assert!(first.fallback.is_some());
    let none = bob.prekeys(30, false);
    assert!(none.one_time.is_empty() && none.fallback.is_none());
    assert_eq!(bob.prekeys(10, false).one_time.len(), 40);
}

#[test]
fn state_survives_save_and_load() {
    let (mut alice, bob) = pair();
    let (a, b) = (ed(&alice), ed(&bob));
    let first = alice.encrypt(&b, text("before restart")).unwrap();

    let file = bob
        .save(&create_file_key(b"bob pw", FAST).unwrap(), "bob settings")
        .unwrap();
    assert!(matches!(
        Client::load(b"nope", &file),
        Err(Error::WrongPassword)
    ));
    let (mut bob, _, app) = Client::load(b"bob pw", &file).unwrap();
    assert_eq!(app.as_str(), "bob settings");
    assert_eq!(ed(&bob), b);

    assert_eq!(
        bob.decrypt(&a, &first).unwrap().body,
        text("before restart")
    );
    let file = alice
        .save(&create_file_key(b"alice pw", FAST).unwrap(), "")
        .unwrap();
    let mut alice = Client::load(b"alice pw", &file).unwrap().0;
    let second = alice.encrypt(&b, text("after restart")).unwrap();
    let got = bob.decrypt(&a, &second).unwrap();
    assert_eq!((got.body, got.missed), (text("after restart"), 0));
}

#[test]
fn identity_restored_from_backup_talks_again() {
    let (mut alice, mut bob) = pair();
    let (a, b) = (ed(&alice), ed(&bob));
    let env = alice
        .encrypt(&b, text("before the phone fell in the river"))
        .unwrap();
    bob.decrypt(&a, &env).unwrap();

    // Backup holds identity and contacts, no sessions; restored on a new device.
    let backup = alice
        .without_sessions()
        .save(&create_file_key(b"backup pw", FAST).unwrap(), "")
        .unwrap();
    drop(alice);
    let mut alice = Client::load(b"backup pw", &backup).unwrap().0;
    assert_eq!(ed(&alice), a);
    assert!(!alice.has_session(&b));

    // Fresh session from a new one-time key; Bob is told the conversation restarted.
    let otk = bob.prekeys(0, false).one_time.remove(0);
    alice.start_session(&b, &otk).unwrap();
    let env = alice.encrypt(&b, text("new phone, same me")).unwrap();
    let got = bob.decrypt(&a, &env).unwrap();
    assert!(got.new_session);
    assert_eq!(got.body, text("new phone, same me"));
    let reply = bob.encrypt(&a, text("welcome back")).unwrap();
    assert_eq!(
        alice.decrypt(&b, &reply).unwrap().body,
        text("welcome back")
    );
}

#[test]
fn requests_are_flagged_names_stick_and_requests_are_capped() {
    let mut bob = Client::default();
    let b = ed(&bob);
    let keys = bob.prekeys(0, true);
    let mut one_time = keys.one_time;
    let fallback = keys.fallback.unwrap();

    let request = |c: &Client, nick: &str| Body::ContactRequest {
        identity: c.identity(),
        nickname: nick.into(),
        note: String::new(),
    };
    let mut carol = Client::default();
    let c = ed(&carol);
    carol.add_contact(&bob.identity()).unwrap();
    carol.start_session(&b, &one_time.remove(0)).unwrap();
    let env = carol.encrypt(&b, request(&carol, "carol")).unwrap();
    assert!(bob.decrypt(&c, &env).unwrap().request);
    let env = carol.encrypt(&b, text("hidden until accepted")).unwrap();
    assert!(bob.decrypt(&c, &env).unwrap().request, "still a request");
    // A pending request may correct its name.
    let env = carol.encrypt(&b, request(&carol, "Carol")).unwrap();
    bob.decrypt(&c, &env).unwrap();
    assert_eq!(bob.nickname(&c), Some("Carol"));

    bob.add_contact(&carol.identity()).unwrap();
    let env = carol.encrypt(&b, text("now visible")).unwrap();
    assert!(!bob.decrypt(&c, &env).unwrap().request);
    // After acceptance the name no longer changes from the network.
    let env = carol
        .encrypt(
            &b,
            Body::ContactAccept {
                nickname: "Alice".into(),
            },
        )
        .unwrap();
    bob.decrypt(&c, &env).unwrap();
    let env = carol.encrypt(&b, request(&carol, "Alice")).unwrap();
    bob.decrypt(&c, &env).unwrap();
    assert_eq!(bob.nickname(&c), Some("Carol"));

    // Carol is accepted, so 50 strangers fill the pending slots; the 51st is refused.
    for i in 0..51 {
        let mut s = Client::default();
        s.add_contact(&bob.identity()).unwrap();
        let key = one_time.pop().unwrap_or_else(|| fallback.clone());
        s.start_session(&b, &key).unwrap();
        let env = s.encrypt(&b, request(&s, "stranger")).unwrap();
        let got = bob.decrypt(&ed(&s), &env);
        match i {
            0..50 => assert!(got.is_ok(), "stranger {i}"),
            _ => assert_eq!(got, Err(Error::TooManyRequests)),
        }
    }
}
