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

impl Engine {
    /// The primary, or a device on its own: only it links other devices.
    pub fn is_primary(&self) -> bool {
        self.data.primary.is_none()
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
        if !addr_ok(&link.relay_url, link.onion_url.as_deref()) {
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

    /// The device waiting for our person to compare fingerprints.
    pub fn pending_link(&self) -> Option<usize> {
        self.data.contacts.iter().position(|c| c.own && !c.link_ok)
    }

    /// The fingerprint both screens show while linking: the identity's.
    pub fn link_fingerprint(&self) -> String {
        let id = self.data.primary.as_ref().map_or(self.me(), |p| p.ed25519);
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
        // Our queue for the primary goes first, then that we agree.
        self.data.contacts[i].outbox.push(Body::DeviceLinked);
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
    fn lists(&self, key: &Ed25519Key) -> bool {
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
        let Some(curve25519) = self.core().curve_of(&key) else {
            return;
        };
        let me = self.core().identity();
        let mut list = self.data.devices.clone().unwrap_or_else(|| DeviceList {
            identity: me.ed25519,
            no: 0,
            devices: vec![Device {
                ed25519: me.ed25519,
                curve25519: me.curve25519,
            }],
            signature: [0; 64],
        });
        list.devices.push(Device {
            ed25519: key,
            curve25519,
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
        for c in &self.data.contacts {
            let Some(theirs) = c.theirs.as_ref() else {
                continue;
            };
            if c.own || c.profile.is_some() || c.blocked || c.left || self.is_request(&c.key) {
                continue;
            }
            let core = self.core();
            let Some(curve25519) = core.curve_of(&c.key) else {
                continue;
            };
            let nickname = core.nickname(&c.key).unwrap_or("?").to_string();
            drop(core);
            bodies.push(Body::LinkedContact {
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
            });
        }
        self.data.contacts[i].outbox.extend(bodies);
        self.deliver(i);
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
        // shortcut: only devices with a channel to us hear (the primary and
        // the devices it linked); linked devices among themselves come later.
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
                self.data.primary = Some(identity);
                self.happened.push(Happened::Notice(
                    "Your main device answered: compare the fingerprints on both screens.".into(),
                ));
            }
            Body::DeviceLinked if own && self.is_primary() => {
                self.data.contacts[i].link_theirs = true;
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
                let note = match (first, on_it) {
                    (true, _) => "This device is linked.",
                    (false, false) => {
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
            _ => {}
        }
    }

    /// Contact `i`'s device list, from any device of theirs: checked, then
    /// each device known by its keys and where it reads us.
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
        let listed = |k: &Ed25519Key| list.devices.iter().any(|d| d.ed25519 == *k);
        let gone: Vec<Ed25519Key> = self.data.contacts[i]
            .their_devices
            .iter()
            .flat_map(|d| d.devices.iter().map(|x| x.ed25519))
            .filter(|k| !listed(k) && *k != key)
            .collect();
        {
            let mut core = self.core_at(slot);
            for k in &gone {
                core.forget_sessions(k);
            }
            for d in list.devices.iter().filter(|d| d.ed25519 != key) {
                core.add_member(d.ed25519, d.curve25519);
            }
        }
        let c = &mut self.data.contacts[i];
        c.device_addrs.retain(|(k, _)| listed(k));
        for q in queues {
            if q.device == key
                || !listed(&q.device)
                || !addr_ok(&q.relay_url, q.onion_url.as_deref())
            {
                continue;
            }
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
        let id = self.transmit(key, body.clone())?;
        let Some(i) = self.index(key) else {
            return Ok(id);
        };
        let slot = self.slot_for(key).ok_or(MISSING_PROFILE)?;
        for (d, addr) in self.copies_of(i) {
            if !self.core_at(slot).has_session(&d) {
                if let Some(l) = self.live(key, &addr.relay_url) {
                    let _ = l.relay.send(ClientFrame::ClaimKey {
                        send_id: addr.send_id,
                    });
                }
                continue;
            }
            let envelope = self.core_at(slot).encrypt(&d, body.clone());
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
        Ok(id)
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
            // shortcut: documents and the rest stay on the device that sent
            // them; copy them too if people miss them on their other devices.
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
        if !addr_ok(&theirs.relay_url, theirs.onion_url.as_deref()) {
            return;
        }
        {
            let mut core = self.core();
            core.add_member(key, l.identity.curve25519);
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
