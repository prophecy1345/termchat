//! Devices of one identity (ADR-028): a new device asks with a link code,
//! the primary answers, both screens show the identity's fingerprint, and
//! once the person confirms on both the primary signs the new device list.
//!
//! An own device is kept as a contact with `own` set: the same queues,
//! sessions and links as a person, never shown as one.

use super::*;
use termchat_core::proto::{Device, DeviceList, MAX_DEVICES};
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
        self.note("device linked");
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
                self.data.contacts[i].own = true;
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
                let newer = self.data.devices.as_ref().is_none_or(|d| list.no > d.no);
                if self.data.contacts[i].key != primary
                    || list.identity != primary
                    || !newer
                    || !list.devices.iter().any(|d| d.ed25519 == me)
                    || verify_devices(&list).is_err()
                {
                    return;
                }
                let first = self.data.devices.is_none();
                self.data.devices = Some(list);
                self.data.device_labels = labels;
                self.data.nickname = nickname;
                if first {
                    self.happened
                        .push(Happened::Notice("This device is linked.".into()));
                }
            }
            _ => {}
        }
    }
}
