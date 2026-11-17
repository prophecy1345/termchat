//! Small private groups (ADR-027): the group queue and its readers on the
//! admin's relay, Megolm sessions shared over Olm, a roster the admin signs.
//! A child of `engine`, for its private state.

use super::*;
use crate::data::{Group, GroupInvite};
use termchat_core::proto::{
    Envelope, GroupId, OLM_TYPE_MEGOLM, ReaderGrant, Roster, RosterMember, SecretBytes,
};
use zeroize::Zeroizing;

/// Group messages kept while their sender's key is on its way.
const MAX_EARLY: usize = 50;

/// The link that carries a group's queue: its own, like a contact's.
pub(super) fn link_who(id: &GroupId) -> Ed25519Key {
    let mut who = [0xff; 32];
    who[..16].copy_from_slice(id);
    who
}

impl Engine {
    pub(super) fn group_of_link(&self, link: &Link) -> Option<usize> {
        self.data
            .groups
            .iter()
            .position(|g| link_who(&g.id) == link.who && same_url(&g.relay_url, &link.relay))
    }

    /// The links groups need, for `start_links`.
    pub(super) fn group_links(&self) -> Vec<(Link, Option<String>)> {
        self.data
            .groups
            .iter()
            .filter(|g| !g.left)
            .map(|g| {
                let link = Link {
                    who: link_who(&g.id),
                    relay: g.relay_url.clone(),
                };
                (link, g.onion_url.clone())
            })
            .collect()
    }

    fn group_send(&self, g: usize, frame: ClientFrame) -> bool {
        let group = &self.data.groups[g];
        let link = Link {
            who: link_who(&group.id),
            relay: group.relay_url.clone(),
        };
        match self.links.get(&link) {
            Some(l) if l.connected => l.relay.send(frame).is_ok(),
            _ => false,
        }
    }

    /// A new group with us as its admin and only member, on our relay.
    pub fn create_group(&mut self, name: &str) -> Result<usize, String> {
        let name = name.trim();
        proto::check_group_name(name)
            .map_err(|_| "a group name is 1 to 64 bytes, no control characters")?;
        let mut id = [0; 16];
        getrandom::fill(&mut id).map_err(|e| e.to_string())?;
        let me = self.me();
        self.core()
            .group_start(id, now())
            .map_err(|e| e.to_string())?;
        self.data.groups.push(Group {
            id,
            roster: Some(Roster {
                group: id,
                no: 0,
                admin: me,
                name: name.to_string(),
                members: Vec::new(),
                signature: [0; 64],
            }),
            relay_url: self.home(),
            onion_url: self.data.onion_url.clone(),
            asking: vec![me],
            next_client_id: getrandom::u64().unwrap_or(0) >> 2,
            ..Default::default()
        });
        self.persist();
        self.sync_links()?;
        Ok(self.data.groups.len() - 1)
    }

    /// The admin asks contact `i` into group `g` (ADR-027): a reader first,
    /// the invite once the relay made it.
    pub fn invite_to_group(&mut self, g: usize, i: usize) -> Result<(), String> {
        let c = self.writable(i)?;
        if c.profile.is_some() {
            return Err("groups use your main identity; this contact knows another".into());
        }
        if c.understands < proto::UNDERSTANDS {
            return Err("their app is older; groups come after they update".into());
        }
        let key = c.key;
        let group = &self.data.groups[g];
        if !group.is_admin() {
            return Err("only the group's admin adds members".into());
        }
        if group.members().iter().any(|m| m.ed25519 == key) || group.asking.contains(&key) {
            return Err("already in the group or invited".into());
        }
        if group.members().len() + group.invited.len() + group.asking.len()
            >= proto::MAX_GROUP_MEMBERS
        {
            return Err("a group has at most 32 members".into());
        }
        self.data.groups[g].asking.push(key);
        self.persist();
        if let Some((group_id, token)) = self.data.groups[g].admin {
            self.group_send(g, ClientFrame::AddReader { group_id, token });
        }
        Ok(())
    }

    /// Takes invite `n`: we join the group, share our key, tell the admin.
    pub fn accept_group(&mut self, n: usize) -> Result<usize, String> {
        let inv = self
            .data
            .group_invites
            .get(n)
            .cloned()
            .ok_or("no such invite")?;
        verify_roster(&inv.roster).map_err(|_| "the invite's roster is not signed by its admin")?;
        if inv.roster.admin != inv.from {
            return Err("the invite does not come from the group's admin".into());
        }
        self.data.group_invites.remove(n);
        let id = inv.roster.group;
        if self.data.groups.iter().any(|g| g.id == id && !g.left) {
            return Err("already in this group".into());
        }
        self.core()
            .group_start(id, now())
            .map_err(|e| e.to_string())?;
        self.data.groups.push(Group {
            id,
            roster: Some(inv.roster),
            relay_url: inv.relay_url,
            onion_url: inv.onion_url,
            send_id: inv.send_id,
            reader: Some(inv.reader),
            next_client_id: getrandom::u64().unwrap_or(0) >> 2,
            ..Default::default()
        });
        let g = self.data.groups.len() - 1;
        self.know_members(g);
        if let Err(e) = self.transmit(&inv.from, Body::GroupJoin { group: id }) {
            self.note(e);
        }
        self.persist();
        self.sync_links()?;
        self.share_key(g);
        Ok(g)
    }

    /// Writes `text` to group `g`.
    pub fn send_group(&mut self, g: usize, text: &str) -> Result<(), String> {
        if text.len() > proto::MAX_TEXT_BYTES {
            return Err("message too long".into());
        }
        let group = self.data.groups.get(g).ok_or("no such group")?;
        if group.left {
            return Err("you are not in this group any more".into());
        }
        let id = getrandom::u64().map_err(|e| e.to_string())?;
        let at = now();
        let expires_in = group.expires_in;
        self.data.groups[g].messages.push(Msg {
            mine: true,
            text: text.to_string(),
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Queued,
            file: None,
            id: Some(id),
            ..Default::default()
        });
        self.persist();
        self.flush_group(g);
        Ok(())
    }

    /// Sends what waits in group `g`, when its link is up.
    pub(super) fn flush_group(&mut self, g: usize) {
        let gid = self.data.groups[g].id;
        if self.core().group_due(&gid, now()) {
            self.rotate_group(g);
        }
        let expires_in = self.data.groups[g].expires_in;
        for m in 0..self.data.groups[g].messages.len() {
            let msg = &self.data.groups[g].messages[m];
            if msg.status != Status::Queued || self.data.groups[g].send_id == [0; 16] {
                continue;
            }
            let body = Body::Message {
                id: msg.id.unwrap_or(0),
                sent_at: msg.at,
                expires_in,
                reply_to: msg.reply_to,
                text: msg.text.clone(),
            };
            let sealed = self.core().group_encrypt(gid, body);
            let bytes = match sealed {
                Ok(b) => b,
                Err(e) => return self.note(format!("group: {e}")),
            };
            // The ratchet moved: saved before anything goes out (as `transmit`).
            let client_id = self.data.groups[g].next_client_id;
            self.data.groups[g].next_client_id += 1;
            if !self.persist() {
                return;
            }
            let frame = ClientFrame::Send {
                client_id,
                send_id: self.data.groups[g].send_id,
                envelope: Envelope {
                    version: proto::VERSION,
                    olm_type: OLM_TYPE_MEGOLM,
                    body: bytes,
                },
            };
            if self.group_send(g, frame) {
                self.data.groups[g].messages[m].status = Status::Sending(client_id);
            }
        }
        self.persist();
    }

    /// A new Megolm session for group `g`, shared with every member again.
    fn rotate_group(&mut self, g: usize) {
        let gid = self.data.groups[g].id;
        if self.core().group_start(gid, now()).is_err() {
            return;
        }
        let group = &mut self.data.groups[g];
        group.shared.clear();
        // Everyone in the group now reads the new session from its start.
        group.since = group.members().iter().map(|m| (m.ed25519, 0)).collect();
        self.share_key(g);
    }

    /// Our key for member `who`: from the message they joined at, so they
    /// read nothing from before (ADR-027).
    fn key_for(&mut self, g: usize, who: &Ed25519Key) -> Option<Zeroizing<Vec<u8>>> {
        let gid = self.data.groups[g].id;
        let since = self.data.groups[g]
            .since
            .iter()
            .find(|(k, _)| k == who)
            .map(|(_, i)| *i);
        let mut core = self.core();
        let at = since.or_else(|| core.group_index(&gid))?;
        core.group_key_at(&gid, at)
    }

    /// Hands our current key to each member who lacks it: over their chat if
    /// they are our contact, else through their reader, with a key claimed
    /// from it (the answer comes as `ClaimedKey`).
    pub(super) fn share_key(&mut self, g: usize) {
        let me = self.me();
        let gid = self.data.groups[g].id;
        let members: Vec<RosterMember> = self.data.groups[g].members().to_vec();
        for m in members {
            if m.ed25519 == me || self.data.groups[g].shared.contains(&m.ed25519) {
                continue;
            }
            let contact = self.index(&m.ed25519).filter(|&i| {
                self.data.contacts[i].profile.is_none() && self.has_session(&m.ed25519)
            });
            if contact.is_some() {
                let Some(key) = self.key_for(g, &m.ed25519) else {
                    return;
                };
                let body = Body::GroupKey {
                    group: gid,
                    key: SecretBytes(key.to_vec()),
                };
                if self.transmit(&m.ed25519, body).is_ok() {
                    self.data.groups[g].shared.push(m.ed25519);
                }
            } else {
                self.group_send(g, ClientFrame::ClaimKey { send_id: m.reader });
            }
        }
        self.persist();
    }

    /// Members we have never talked to become known to the crypto core by
    /// the keys the admin signed for.
    /// Remembers, for each member new to us, the message they joined at.
    fn know_members(&mut self, g: usize) {
        let me = self.me();
        let gid = self.data.groups[g].id;
        let members: Vec<RosterMember> = self.data.groups[g].members().to_vec();
        let mut core = self.core();
        let at = core.group_index(&gid).unwrap_or(0);
        for m in members.iter().filter(|m| m.ed25519 != me) {
            core.add_member(m.ed25519, m.curve25519);
        }
        drop(core);
        let since = &mut self.data.groups[g].since;
        for m in members.iter().filter(|m| m.ed25519 != me) {
            if !since.iter().any(|(k, _)| *k == m.ed25519) {
                since.push((m.ed25519, at));
            }
        }
    }

    /// The link of group `g` is up: what the relay should know or do now.
    pub(super) fn group_connected(&mut self, g: usize) {
        let group = &self.data.groups[g];
        match group.admin {
            None if group.is_admin_to_be() => {
                self.group_send(g, ClientFrame::CreateGroup);
            }
            Some((group_id, token)) => {
                for _ in 0..group.asking.len() {
                    self.group_send(g, ClientFrame::AddReader { group_id, token });
                }
            }
            None => {}
        }
        if let Some(r) = self.data.groups[g].reader.clone() {
            self.group_send(
                g,
                ClientFrame::Subscribe {
                    recv_id: r.recv_id,
                    token: r.token,
                },
            );
        }
        self.share_key(g);
        self.flush_group(g);
    }

    pub(super) fn on_group_frame(&mut self, g: usize, frame: ServerFrame) {
        match frame {
            ServerFrame::GroupCreated {
                group_id,
                send_id,
                token,
            } => {
                let group = &mut self.data.groups[g];
                if group.admin.is_some() {
                    return;
                }
                group.admin = Some((group_id, token));
                group.send_id = send_id;
                self.persist();
                for _ in 0..self.data.groups[g].asking.len() {
                    self.group_send(g, ClientFrame::AddReader { group_id, token });
                }
            }
            ServerFrame::ReaderAdded {
                recv_id,
                send_id,
                token,
                ..
            } => {
                let grant = ReaderGrant {
                    recv_id,
                    send_id,
                    token,
                };
                if self.data.groups[g].asking.is_empty() {
                    return;
                }
                let who = self.data.groups[g].asking.remove(0);
                if who == self.me() {
                    self.data.groups[g].reader = Some(grant.clone());
                    self.add_to_roster(g, who, grant.send_id);
                    self.group_send(g, ClientFrame::Subscribe { recv_id, token });
                    self.persist();
                    self.flush_group(g);
                } else {
                    self.data.groups[g].invited.push((who, grant.clone()));
                    self.persist();
                    let group = &self.data.groups[g];
                    let body = Body::GroupInvite {
                        roster: group
                            .roster
                            .clone()
                            .unwrap_or_else(|| empty_roster(group.id)),
                        relay_url: group.relay_url.clone(),
                        onion_url: group.onion_url.clone(),
                        send_id: group.send_id,
                        reader: grant,
                    };
                    if let Err(e) = self.transmit(&who, body) {
                        self.note(e);
                    }
                }
            }
            ServerFrame::KeyCount {
                recv_id,
                one_time_left,
                ..
            } => {
                let Some(r) = self.data.groups[g].reader.clone() else {
                    return;
                };
                if r.recv_id != recv_id || one_time_left >= CONTACT_KEYS {
                    return;
                }
                let one_time = self
                    .core()
                    .one_time_keys((CONTACT_KEYS - one_time_left) as usize);
                self.persist();
                self.group_send(
                    g,
                    ClientFrame::PublishKeys {
                        recv_id: r.recv_id,
                        token: r.token,
                        one_time,
                        fallback: None,
                    },
                );
            }
            ServerFrame::ClaimedKey { send_id, key } => {
                let Some(m) = self.data.groups[g]
                    .members()
                    .iter()
                    .find(|m| m.reader == send_id)
                    .cloned()
                else {
                    return;
                };
                let gid = self.data.groups[g].id;
                let Some(session_key) = self.key_for(g, &m.ed25519) else {
                    return;
                };
                let envelope = {
                    let mut core = self.core();
                    if core.start_session(&m.ed25519, &key).is_err() {
                        return;
                    }
                    core.encrypt(
                        &m.ed25519,
                        Body::GroupKey {
                            group: gid,
                            key: SecretBytes(session_key.to_vec()),
                        },
                    )
                };
                let Ok(envelope) = envelope else { return };
                if !self.persist() {
                    return;
                }
                let client_id = self.data.groups[g].next_client_id;
                self.data.groups[g].next_client_id += 1;
                if self.group_send(
                    g,
                    ClientFrame::Send {
                        client_id,
                        send_id,
                        envelope,
                    },
                ) {
                    self.data.groups[g].shared.push(m.ed25519);
                    self.persist();
                }
            }
            ServerFrame::Deliver {
                recv_id,
                relay_id,
                envelope,
            } => {
                if self.data.groups[g]
                    .reader
                    .as_ref()
                    .is_none_or(|r| r.recv_id != recv_id)
                {
                    return;
                }
                if self.group_receive(g, &envelope) {
                    self.group_send(g, ClientFrame::Ack { recv_id, relay_id });
                }
            }
            ServerFrame::Accepted { client_id } => {
                for m in &mut self.data.groups[g].messages {
                    if m.status == Status::Sending(client_id) {
                        m.status = Status::Sent;
                    }
                }
                self.persist();
            }
            // A member's reader has no key yet: try again shortly, as for a contact.
            ServerFrame::Error {
                code: ErrorCode::NoKeys,
                ..
            } => self.reclaim = Some(Instant::now() + RECLAIM_AFTER),
            ServerFrame::Error { code, .. } => self.note(format!("group: {}", why(code))),
            _ => {}
        }
    }

    /// One envelope from our reader; `true` once it is dealt with and saved.
    fn group_receive(&mut self, g: usize, envelope: &Envelope) -> bool {
        let gid = self.data.groups[g].id;
        if envelope.olm_type == OLM_TYPE_MEGOLM {
            let got = self.core().group_decrypt(&gid, &envelope.body);
            match got {
                Ok((sender, body)) => self.group_body(g, sender, body),
                Err(termchat_core::Error::UnknownSender) => {
                    let early = &mut self.data.groups[g].early;
                    if early.len() < MAX_EARLY {
                        early.push(envelope.body.clone());
                    }
                }
                Err(_) => {}
            }
            return self.persist();
        }
        // A session key from a member, over Olm.
        let got = self.core().decrypt(None, envelope);
        if let Ok(r) = got {
            self.group_key_from(r.sender, r.body);
        }
        self.persist()
    }

    /// A `GroupKey` from `sender`, over their chat or our reader.
    pub(super) fn group_key_from(&mut self, sender: Ed25519Key, body: Body) {
        let Body::GroupKey { group, key } = body else {
            return;
        };
        let Some(g) = self.data.groups.iter().position(|x| x.id == group) else {
            return;
        };
        if !self.data.groups[g]
            .members()
            .iter()
            .any(|m| m.ed25519 == sender)
        {
            return;
        }
        if self
            .core()
            .group_member_key(&group, sender, &key.0)
            .is_err()
        {
            return;
        }
        // What came before the key reads now.
        let early = std::mem::take(&mut self.data.groups[g].early);
        for bytes in early {
            let env = Envelope {
                version: proto::VERSION,
                olm_type: OLM_TYPE_MEGOLM,
                body: bytes,
            };
            self.group_receive(g, &env);
        }
        self.persist();
    }

    /// What a member wrote in group `g`.
    fn group_body(&mut self, g: usize, sender: Ed25519Key, body: Body) {
        if sender == self.me() {
            return;
        }
        match body {
            Body::Message {
                id,
                expires_in,
                reply_to,
                text,
                ..
            } => {
                let at = now();
                let group = &mut self.data.groups[g];
                group.messages.push(Msg {
                    mine: false,
                    text,
                    at,
                    expires_at: expires_in.map(|s| at + u64::from(s)),
                    status: Status::Received,
                    file: None,
                    author: Some(sender),
                    id: Some(id),
                    reply_to,
                    unfetched: true,
                    ..Default::default()
                });
                group.unread += 1;
            }
            Body::GroupRoster { roster } => self.take_roster(g, sender, roster),
            _ => {}
        }
    }

    /// A newer roster, from the admin, signed: new members learn our key.
    fn take_roster(&mut self, g: usize, sender: Ed25519Key, roster: Roster) {
        let current = self.data.groups[g].roster.as_ref();
        let admin = current.map(|r| r.admin);
        if Some(sender) != admin
            || Some(roster.admin) != admin
            || roster.group != self.data.groups[g].id
            || current.is_some_and(|r| roster.no <= r.no)
            || verify_roster(&roster).is_err()
        {
            return;
        }
        self.data.groups[g].roster = Some(roster);
        self.know_members(g);
        self.share_key(g);
    }

    /// The admin puts `who` on the roster, signs it and tells the group.
    fn add_to_roster(&mut self, g: usize, who: Ed25519Key, reader: QueueId) {
        let nickname = match self.index(&who) {
            Some(i) => clean(&self.name(&self.data.contacts[i])),
            None => self.data.nickname.clone(),
        };
        // One lock at a time: the core's mutex is not reentrant.
        let me = self.me();
        let curve = self.core().curve_of(&who);
        let Some(curve25519) =
            curve.or_else(|| (who == me).then(|| self.core().identity().curve25519))
        else {
            return;
        };
        let group = &mut self.data.groups[g];
        let Some(roster) = group.roster.as_mut() else {
            return;
        };
        roster.members.retain(|m| m.ed25519 != who);
        roster.members.push(RosterMember {
            ed25519: who,
            curve25519,
            nickname,
            reader,
        });
        roster.no += 1;
        let bytes = self.data.groups[g]
            .roster
            .as_ref()
            .map(Roster::signed_bytes)
            .unwrap_or_default();
        let signature = self.core().sign_roster(&bytes);
        if let Some(r) = self.data.groups[g].roster.as_mut() {
            r.signature = signature;
        }
        self.persist();
        let Some(roster) = self.data.groups[g].roster.clone() else {
            return;
        };
        // The newcomer gets our key first: a key reads only what is sent
        // after it, and the roster that names them must read (ADR-027).
        self.know_members(g);
        self.share_key(g);
        if roster.members.len() > 1 {
            self.send_group_body(g, Body::GroupRoster { roster });
        }
    }

    /// A body for everyone in group `g` that is not a message one sees.
    fn send_group_body(&mut self, g: usize, body: Body) {
        let gid = self.data.groups[g].id;
        let Ok(bytes) = self.core().group_encrypt(gid, body) else {
            return;
        };
        let client_id = self.data.groups[g].next_client_id;
        self.data.groups[g].next_client_id += 1;
        if !self.persist() {
            return;
        }
        self.group_send(
            g,
            ClientFrame::Send {
                client_id,
                send_id: self.data.groups[g].send_id,
                envelope: Envelope {
                    version: proto::VERSION,
                    olm_type: OLM_TYPE_MEGOLM,
                    body: bytes,
                },
            },
        );
    }

    /// Group bodies that come over a chat with contact `i`.
    pub(super) fn group_from_contact(&mut self, i: usize, body: Body) {
        let from = self.data.contacts[i].key;
        match body {
            Body::GroupInvite {
                roster,
                relay_url,
                onion_url,
                send_id,
                reader,
            } => {
                let name = roster.name.clone();
                self.data
                    .group_invites
                    .retain(|x| x.roster.group != roster.group);
                self.data.group_invites.push(GroupInvite {
                    from,
                    roster,
                    relay_url,
                    onion_url,
                    send_id,
                    reader,
                });
                self.system(i, format!("invites you to the group \"{}\"", clean(&name)));
            }
            Body::GroupJoin { group } => {
                let Some(g) = self
                    .data
                    .groups
                    .iter()
                    .position(|x| x.id == group && x.is_admin())
                else {
                    return;
                };
                let Some(at) = self.data.groups[g]
                    .invited
                    .iter()
                    .position(|(k, _)| *k == from)
                else {
                    return;
                };
                let (_, grant) = self.data.groups[g].invited.remove(at);
                self.add_to_roster(g, from, grant.send_id);
            }
            body @ Body::GroupKey { .. } => self.group_key_from(from, body),
            _ => {}
        }
        self.persist();
    }
}

impl Group {
    /// Ours, before the relay made it: we are its only roster entry's admin.
    fn is_admin_to_be(&self) -> bool {
        self.admin.is_none() && self.send_id == [0; 16] && self.reader.is_none()
    }
}

fn empty_roster(group: GroupId) -> Roster {
    Roster {
        group,
        no: 0,
        admin: [0; 32],
        name: String::new(),
        members: Vec::new(),
        signature: [0; 64],
    }
}

fn verify_roster(r: &Roster) -> Result<(), termchat_core::Error> {
    termchat_core::verify_roster(&r.admin, &r.signed_bytes(), &r.signature)
}
