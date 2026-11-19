//! Devices of one identity (ADR-028): the primary signs the list, contacts
//! check it, and each device talks over sessions of its own.
#![allow(clippy::unwrap_used)] // test-only file

use termchat_core::proto::{Body, Device, DeviceList};
use termchat_core::{Client, Error, verify_devices};

fn text(s: &str) -> Body {
    Body::Text {
        sent_at: 0,
        expires_in: None,
        text: s.into(),
    }
}

fn device(c: &Client) -> Device {
    let id = c.identity();
    Device {
        ed25519: id.ed25519,
        curve25519: id.curve25519,
    }
}

/// Alice's phone (the primary) and laptop, the list signed by the phone.
fn alice() -> (Client, Client, DeviceList) {
    let phone = Client::default();
    let laptop = Client::default();
    let mut list = DeviceList {
        identity: phone.identity().ed25519,
        no: 1,
        devices: vec![device(&phone), device(&laptop)],
        signature: [0; 64],
    };
    list.signature = phone.sign_devices(&list).unwrap();
    (phone, laptop, list)
}

#[test]
fn a_list_signed_by_its_primary_checks_and_nothing_else_does() {
    let (phone, laptop, list) = alice();
    verify_devices(&list).unwrap();

    // A device slipped in, or the number changed, after signing.
    let mut more = list.clone();
    more.devices.push(device(&Client::default()));
    assert_eq!(verify_devices(&more), Err(Error::BadSignature));
    let mut renumbered = list.clone();
    renumbered.no = 2;
    assert_eq!(verify_devices(&renumbered), Err(Error::BadSignature));

    // Only the primary signs: the laptop cannot, for Alice or for itself.
    assert_eq!(laptop.sign_devices(&list), Err(Error::BadSignature));
    let mut its_own = list.clone();
    its_own.identity = laptop.identity().ed25519;
    assert!(
        laptop.sign_devices(&its_own).is_err(),
        "laptop is not first"
    );

    // A stranger's list cannot borrow Alice's signature.
    let mallory = Client::default();
    let mut stolen = list.clone();
    stolen.identity = mallory.identity().ed25519;
    stolen.devices[0] = device(&mallory);
    assert_eq!(verify_devices(&stolen), Err(Error::BadSignature));
    // A signature over a roster is not one over a device list.
    let roster_sig = phone.sign_roster(&list.signed_bytes());
    let mut wrong_context = list.clone();
    wrong_context.signature = roster_sig;
    assert_eq!(verify_devices(&wrong_context), Err(Error::BadSignature));
}

#[test]
fn a_contact_talks_to_each_device_of_the_list() {
    let (phone, mut laptop, list) = alice();
    let mut bob = Client::default();
    let (b, l) = (bob.identity().ed25519, laptop.identity().ed25519);
    bob.add_contact(&phone.identity()).unwrap();
    laptop.add_contact(&bob.identity()).unwrap();

    // Bob knows the laptop only from the signed list.
    verify_devices(&list).unwrap();
    for d in &list.devices[1..] {
        bob.add_member(d.ed25519, d.curve25519).unwrap();
    }
    let otk = laptop.one_time_keys(1).remove(0);
    bob.start_session(&l, &otk).unwrap();
    let env = bob.encrypt(&l, text("to your laptop")).unwrap();
    let got = laptop.decrypt(Some(&b), &env).unwrap();
    assert_eq!((got.sender, got.body), (b, text("to your laptop")));

    let back = laptop.encrypt(&b, text("from the laptop")).unwrap();
    let got = bob.decrypt(Some(&l), &back).unwrap();
    assert_eq!((got.sender, got.body), (l, text("from the laptop")));

    // A key the laptop signed does not open a session as the phone.
    let otk = laptop.one_time_keys(1).remove(0);
    assert_eq!(
        bob.start_session(&phone.identity().ed25519, &otk),
        Err(Error::BadSignature)
    );
}

#[test]
fn a_device_off_the_list_is_nobody() {
    let (_, mut laptop, _) = alice();
    let mut bob = Client::default();
    let b = bob.identity().ed25519;
    laptop.add_contact(&bob.identity()).unwrap();
    let otk = bob.one_time_keys(1).remove(0);
    laptop.start_session(&b, &otk).unwrap();
    let env = laptop.encrypt(&b, text("hi, it's alice")).unwrap();
    // Bob never saw a list with this laptop on it.
    let l = laptop.identity().ed25519;
    assert_eq!(
        bob.decrypt(Some(&l), &env).unwrap_err(),
        Error::UnknownSender
    );
}

#[test]
fn a_link_answer_binds_the_primarys_key_like_a_request() {
    let link = |identity| Body::DeviceLink {
        code: [9; 16],
        identity,
    };
    // A primary with a session to the new device's invite queue.
    let start = |laptop: &mut Client| {
        let mut phone = Client::default();
        let l = laptop.identity().ed25519;
        phone.add_contact(&laptop.identity()).unwrap();
        let otk = laptop.one_time_keys(1).remove(0);
        phone.start_session(&l, &otk).unwrap();
        (phone, l)
    };
    let mut laptop = Client::default();

    // An identity that is not the sender's own is refused.
    let (mut phone, l) = start(&mut laptop);
    let env = phone
        .encrypt(&l, link(Client::default().identity()))
        .unwrap();
    assert_eq!(
        laptop.decrypt(None, &env).unwrap_err(),
        Error::SenderMismatch
    );

    // The genuine one arrives as a request from the primary.
    let (mut phone, l) = start(&mut laptop);
    let env = phone.encrypt(&l, link(phone.identity())).unwrap();
    let got = laptop.decrypt(None, &env).unwrap();
    assert_eq!(got.sender, phone.identity().ed25519);
    assert!(got.request);
}

/// ADR-030: someone else's word never changes a key we hold, and a signed
/// identity replaces what someone else said of a key.
#[test]
fn a_member_entry_yields_to_a_signed_identity_and_never_rekeys_a_peer() {
    let mut me = Client::default();
    let victor = Client::default();
    let mallory = Client::default();
    let v = victor.identity();
    // Mallory's word: Victor's key with her Curve25519 key.
    me.add_member(v.ed25519, mallory.identity().curve25519)
        .unwrap();
    // Victor's own signed identity wins.
    me.add_contact(&v).unwrap();
    assert_eq!(me.curve_of(&v.ed25519), Some(v.curve25519));
    // Now a peer: a roster or list cannot change its key.
    assert!(
        me.add_member(v.ed25519, mallory.identity().curve25519)
            .is_err()
    );
    assert_eq!(me.curve_of(&v.ed25519), Some(v.curve25519));
}
