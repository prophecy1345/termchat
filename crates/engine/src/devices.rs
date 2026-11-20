//! Devices of one identity (ADR-028): a new device asks with a link code,
//! the primary answers, both screens show the identity's fingerprint, and
//! once the person confirms on both the primary signs the new device list.
//!
//! An own device is kept as a contact with `own` set: the same queues,
//! sessions and links as a person, never shown as one.

use super::*;
use termchat_core::proto::{
    Device, DeviceList, DeviceQueue, LinkedContact, MAX_DEVICES, SignedKey,
};
use termchat_core::{fingerprint, verify_devices};

/// Messages kept for one device of a contact until we have a session with it.
const WAITING_PER_DEVICE: usize = 64;

impl Engine {
    /// The primary, or a device on its own: only it links other devices.
    pub fn is_primary(&self) -> bool {
        self.data.primary.is_none()
    }

    /// Our identity: the primary's key, also on a linked device.
    pub fn identity_key(&self) -> Ed25519Key {
        self.data.primary.as_ref().map_or(self.me(), |p| p.ed25519)
    }

    /// A new device's link code, to paste or scan on the primary. Only a
    /// device with nobody on it yet can be linked.
    pub fn link_code(&mut self) -> Result<String, String> {
        if !self.is_primary() || self.data.devices.is_some() || !self.data.contacts.is_empty() {
            return Err("only a new device, with no contacts yet, can be linked".into());
        }
        let code = new_code();
        let link = self.invite_at(0, code)?.encode_link();
        self.data.link_code = Some(code);
        self.persist();
        Ok(link)
    }

    /// On the primary: takes a new device's link code. Nothing secret goes
    /// before the person confirms the fingerprint on both screens.
    pub fn add_device(&mut self, code: &str) -> Result<usize, String> {
        if !self.is_primary() {
            return Err("only your main device links others".into());
        }
        let link = Invite::decode_link(code).map_err(|e| format!("invalid link code: {e}"))?;
        if !self.peer_addr_ok(&link.relay_url, link.onion_url.as_deref()) {
            return Err("invalid link code: its relay address cannot be used".into());
        }
        let key = link.identity.ed25519;
        if key == self.me() || self.index(&key).is_some() {
            return Err("this device is already linked, or is this one".into());
        }
        let linked = self.data.devices.as_ref().map_or(1, |d| d.devices.len());
        let pending = self
            .data
            .contacts
            .iter()
            .filter(|c| c.own && !self.lists(&c.key))
            .count();
        if linked + pending >= MAX_DEVICES {
            return Err(format!("at most {MAX_DEVICES} devices, this one included"));
        }
        self.core()
            .add_contact(&link.identity)
            .map_err(|e| format!("invalid link code: {e}"))?;
        self.data.contacts.push(Contact {
            key,
            own: true,
            added_at: now(),
            next_client_id: getrandom::u64().unwrap_or(0) >> 2,
            request_pending: true,
            invite_code: Some(link.code),
            their_invite: Some(Addr {
                relay_url: link.relay_url,
                onion_url: link.onion_url,
                send_id: link.queue,
            }),
            ..Default::default()
        });
        let i = self.data.contacts.len() - 1;
        self.persist();
        self.sync_links()?;
        self.ensure_queue(key);
        self.claim(key);
        Ok(i)
    }

    /// On the main device: our answer has not reached the new device's relay
    /// yet (through Tor that takes 10 to 30 s), so its screen cannot show the
    /// fingerprint; ours waits too, or the person would confirm alone.
    pub fn link_waiting(&self, i: usize) -> bool {
        self.is_primary()
            && self
                .data
                .contacts
                .get(i)
                .is_some_and(|c| c.own && (c.request_pending || !self.all_answered()))
    }

    /// The device waiting for our person to compare fingerprints.
    pub fn pending_link(&self) -> Option<usize> {
        self.data.contacts.iter().position(|c| c.own && !c.link_ok)
    }

    /// The fingerprint both screens show while linking: the identity's.
    pub fn link_fingerprint(&self) -> String {
        let id = self.identity_key();
        fingerprint(&id)
    }

    /// Our person saw the same fingerprint on both screens.
    pub fn confirm_link(&mut self, i: usize) {
        let Some(c) = self.data.contacts.get_mut(i) else {
            return;
        };
        if !c.own || c.link_ok {
            return;
        }
        c.link_ok = true;
        if self.is_primary() {
            self.persist();
            return self.finish_link(i);
        }
        // Our queue for the primary goes first, then that we agree, with our
        // proof that we join its identity (ADR-030).
        let primary = self.data.contacts[i].key;
        let proof = self.core().device_proof(&primary);
        self.data.contacts[i]
            .outbox
            .push(Body::DeviceLinked { proof });
        self.accept(i);
    }

    /// On the primary: device `key` leaves the list. Contacts stop writing to
    /// it, our devices stop sending it copies, and it hears so itself.
    pub fn remove_device(&mut self, key: &Ed25519Key) -> Result<(), String> {
        if !self.is_primary() {
            return Err("only your main device removes devices".into());
        }
        if *key == self.me() {
            return Err("the main device stays: it is your identity".into());
        }
        let Some(mut list) = self.data.devices.clone() else {
            return Err("no such device".into());
        };
        let before = list.devices.len();
        list.devices.retain(|d| d.ed25519 != *key);
        if list.devices.len() == before {
            return Err("no such device".into());
        }
        list.no += 1;
        list.signature = self.core().sign_devices(&list).map_err(|e| e.to_string())?;
        self.data.devices = Some(list.clone());
        self.data.device_labels.retain(|(k, _)| k != key);
        let me = self.me();
        self.group_devices_changed(me, &[*key]);
        // The removed device hears the list without itself, then nothing.
        if let Some(o) = self.index(key) {
            self.data.contacts[o].outbox.push(Body::OwnDevices {
                list,
                labels: Vec::new(),
                nickname: self.data.nickname.clone(),
            });
            self.deliver(o);
        }
        self.tell_own_devices();
        for c in 0..self.data.contacts.len() {
            if !self.data.contacts[c].own {
                self.deliver(c);
            }
        }
        self.persist();
        Ok(())
    }

    /// On the primary: device `key`'s label, "phone" or "laptop", seen only
    /// by our own devices. Empty takes it away.
    pub fn set_device_label(&mut self, key: &Ed25519Key, label: &str) -> Result<(), String> {
        if !self.is_primary() {
            return Err("labels are set on your main device".into());
        }
        if !self.lists(key) && *key != self.me() {
            return Err("no such device".into());
        }
        let label = label.trim();
        if !label.is_empty() {
            check_nickname(label)
                .map_err(|_| "a label is up to 64 bytes, no control characters")?;
        }
        self.data.device_labels.retain(|(k, _)| k != key);
        if !label.is_empty() {
            self.data.device_labels.push((*key, label.to_string()));
        }
        self.tell_own_devices();
        Ok(())
    }

    /// A linked device the primary removed: it keeps what it had, and sends
    /// nothing more.
    pub fn removed(&self) -> bool {
        !self.is_primary() && self.data.devices.is_some() && !self.lists(&self.me())
    }

    /// Our devices for a list on screen: key and label, the primary first.
    pub fn device_rows(&self) -> Vec<(Ed25519Key, String)> {
        let label = |k: &Ed25519Key| {
            let l = self.data.device_labels.iter().find(|(x, _)| x == k);
            l.map(|(_, l)| clean(l)).unwrap_or_default()
        };
        match &self.data.devices {
            Some(d) => d
                .devices
                .iter()
                .map(|x| (x.ed25519, label(&x.ed25519)))
                .collect(),
            None => vec![(self.me(), label(&self.me()))],
        }
    }

    /// Whether our device list names `key`.
    pub(super) fn lists(&self, key: &Ed25519Key) -> bool {
        self.data
            .devices
            .as_ref()
            .is_some_and(|d| d.devices.iter().any(|x| x.ed25519 == *key))
    }

    /// On the primary, once both screens were confirmed: the device joins
    /// the list, signed, and every device of ours hears of it.
    fn finish_link(&mut self, i: usize) {
        let c = &self.data.contacts[i];
        if !(c.own && c.link_ok && c.link_theirs) || self.lists(&c.key) {
            return;
        }
        let key = c.key;
        let Ok(proof) = <[u8; 64]>::try_from(c.link_proof.as_slice()) else {
            return self.note("the new device sent no proof: link it again");
        };
        let Some(curve25519) = self.core().curve_of(&key) else {
            return;
        };
        let me = self.core().identity();
        let own_proof = self.core().device_proof(&me.ed25519);
        let mut list = self.data.devices.clone().unwrap_or_else(|| DeviceList {
            identity: me.ed25519,
            no: 0,
            devices: vec![Device {
                ed25519: me.ed25519,
                curve25519: me.curve25519,
                proof: own_proof,
            }],
            signature: [0; 64],
        });
        list.devices.push(Device {
            ed25519: key,
            curve25519,
            proof,
        });
        list.no += 1;
        let signed = self.core().sign_devices(&list);
        match signed {
            Ok(s) => list.signature = s,
            Err(e) => return self.note(e.to_string()),
        }
        self.data.devices = Some(list);
        self.tell_own_devices();
        self.hand_over(i);
        // Our groups: the new device takes its invite to each when it comes.
        for g in self.data.groups.iter().filter(|g| !g.left) {
            self.data.contacts[i]
                .outbox
                .push(Body::OwnGroupJoin { group: g.id });
        }
        self.deliver(i);
        let me = self.me();
        self.group_devices_changed(me, &[]);
        // Every contact hears of the new list; the new device learns where
        // we read each of them.
        for c in 0..self.data.contacts.len() {
            if !self.data.contacts[c].own {
                self.deliver(c);
            }
        }
        self.note("device linked");
    }

    /// The people we talk to, to new device `i`: never our own devices, nor
    /// the contacts of an incognito identity (each is an identity of its own).
    fn hand_over(&mut self, i: usize) {
        let mut bodies = Vec::new();
        for c in 0..self.data.contacts.len() {
            if let Some(body) = self.linked_contact(c) {
                bodies.push(body);
                self.data.contacts[c].handed = true;
            }
        }
        self.data.contacts[i].outbox.extend(bodies);
        self.deliver(i);
    }

    /// On the primary: contact `c`, made after our devices were linked, to
    /// each of them, once it is complete (accepted, their queue known).
    pub(super) fn hand_to_devices(&mut self, c: usize) {
        if !self.is_primary() || self.data.devices.is_none() || self.data.contacts[c].handed {
            return;
        }
        let Some(body) = self.linked_contact(c) else {
            return;
        };
        self.data.contacts[c].handed = true;
        let own: Vec<usize> = (0..self.data.contacts.len())
            .filter(|&d| self.data.contacts[d].own)
            .collect();
        for d in own {
            self.data.contacts[d].outbox.push(body.clone());
            self.deliver(d);
        }
        self.persist();
    }

    /// Contact `c` as a linked device takes it, if it is one to hand over.
    fn linked_contact(&self, c: usize) -> Option<Body> {
        let c = &self.data.contacts[c];
        let theirs = c.theirs.as_ref()?;
        if c.own || c.profile.is_some() || c.blocked || c.left || self.is_request(&c.key) {
            return None;
        }
        let core = self.core();
        let curve25519 = core.curve_of(&c.key)?;
        let nickname = core.nickname(&c.key).unwrap_or("?").to_string();
        drop(core);
        Some(Body::LinkedContact {
            contact: LinkedContact {
                // Over Olm from our primary: the new device trusts the
                // keys without the contact's own signature, which we keep
                // nowhere.
                identity: proto::Identity {
                    ed25519: c.key,
                    curve25519,
                    signature: [0; 64],
                },
                nickname,
                petname: c.petname.clone(),
                verified: c.verified,
                understands: c.understands,
                devices: None,
                queues: vec![DeviceQueue {
                    device: c.key,
                    relay_url: theirs.relay_url.clone(),
                    onion_url: theirs.onion_url.clone(),
                    send_id: theirs.send_id,
                }],
            },
        })
    }

    /// What contact `i` and our other devices should know of this device,
    /// once our queue for them exists: our device list and address, for the
    /// contact (`Devices`); where we read them, for each own device (`OwnQueue`).
    pub(super) fn tell_devices(&mut self, i: usize) -> Result<(), String> {
        let Some(list) = self.data.devices.clone() else {
            return Ok(());
        };
        let c = &self.data.contacts[i];
        let Some(addr) = c.mine.as_ref().map(Queue::addr) else {
            return Ok(());
        };
        if c.own || c.profile.is_some() {
            return Ok(());
        }
        let (key, me) = (c.key, self.me());
        let queue = DeviceQueue {
            device: me,
            relay_url: addr.relay_url,
            onion_url: addr.onion_url,
            send_id: addr.send_id,
        };
        if c.devices_told < list.no && c.understands >= proto::UNDERSTANDS {
            // Every address we know: the contact may not read a device of
            // ours before it has our list, so each device repeats the others'.
            let mut queues = vec![queue.clone()];
            for (d, a) in &c.siblings {
                if list.devices.iter().any(|x| x.ed25519 == *d) {
                    queues.push(DeviceQueue {
                        device: *d,
                        relay_url: a.relay_url.clone(),
                        onion_url: a.onion_url.clone(),
                        send_id: a.send_id,
                    });
                }
            }
            let body = Body::Devices {
                list: list.clone(),
                queues,
            };
            self.transmit(&key, body)?;
            self.data.contacts[i].devices_told = list.no;
        }
        // Our devices with a channel to us: the primary, or each other once met.
        for d in list.devices.iter().filter(|d| d.ed25519 != me) {
            let Some(o) = self
                .index(&d.ed25519)
                .filter(|&o| self.data.contacts[o].own)
            else {
                continue;
            };
            if self.data.contacts[i].siblings_told.contains(&d.ed25519) {
                continue;
            }
            self.data.contacts[o].outbox.push(Body::OwnQueue {
                contact: key,
                queue: queue.clone(),
            });
            self.data.contacts[i].siblings_told.push(d.ed25519);
            self.deliver(o);
        }
        Ok(())
    }

    /// `body` to each of our own devices that we have a channel with.
    pub(super) fn tell_own(&mut self, body: Body) {
        for o in 0..self.data.contacts.len() {
            let key = self.data.contacts[o].key;
            if self.data.contacts[o].own && self.lists(&key) {
                self.data.contacts[o].outbox.push(body.clone());
                self.deliver(o);
            }
        }
    }

    /// Our device list, labels and nickname, to each of our devices.
    fn tell_own_devices(&mut self) {
        let Some(list) = self.data.devices.clone() else {
            return;
        };
        let body = Body::OwnDevices {
            list,
            labels: self.data.device_labels.clone(),
            nickname: self.data.nickname.clone(),
        };
        for i in 0..self.data.contacts.len() {
            let key = self.data.contacts[i].key;
            if self.data.contacts[i].own && self.lists(&key) {
                self.data.contacts[i].outbox.push(body.clone());
                self.deliver(i);
            }
        }
        self.persist();
    }

    /// A device kind of message from contact `i` (ADR-028).
    pub(super) fn device_body(&mut self, i: usize, request: bool, body: Body) {
        let own = self.data.contacts[i].own;
        match body {
            // Let in only by our link code (`admit`): the primary answers.
            Body::DeviceLink { identity, .. } if request && self.data.primary.is_none() => {
                // Our own device goes after the people (`Data::people`).
                let mut c = self.data.contacts.remove(i);
                c.own = true;
                self.data.contacts.push(c);
                // The fingerprint window opens by itself (`pending_link`); a
                // note on top would hide it until dismissed.
                self.data.primary = Some(identity);
            }
            Body::DeviceLinked { proof } if own && self.is_primary() => {
                self.data.contacts[i].link_theirs = true;
                self.data.contacts[i].link_proof = proof.to_vec();
                self.finish_link(i);
            }
            Body::OwnDevices {
                list,
                labels,
                nickname,
            } if own => {
                let Some(primary) = self.data.primary.as_ref().map(|p| p.ed25519) else {
                    return;
                };
                let me = self.me();
                // The same list again: new labels or a new nickname only.
                if self.data.contacts[i].key == primary && self.data.devices.as_ref() == Some(&list)
                {
                    self.data.device_labels = labels;
                    self.data.nickname = nickname;
                    return;
                }
                let newer = self.data.devices.as_ref().is_none_or(|d| list.no > d.no);
                let on_it = list.devices.iter().any(|d| d.ed25519 == me);
                let first = self.data.devices.is_none();
                if self.data.contacts[i].key != primary
                    || list.identity != primary
                    || !newer
                    || (first && !on_it)
                    || verify_devices(&list).is_err()
                {
                    return;
                }
                self.data.devices = Some(list);
                self.data.device_labels = labels;
                self.data.nickname = nickname;
                if on_it {
                    self.meet_siblings();
                }
                let note = match (first, on_it) {
                    (true, _) => "This device is linked.",
                    (false, false) => {
                        // Its groups end here too: the admin moves them without it.
                        for g in 0..self.data.groups.len() {
                            self.lose_group_access(g);
                        }
                        "This device was removed from your account: it reads and sends nothing more."
                    }
                    (false, true) => return,
                };
                self.happened.push(Happened::Notice(note.into()));
            }
            Body::LinkedContact { contact } if own && !self.is_primary() => {
                if self.data.contacts[i].key
                    == self.data.primary.as_ref().map_or([0; 32], |p| p.ed25519)
                {
                    self.take_contact(contact);
                }
            }
            Body::OwnQueue { contact, queue } if own => {
                let from = self.data.contacts[i].key;
                let Some(c) = self.index(&contact) else {
                    return;
                };
                if queue.device != from || self.data.contacts[c].own {
                    return;
                }
                let addr = Addr {
                    relay_url: queue.relay_url,
                    onion_url: queue.onion_url,
                    send_id: queue.send_id,
                };
                let s = &mut self.data.contacts[c].siblings;
                s.retain(|(k, _)| *k != from);
                s.push((from, addr));
                // The contact hears this address from us too (`tell_devices`).
                self.data.contacts[c].devices_told = 0;
                self.deliver(c);
            }
            Body::Devices { list, queues } if !own => self.their_devices(i, list, queues),
            Body::SiblingQueue { device, queue } if own => self.sibling_queue(i, device, queue),
            // Another device of ours joined: take our own invite, now or when it comes.
            Body::OwnGroupJoin { group } if own => {
                match self
                    .data
                    .group_invites
                    .iter()
                    .position(|x| x.roster.group == group)
                {
                    Some(n) => {
                        if let Err(e) = self.take_group_invite(n, false) {
                            self.note(e);
                        }
                    }
                    None if !self.data.pending_group_joins.contains(&group)
                        && !self.data.groups.iter().any(|g| g.id == group && !g.left) =>
                    {
                        self.data.pending_group_joins.push(group);
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    /// On a linked device: a channel to each other linked device we have
    /// none with yet. Its address goes through the primary (`introduce`).
    fn meet_siblings(&mut self) {
        let (me, primary) = (self.me(), self.data.primary.as_ref().map(|p| p.ed25519));
        let Some(list) = self.data.devices.clone() else {
            return;
        };
        for d in list.devices {
            if d.ed25519 == me || Some(d.ed25519) == primary || self.index(&d.ed25519).is_some() {
                continue;
            }
            // From our own list, signed by our primary.
            let _ = self.core().add_member(d.ed25519, d.curve25519);
            self.data.contacts.push(Contact {
                key: d.ed25519,
                own: true,
                link_ok: true,
                link_theirs: true,
                added_at: now(),
                next_client_id: getrandom::u64().unwrap_or(0) >> 2,
                ..Default::default()
            });
            self.persist();
            if let Err(e) = self.sync_links() {
                return self.note(e);
            }
            self.ensure_queue(d.ed25519);
        }
    }

    /// On a linked device, once our queue for another linked device exists:
    /// its address, to the primary to pass on.
    pub(super) fn introduce(&mut self, i: usize) {
        let c = &self.data.contacts[i];
        let primary = self.data.primary.as_ref().map(|p| p.ed25519);
        if !c.own || c.theirs.is_some() || Some(c.key) == primary {
            return;
        }
        let (Some(addr), Some(primary)) = (c.mine.as_ref().map(Queue::addr), primary) else {
            return;
        };
        let Some(p) = self.index(&primary) else {
            return;
        };
        let body = Body::SiblingQueue {
            device: c.key,
            queue: DeviceQueue {
                device: self.me(),
                relay_url: addr.relay_url,
                onion_url: addr.onion_url,
                send_id: addr.send_id,
            },
        };
        self.data.contacts[p].outbox.push(body);
        self.deliver(p);
    }

    /// A `SiblingQueue` from own device `i`: on the primary, passed on to the
    /// device it is for; on a linked device, the other's address to write to.
    fn sibling_queue(&mut self, i: usize, device: Ed25519Key, queue: DeviceQueue) {
        let from = self.data.contacts[i].key;
        if self.is_primary() {
            if queue.device != from
                || device == self.me()
                || !self.lists(&from)
                || !self.lists(&device)
            {
                return;
            }
            if let Some(o) = self.index(&device).filter(|&o| self.data.contacts[o].own) {
                self.data.contacts[o]
                    .outbox
                    .push(Body::SiblingQueue { device, queue });
                self.deliver(o);
            }
            return;
        }
        let primary = self.data.primary.as_ref().map(|p| p.ed25519);
        if Some(from) != primary || device != self.me() || !self.lists(&queue.device) {
            return;
        }
        if !self.peer_addr_ok(&queue.relay_url, queue.onion_url.as_deref()) {
            return;
        }
        self.meet_siblings();
        let Some(o) = self
            .index(&queue.device)
            .filter(|&o| self.data.contacts[o].own)
        else {
            return;
        };
        self.data.contacts[o].theirs = Some(Addr {
            relay_url: queue.relay_url,
            onion_url: queue.onion_url,
            send_id: queue.send_id,
        });
        self.persist();
        if let Err(e) = self.sync_links() {
            return self.note(e);
        }
        self.claim(queue.device);
    }

    /// Contact `i`'s device list, from any device of theirs: checked, then
    /// each device known by its keys and where it reads us.
    /// ADR-030, section 1, for states from before it: lists without device
    /// proofs no longer verify. Ours: the main device keeps itself and drops
    /// the others, to be linked again; a linked device sees it was removed.
    /// Contacts' and group members' old lists are forgotten until they send
    /// new ones.
    pub(super) fn drop_unproven_devices(&mut self) {
        let stale = |l: &DeviceList| termchat_core::verify_devices(l).is_err();
        let mut told = false;
        if self.data.devices.as_ref().is_some_and(stale) {
            let me = self.core().identity();
            let list = DeviceList {
                identity: me.ed25519,
                no: self.data.devices.as_ref().map_or(0, |l| l.no) + 1,
                devices: vec![Device {
                    ed25519: me.ed25519,
                    curve25519: me.curve25519,
                    proof: self.core().device_proof(&me.ed25519),
                }],
                signature: [0; 64],
            };
            if self.is_primary() {
                let gone: Vec<Ed25519Key> = self
                    .data
                    .contacts
                    .iter()
                    .filter(|c| c.own)
                    .map(|c| c.key)
                    .collect();
                let mut core = self.core();
                for k in &gone {
                    core.remove_contact(k);
                }
                drop(core);
                self.data.contacts.retain(|c| !c.own);
                let mut list = list;
                let signed = self.core().sign_devices(&list);
                match signed {
                    Ok(s) => list.signature = s,
                    Err(e) => return self.note(e.to_string()),
                }
                self.data.devices = Some(list);
                if !gone.is_empty() {
                    told = true;
                }
            } else {
                // Lists no other device: `removed()` holds, nothing more is sent.
                self.data.devices = Some(DeviceList {
                    devices: Vec::new(),
                    ..list
                });
                self.note(
                    "Security update (ADR-030): this linked device must be linked again. \
                     Erase it (Wipe, then MOVED) and choose \"Link to my main device\" on the first screen.",
                );
            }
        }
        for c in &mut self.data.contacts {
            if c.their_devices.as_ref().is_some_and(stale) {
                c.their_devices = None;
                c.device_addrs.clear();
                c.device_outbox.clear();
            }
        }
        for g in &mut self.data.groups {
            g.member_devices.retain(|(_, l, _)| !stale(l));
        }
        if told {
            self.note("Security update (ADR-030): link your other devices to your main one again.");
        }
    }

    /// ADR-030, section 3: `k` is someone's other than contact `i`'s: ours (our
    /// identity or one of our devices), a contact, a device another contact
    /// listed, or a member of one of our groups.
    pub(super) fn owned_elsewhere(&self, k: &Ed25519Key, i: usize) -> bool {
        let mine = *k == self.me()
            || *k == self.identity_key()
            || self
                .data
                .devices
                .as_ref()
                .is_some_and(|l| l.devices.iter().any(|d| d.ed25519 == *k));
        let contact = self.index(k).is_some_and(|j| j != i);
        let theirs = self.data.contacts.iter().enumerate().any(|(j, c)| {
            j != i
                && c.their_devices
                    .as_ref()
                    .is_some_and(|l| l.devices.iter().any(|d| d.ed25519 == *k))
        });
        let member = self.data.groups.iter().any(|g| {
            g.roster
                .as_ref()
                .is_some_and(|r| r.members.iter().any(|m| m.ed25519 == *k))
        }) && self.data.contacts.get(i).is_none_or(|c| c.key != *k);
        mine || contact || theirs || member
    }

    fn their_devices(&mut self, i: usize, list: DeviceList, queues: Vec<DeviceQueue>) {
        let key = self.data.contacts[i].key;
        let Some(slot) = self.slot_for(&key) else {
            return;
        };
        let older = self.data.contacts[i]
            .their_devices
            .as_ref()
            .is_some_and(|d| list.no < d.no);
        if list.identity != key || older || verify_devices(&list).is_err() {
            return;
        }
        // ADR-030, section 3: a list naming a key someone else holds is refused
        // whole (review 2026-11c, findings 1, 2, 4).
        let rekeyed = {
            let core = self.core_at(slot);
            list.devices.iter().any(|d| {
                d.ed25519 != key && core.curve_of(&d.ed25519).is_some_and(|c| c != d.curve25519)
            })
        };
        let refused = rekeyed
            || list
                .devices
                .iter()
                .any(|d| d.ed25519 != key && self.owned_elsewhere(&d.ed25519, i));
        if refused {
            self.note("a contact listed a device that belongs to someone else: ignored");
            return;
        }
        let listed = |k: &Ed25519Key| list.devices.iter().any(|d| d.ed25519 == *k);
        // Only keys this contact's own lists brought in, and nobody else's now.
        let gone: Vec<Ed25519Key> = self.data.contacts[i]
            .their_devices
            .iter()
            .flat_map(|d| d.devices.iter().map(|x| x.ed25519))
            .filter(|k| !listed(k) && *k != key && !self.owned_elsewhere(k, i))
            .collect();
        {
            let mut core = self.core_at(slot);
            for k in &gone {
                core.forget_sessions(k);
            }
            for d in list.devices.iter().filter(|d| d.ed25519 != key) {
                // Checked above: a key we hold has the same Curve25519 key here.
                let _ = core.add_member(d.ed25519, d.curve25519);
            }
        }
        let queues: Vec<_> = queues
            .into_iter()
            .filter(|q| {
                q.device != key
                    && listed(&q.device)
                    && self.peer_addr_ok(&q.relay_url, q.onion_url.as_deref())
            })
            .collect();
        let c = &mut self.data.contacts[i];
        c.device_addrs.retain(|(k, _)| listed(k));
        for q in queues {
            c.device_addrs.retain(|(k, _)| *k != q.device);
            c.device_addrs.push((
                q.device,
                Addr {
                    relay_url: q.relay_url,
                    onion_url: q.onion_url,
                    send_id: q.send_id,
                },
            ));
        }
        c.their_devices = Some(list);
        if let Err(e) = self.sync_links() {
            return self.note(e);
        }
        self.deliver(i);
        self.group_devices_changed(key, &gone);
    }

    /// Whoever else may write to our queue for contact `key`: their other
    /// devices, and ours with copies.
    pub(super) fn writers_of(&self, key: &Ed25519Key) -> Vec<Ed25519Key> {
        let Some(c) = self.index(key).map(|i| &self.data.contacts[i]) else {
            return Vec::new();
        };
        let theirs = c.their_devices.iter().flat_map(|d| &d.devices);
        theirs
            .map(|d| d.ed25519)
            .filter(|k| k != key)
            .chain(c.siblings.iter().map(|(k, _)| *k).filter(|k| self.lists(k)))
            .collect()
    }

    /// Where `body` for contact `i` goes besides their primary: each other
    /// device of theirs, and each of ours.
    fn copies_of(&self, i: usize) -> Vec<(Ed25519Key, Addr)> {
        let c = &self.data.contacts[i];
        let listed = |k: &Ed25519Key| {
            c.their_devices
                .as_ref()
                .is_some_and(|d| d.devices.iter().any(|x| x.ed25519 == *k))
        };
        let theirs = c.device_addrs.iter().filter(|(k, _)| listed(k));
        let ours = c.siblings.iter().filter(|(k, _)| self.lists(k));
        theirs.chain(ours).cloned().collect()
    }

    /// `body` to contact `key` on every device of theirs, and a copy to every
    /// device of ours, each into its own queue (ADR-028). The client id is
    /// the primary's: sent once one of theirs has it.
    pub(super) fn transmit_all(&mut self, key: &Ed25519Key, body: Body) -> Result<u64, String> {
        self.transmit_each(key, body, true)
    }

    /// `transmit_all`, with copies to our own devices only if `ours`: a
    /// document's later chunks go to the contact's devices alone.
    pub(super) fn transmit_each(
        &mut self,
        key: &Ed25519Key,
        body: Body,
        ours: bool,
    ) -> Result<u64, String> {
        let id = self.transmit(key, body.clone())?;
        let Some(i) = self.index(key) else {
            return Ok(id);
        };
        let slot = self.slot_for(key).ok_or(MISSING_PROFILE)?;
        let ours_too = |d: &Ed25519Key| ours || !self.lists(d);
        let targets: Vec<_> = self
            .copies_of(i)
            .into_iter()
            .filter(|(d, _)| ours_too(d))
            .collect();
        let theirs = self.data.contacts[i].their_devices.clone();
        let theirs = |d: &Ed25519Key| {
            theirs
                .as_ref()
                .is_some_and(|l| l.devices.iter().any(|x| x.ed25519 == *d))
        };
        let mut waiting = false;
        for (d, addr) in targets {
            if !self.core_at(slot).has_session(&d) {
                // A device of theirs new to us gets this once a session is up,
                // instead of never (`flush_devices`). Not file chunks, and only
                // the newest few: one that never answers must not grow the state.
                if theirs(&d) && !matches!(body, Body::File { .. }) {
                    let q = &mut self.data.contacts[i].device_outbox;
                    q.push((d, body.clone()));
                    if q.iter().filter(|(k, _)| *k == d).count() > WAITING_PER_DEVICE
                        && let Some(n) = q.iter().position(|(k, _)| *k == d)
                    {
                        q.remove(n);
                    }
                    waiting = true;
                } else if let Some(l) = self.live(key, &addr.relay_url) {
                    let _ = l.relay.send(ClientFrame::ClaimKey {
                        send_id: addr.send_id,
                    });
                }
                continue;
            }
            // ADR-030: a copy for one of our devices names the chat it is for.
            let what = match self.lists(&d) {
                true => Body::Copy {
                    contact: *key,
                    body: Box::new(body.clone()),
                },
                false => body.clone(),
            };
            let envelope = self.core_at(slot).encrypt(&d, what);
            let envelope = envelope.map_err(|e| e.to_string())?;
            let next = &mut self.data.contacts[i].next_client_id;
            let client_id = *next;
            *next += 1;
            if !self.persist() {
                return Err(self.take_note());
            }
            if let Some(l) = self.live(key, &addr.relay_url) {
                let _ = l.relay.send(ClientFrame::Send {
                    client_id,
                    send_id: addr.send_id,
                    envelope,
                });
            }
        }
        if waiting {
            self.persist();
            self.flush_devices(i);
        }
        Ok(id)
    }

    /// `body` to one device of contact `key`: now if there is a session with
    /// it, else once there is (`flush_devices`).
    pub(super) fn transmit_device(&mut self, key: &Ed25519Key, device: Ed25519Key, body: Body) {
        let Some(i) = self.index(key) else {
            return;
        };
        self.data.contacts[i].device_outbox.push((device, body));
        self.persist();
        self.flush_devices(i);
    }

    /// What waits for single devices of contact `i` and can go now.
    pub(super) fn flush_devices(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        let Some(slot) = self.slot_for(&key) else {
            return;
        };
        // Devices gone from their list hear nothing more.
        let c = &mut self.data.contacts[i];
        let listed = c.their_devices.clone();
        let listed = |d: &Ed25519Key| {
            listed
                .as_ref()
                .is_some_and(|l| l.devices.iter().any(|x| x.ed25519 == *d))
        };
        c.device_outbox.retain(|(d, _)| listed(d));
        let mut n = 0;
        while n < self.data.contacts[i].device_outbox.len() {
            let (d, body) = self.data.contacts[i].device_outbox[n].clone();
            let addr = self.data.contacts[i]
                .device_addrs
                .iter()
                .find(|(k, _)| *k == d)
                .map(|(_, a)| a.clone());
            let Some(addr) = addr else {
                n += 1;
                continue;
            };
            if !self.core_at(slot).has_session(&d) {
                // One claim per device until it answers or the reclaim timer.
                if !self.claiming.insert(d) {
                    n += 1;
                    continue;
                }
                if let Some(l) = self.live(&key, &addr.relay_url) {
                    let _ = l.relay.send(ClientFrame::ClaimKey {
                        send_id: addr.send_id,
                    });
                }
                n += 1;
                continue;
            }
            let envelope = self.core_at(slot).encrypt(&d, body);
            let Ok(envelope) = envelope else {
                n += 1;
                continue;
            };
            let next = &mut self.data.contacts[i].next_client_id;
            let client_id = *next;
            *next += 1;
            self.data.contacts[i].device_outbox.remove(n);
            if !self.persist() {
                return;
            }
            if let Some(l) = self.live(&key, &addr.relay_url) {
                let _ = l.relay.send(ClientFrame::Send {
                    client_id,
                    send_id: addr.send_id,
                    envelope,
                });
            }
        }
    }

    /// The contact with device `device` on their list, if any.
    pub(super) fn owner_of_device(&self, device: &Ed25519Key) -> Option<usize> {
        self.data.contacts.iter().position(|c| {
            !c.own
                && c.their_devices
                    .as_ref()
                    .is_some_and(|d| d.devices.iter().any(|x| x.ed25519 == *device))
        })
    }

    /// A key claimed from a queue of one of the contact's other devices, or
    /// of one of ours: a session with that device.
    pub(super) fn claimed_device_key(
        &mut self,
        who: &Ed25519Key,
        send_id: QueueId,
        key: &SignedKey,
    ) {
        let found = (0..self.data.contacts.len()).find_map(|i| {
            let c = &self.data.contacts[i];
            (self.link_who(&c.key) == *who)
                .then(|| self.copies_of(i))
                .and_then(|all| all.into_iter().find(|(_, a)| a.send_id == send_id))
                .map(|(d, _)| (i, d))
        });
        let Some((i, d)) = found else {
            return;
        };
        let Some(slot) = self.slot_for(&self.data.contacts[i].key) else {
            return;
        };
        if self.core_at(slot).has_session(&d) {
            return;
        }
        self.claiming.remove(&d);
        let started = self.core_at(slot).start_session(&d, key);
        if let Err(e) = started {
            return self.note(format!("cannot start session: {e}"));
        }
        self.persist();
        self.flush(i);
    }

    /// What another device of ours sent contact `i`: ours here too.
    pub(super) fn own_copy(&mut self, i: usize, body: Body) {
        let c = &mut self.data.contacts[i];
        match body {
            Body::Message {
                id,
                expires_in,
                reply_to,
                text,
                ..
            } => {
                let at = now();
                c.messages.push(Msg {
                    mine: true,
                    text,
                    at,
                    expires_at: expires_in.map(|s| at + u64::from(s)),
                    status: Status::Sent,
                    file: None,
                    unfetched: false,
                    id: Some(id),
                    reply_to,
                    ..Default::default()
                });
            }
            Body::Text {
                expires_in, text, ..
            } => {
                let at = now();
                c.messages.push(Msg {
                    mine: true,
                    text,
                    at,
                    expires_at: expires_in.map(|s| at + u64::from(s)),
                    status: Status::Sent,
                    file: None,
                    unfetched: false,
                    ..Default::default()
                });
            }
            body @ (Body::Edit { .. } | Body::Delete { .. } | Body::React { .. }) => {
                c.own_change(body);
            }
            // A document we sent from another device: its line, not its
            // bytes, which stay on the device that sent it (first chunk only).
            Body::File {
                id,
                name,
                size,
                index: 0,
                ..
            } => {
                let total =
                    u16::try_from(termchat_core::proto::file_chunks(size)).unwrap_or(u16::MAX);
                c.messages.push(Msg {
                    mine: true,
                    text: name.clone(),
                    at: now(),
                    expires_at: None,
                    status: Status::Sent,
                    file: Some(FileMsg {
                        id,
                        name,
                        size,
                        done: total,
                        data: None,
                    }),
                    unfetched: false,
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    /// A contact our primary handed over: we write to them as it does, from
    /// a queue of our own.
    fn take_contact(&mut self, l: LinkedContact) {
        let key = l.identity.ed25519;
        if key == self.me() || self.index(&key).is_some() {
            return;
        }
        let Some(theirs) = l.queues.iter().find(|q| q.device == key) else {
            return;
        };
        let theirs = Addr {
            relay_url: theirs.relay_url.clone(),
            onion_url: theirs.onion_url.clone(),
            send_id: theirs.send_id,
        };
        if !self.peer_addr_ok(&theirs.relay_url, theirs.onion_url.as_deref()) {
            return;
        }
        {
            let mut core = self.core();
            core.add_from_primary(key, l.identity.curve25519);
            core.set_nickname(&key, l.nickname);
        }
        self.data.push_person(Contact {
            key,
            petname: l.petname,
            verified: l.verified,
            understands: l.understands,
            // They already know what we read; the primary told them.
            told: proto::UNDERSTANDS,
            theirs: Some(theirs),
            added_at: now(),
            next_client_id: getrandom::u64().unwrap_or(0) >> 2,
            ..Default::default()
        });
        self.persist();
        if let Err(e) = self.sync_links() {
            return self.note(e);
        }
        self.ensure_queue(key);
        self.claim(key);
    }
}
