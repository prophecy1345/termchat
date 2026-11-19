//! Group sessions (ADR-027): Megolm keys move one to one, messages read by
//! every member, and a member cannot pass for another.
#![allow(clippy::unwrap_used)] // test-only file

use termchat_core::proto::{Body, GroupCiphertext, GroupInner};
use termchat_core::{Client, Error, KdfParams, create_file_key, verify_roster};

const G: [u8; 16] = [7; 16];

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

/// Three members, each with every other's session key (as if shared over Olm).
fn group() -> Vec<Client> {
    let mut all: Vec<Client> = (0..3).map(|_| Client::default()).collect();
    let keys: Vec<_> = all
        .iter_mut()
        .map(|c| c.group_start(G, 0).unwrap())
        .collect();
    let ids: Vec<_> = all.iter().map(ed).collect();
    for (i, c) in all.iter_mut().enumerate() {
        for (j, key) in keys.iter().enumerate() {
            if i != j {
                c.group_member_key(&G, ids[j], key).unwrap();
            }
        }
    }
    all
}

#[test]
fn every_member_reads_and_knows_who_wrote() {
    let mut g = group();
    let alice = ed(&g[0]);
    let bytes = g[0].group_encrypt(G, text("hi all")).unwrap();
    for member in &mut g {
        let (from, body) = member.group_decrypt(&G, &bytes).unwrap();
        assert_eq!((from, body), (alice, text("hi all")));
    }
    // Read once.
    assert_eq!(g[1].group_decrypt(&G, &bytes), Err(Error::Duplicate));
}

#[test]
fn a_member_cannot_write_as_another_and_others_cannot_read() {
    let mut g = group();
    let (alice, bob) = (ed(&g[0]), ed(&g[1]));
    // Keys are filed under whoever the Olm channel said sent them. Carol
    // passing her key off as Alice's changes nothing: the session already
    // belongs to Carol, and what she writes names her.
    let carols = g[2].group_key(&G).unwrap();
    g[1].group_member_key(&G, alice, &carols).ok();
    let bytes = g[2].group_encrypt(G, text("I am alice")).unwrap();
    let (from, _) = g[0].group_decrypt(&G, &bytes).unwrap();
    assert_ne!(from, alice);
    assert_eq!(
        g[1].group_decrypt(&G, &bytes).map(|r| r.0).unwrap(),
        ed(&g[2])
    );
    // Someone outside the group reads nothing.
    let mut outsider = Client::default();
    outsider.group_start(G, 0).unwrap();
    assert_eq!(
        outsider.group_decrypt(&G, &bytes),
        Err(Error::UnknownSender)
    );
    let _ = bob;
}

#[test]
fn a_removed_member_reads_nothing_after_rotation_and_newcomers_no_history() {
    let mut g = group();
    let (alice, carol) = (ed(&g[0]), ed(&g[2]));
    let before = g[0].group_encrypt(G, text("before")).unwrap();
    // Carol is removed: everyone forgets her sessions, Alice rotates and shares
    // the new key with Bob only.
    g[0].group_forget_member(&G, &carol);
    g[1].group_forget_member(&G, &carol);
    let key = g[0].group_start(G, 10).unwrap();
    g[1].group_member_key(&G, alice, &key).unwrap();
    let after = g[0].group_encrypt(G, text("after")).unwrap();
    assert_eq!(g[1].group_decrypt(&G, &after).unwrap().1, text("after"));
    assert_eq!(g[2].group_decrypt(&G, &after), Err(Error::UnknownSender));
    assert!(
        g[2].group_decrypt(&G, &before).is_ok(),
        "what she had before stays hers"
    );
    // Carol's old session no longer reads at Bob's.
    let hers = g[2].group_encrypt(G, text("still here")).unwrap();
    assert_eq!(g[1].group_decrypt(&G, &hers), Err(Error::UnknownSender));

    // A newcomer gets the current key: no history.
    let mut dave = Client::default();
    dave.group_start(G, 10).unwrap();
    dave.group_member_key(&G, alice, &g[0].group_key(&G).unwrap())
        .unwrap();
    assert!(dave.group_decrypt(&G, &before).is_err());
    let next = g[0].group_encrypt(G, text("welcome")).unwrap();
    assert_eq!(dave.group_decrypt(&G, &next).unwrap().1, text("welcome"));
}

#[test]
fn rotation_is_due_after_100_messages_or_7_days() {
    let mut c = Client::default();
    c.group_start(G, 0).unwrap();
    assert!(!c.group_due(&G, 7 * 86_400 - 1));
    assert!(c.group_due(&G, 7 * 86_400));
    for _ in 0..100 {
        c.group_encrypt(G, text("x")).unwrap();
    }
    assert!(c.group_due(&G, 0));
}

#[test]
fn group_sessions_survive_save_and_load_but_not_a_backup() {
    let mut g = group();
    let key = create_file_key(
        b"pw",
        KdfParams {
            m_kib: 64,
            t: 1,
            p: 1,
        },
    )
    .unwrap();
    let file = g[1].save(&key, "").unwrap();
    let (mut bob, _, _) = Client::load(b"pw", &file).unwrap();
    let bytes = g[0].group_encrypt(G, text("after reload")).unwrap();
    assert_eq!(
        bob.group_decrypt(&G, &bytes).unwrap().1,
        text("after reload")
    );
    let mut restored = bob.without_sessions();
    assert_eq!(restored.group_decrypt(&G, &bytes), Err(Error::NoSession));
}

#[test]
fn rosters_verify_only_with_the_admins_key() {
    let (admin, other) = (Client::default(), Client::default());
    let roster = b"members: alice, bob";
    let sig = admin.sign_roster(roster);
    assert!(verify_roster(&ed(&admin), roster, &sig).is_ok());
    assert!(verify_roster(&ed(&other), roster, &sig).is_err());
    assert!(verify_roster(&ed(&admin), b"members: alice, mallory", &sig).is_err());
    // Not a signature over anything else this identity signs.
    assert!(
        verify_roster(
            &ed(&admin),
            &admin.identity().curve25519,
            &admin.identity().signature
        )
        .is_err()
    );
}

#[test]
fn garbage_and_a_wrong_group_are_refused() {
    let mut g = group();
    assert!(g[0].group_decrypt(&G, b"not a group message").is_err());
    let mut bytes = g[1].group_encrypt(G, text("x")).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert!(g[0].group_decrypt(&G, &bytes).is_err());
    // A message for another group under this group's id.
    let other = [8; 16];
    g[1].group_start(other, 0).unwrap();
    let k = g[1].group_key(&other).unwrap();
    let bob = ed(&g[1]);
    g[0].group_start(other, 0).unwrap();
    g[0].group_member_key(&other, bob, &k).unwrap();
    g[0].group_member_key(&G, bob, &k).unwrap();
    let wrong = g[1].group_encrypt(other, text("y")).unwrap();
    assert_eq!(g[0].group_decrypt(&G, &wrong), Err(Error::SenderMismatch));
    let _ = (
        GroupCiphertext::decode(&wrong).unwrap(),
        GroupInner::from_padded(&[0; 512]).is_err(),
    );
}

/// ADR-030, section 6: a group message the relay withholds shows as a gap,
/// and one that comes late closes it.
#[test]
fn a_withheld_group_message_shows_as_missing() {
    let mut g = group();
    let one = g[0].group_encrypt(G, text("one")).unwrap();
    let two = g[0].group_encrypt(G, text("two")).unwrap();
    let three = g[0].group_encrypt(G, text("three")).unwrap();
    g[1].group_decrypt(&G, &one).unwrap();
    assert_eq!(g[1].group_missed(&G), 0);
    g[1].group_decrypt(&G, &three).unwrap();
    assert_eq!(g[1].group_missed(&G), 1);
    g[1].group_decrypt(&G, &two).unwrap();
    assert_eq!(g[1].group_missed(&G), 0);
}
