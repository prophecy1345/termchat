//! Small private groups (ADR-027): the group queue and its readers on the
//! admin's relay, Megolm sessions shared over Olm, a roster the admin signs.
//! A child of `engine`, for its private state.

use super::*;
use crate::data::{Group, GroupInvite, GroupMoving};
use termchat_core::proto::{
    Envelope, GroupId, OLM_TYPE_MEGOLM, ReaderGrant, Ref, Roster, RosterMember, SecretBytes,
};
use zeroize::Zeroizing;

/// Group messages kept while their sender's key is on its way.
const MAX_EARLY: usize = 50;
/// Keys from members a roster has not named yet; a group has at most 32.
const MAX_PENDING_KEYS: usize = 32;
/// The `Understands` level that knows groups (ADR-027).
const GROUPS_LEVEL: u16 = 2;

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
            // A group we left keeps its link until its goodbye is out.
            .filter(|g| !g.left || !g.outbox.is_empty())
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

    /// Reads our reader of group `g`, and asks the relay to wake us for it
    /// when a push endpoint is set (ADR-012): a reader is a queue like any.
    fn subscribe_group(&self, g: usize) {
        let group = &self.data.groups[g];
        let Some(r) = group.reader.clone() else {
            return;
        };
        let push = group.push.clone();
        self.group_send(
            g,
            ClientFrame::Subscribe {
                recv_id: r.recv_id,
                token: r.token,
            },
        );
        if push.is_some() {
            self.group_send(
                g,
                ClientFrame::SetPush {
                    recv_id: r.recv_id,
                    token: r.token,
                    endpoint: push,
                },
            );
        }
    }

    /// The UnifiedPush endpoint for group `id`'s reader, or none (ADR-012).
    pub fn set_group_push(&mut self, id: &GroupId, endpoint: Option<String>) {
        let Some(g) = self.data.groups.iter().position(|g| g.id == *id) else {
            return;
        };
        self.data.groups[g].push = endpoint.clone();
        self.persist();
        if let Some(r) = self.data.groups[g].reader.clone() {
            self.group_send(
                g,
                ClientFrame::SetPush {
                    recv_id: r.recv_id,
                    token: r.token,
                    endpoint,
                },
            );
        }
    }

    /// A push for group `id`: its link checks itself.
    pub fn woken_group(&self, id: &GroupId) {
        let Some(group) = self.data.groups.iter().find(|g| g.id == *id) else {
            return;
        };
        let link = Link {
            who: link_who(&group.id),
            relay: group.relay_url.clone(),
        };
        if let Some(l) = self.links.get(&link) {
            l.relay.check();
        }
    }

    /// A new group with us as its admin and only member, on our relay.
    pub fn create_group(&mut self, name: &str) -> Result<usize, String> {
        // ADR-028: the admin's token lives on the main device.
        if !self.is_primary() {
            return Err("create groups on your main device".into());
        }
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
                expires_in: None,
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
    /// Contacts the admin of group `g` could invite: main identity, accepted,
    /// not in the group yet. Older apps stay listed; inviting says why not.
    pub fn invitable(&self, g: usize) -> Vec<usize> {
        let Some(group) = self.data.groups.get(g) else {
            return Vec::new();
        };
        (0..self.data.contacts.len())
            .filter(|&i| {
                let c = &self.data.contacts[i];
                c.profile.is_none()
                    && !c.left
                    && !c.blocked
                    && !self.is_request(&c.key)
                    && !group.members().iter().any(|m| m.ed25519 == c.key)
            })
            .collect()
    }

    pub fn invite_to_group(&mut self, g: usize, i: usize) -> Result<(), String> {
        let c = self.writable(i)?;
        if c.profile.is_some() {
            return Err("groups use your main identity; this contact knows another".into());
        }
        // 0: nothing heard yet, as from someone whose request we just took;
        // their level comes with their first answer.
        let unheard = c.understands == 0;
        if !unheard && c.understands < GROUPS_LEVEL {
            return Err("their app is older; groups come after they update".into());
        }
        let key = c.key;
        let group = &self.data.groups[g];
        if !group.is_admin() {
            return Err("only the group's admin adds members".into());
        }
        if group.moving.is_some() {
            return Err("the group is moving to a new queue; try again in a moment".into());
        }
        if group.members().iter().any(|m| m.ed25519 == key) || group.asking.contains(&key) {
            return Err("already in the group or invited".into());
        }
        if group.members().len() + group.invited.len() + group.asking.len()
            >= proto::MAX_GROUP_MEMBERS
        {
            return Err("a group has at most 32 members".into());
        }
        if unheard {
            // shortcut: an app too old to say its level at all keeps this waiting
            // for good; such apps predate 0.9 and must update anyway.
            if !self.data.groups[g].awaiting.contains(&key) {
                self.data.groups[g].awaiting.push(key);
            }
            self.persist();
            return Ok(());
        }
        self.data.groups[g].awaiting.retain(|k| *k != key);
        // ADR-028: a reader for each of their devices, primary first.
        let welcome =
            self.devices_welcome(g) && self.data.contacts[i].understands >= proto::UNDERSTANDS;
        let devices: Vec<Ed25519Key> = self.data.contacts[i]
            .their_devices
            .iter()
            .filter(|_| welcome)
            .flat_map(|d| d.devices.iter().map(|x| x.ed25519))
            .filter(|k| *k != key)
            .collect();
        self.data.groups[g].asking.push(key);
        self.data.groups[g].asking.extend(&devices);
        self.persist();
        if let Some((group_id, token)) = self.data.groups[g].admin {
            for _ in 0..=devices.len() {
                self.group_send(g, ClientFrame::AddReader { group_id, token });
            }
        }
        Ok(())
    }

    /// Drops invite `n` unanswered; the admin is not told.
    /// Contact `i` said what their app reads: invites waiting for that go.
    pub(super) fn invite_awaiting(&mut self, i: usize) {
        let key = self.data.contacts[i].key;
        for g in 0..self.data.groups.len() {
            if self.data.groups[g].awaiting.contains(&key) {
                self.data.groups[g].awaiting.retain(|k| *k != key);
                if let Err(e) = self.invite_to_group(g, i) {
                    self.note(e);
                }
            }
        }
    }

    pub fn decline_group(&mut self, n: usize) {
        if n < self.data.group_invites.len() {
            self.data.group_invites.remove(n);
            self.persist();
        }
    }

    /// Takes invite `n`: we join the group, share our key, tell the admin.
    pub fn accept_group(&mut self, n: usize) -> Result<usize, String> {
        self.take_group_invite(n, true)
    }

    /// Takes invite `n`. `announce`: our person chose it here, so the admin
    /// hears it and our other devices take theirs (ADR-028); a device that
    /// follows another of ours says nothing.
    pub(super) fn take_group_invite(&mut self, n: usize, announce: bool) -> Result<usize, String> {
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
        // Invited back after leaving: the old read-only copy goes, one entry per group.
        self.data.groups.retain(|g| g.id != id);
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
        self.group_system(
            g,
            "you joined: messages from before are not shared with new members".into(),
        );
        self.know_members(g);
        if announce {
            if let Err(e) = self.transmit(&inv.from, Body::GroupJoin { group: id }) {
                self.note(e);
            }
            self.tell_own(Body::OwnGroupJoin { group: id });
        }
        self.data.pending_group_joins.retain(|x| *x != id);
        self.persist();
        self.sync_links()?;
        self.share_key(g);
        Ok(g)
    }

    /// Writes `text` to group `g`.
    pub fn send_group(&mut self, g: usize, text: &str) -> Result<(), String> {
        self.send_group_with(g, text, None)
    }

    fn send_group_with(
        &mut self,
        g: usize,
        text: &str,
        reply_to: Option<Ref>,
    ) -> Result<(), String> {
        if text.len() > proto::MAX_TEXT_BYTES {
            return Err("message too long".into());
        }
        let group = self.data.groups.get(g).ok_or("no such group")?;
        if group.left {
            return Err("you are not in this group any more".into());
        }
        let id = getrandom::u64().map_err(|e| e.to_string())?;
        let at = now();
        let expires_in = group.timer();
        self.data.groups[g].messages.push(Msg {
            mine: true,
            text: text.to_string(),
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Queued,
            file: None,
            id: Some(id),
            reply_to,
            ..Default::default()
        });
        self.persist();
        self.flush_group(g);
        Ok(())
    }

    /// `text` in group `g` as an answer to message `m`.
    pub fn reply_group(&mut self, g: usize, m: usize, text: &str) -> Result<(), String> {
        let target = self.group_target(g, m)?;
        self.send_group_with(g, text, Some(target))
    }

    /// Message `m` of group `g` as something to point at.
    fn group_target(&self, g: usize, m: usize) -> Result<Ref, String> {
        let group = self.data.groups.get(g).ok_or("no such group")?;
        if group.left {
            return Err("you are not in this group any more".into());
        }
        let msg = group.messages.get(m).ok_or("no such message")?;
        match msg.id {
            Some(id) if !msg.deleted && msg.status != Status::System => Ok(Ref {
                id,
                theirs: !msg.mine,
            }),
            _ => Err("this message cannot be pointed at".into()),
        }
    }

    /// New text for our message `m` of group `g`.
    pub fn edit_group_message(&mut self, g: usize, m: usize, text: &str) -> Result<(), String> {
        if text.trim().is_empty() || text.len() > proto::MAX_TEXT_BYTES {
            return Err("an edit is 1 to 16 000 bytes".into());
        }
        let target = self.group_target(g, m)?;
        let msg = &mut self.data.groups[g].messages[m];
        if !msg.mine {
            return Err("only your own messages can be changed".into());
        }
        if msg.status != Status::Queued {
            self.data.groups[g].outbox.push(Body::Edit {
                id: target.id,
                text: text.to_string(),
            });
        }
        let msg = &mut self.data.groups[g].messages[m];
        msg.edited = msg.status != Status::Queued;
        msg.replace_text(text.to_string());
        self.persist();
        self.flush_group(g);
        Ok(())
    }

    /// Deletes our message `m` of group `g` for everyone.
    pub fn delete_group_message(&mut self, g: usize, m: usize) -> Result<(), String> {
        let target = self.group_target(g, m)?;
        let messages = &mut self.data.groups[g].messages;
        if !messages[m].mine {
            return Err("only your own messages can be changed".into());
        }
        match messages[m].status {
            Status::Queued => drop(messages.remove(m)),
            _ => {
                messages[m].wipe();
                let group = &mut self.data.groups[g];
                group.reactions.retain(|(id, _, _)| *id != target.id);
                group.outbox.push(Body::Delete { id: target.id });
            }
        }
        self.persist();
        self.flush_group(g);
        Ok(())
    }

    /// Our reaction to message `m` of group `g`, or none.
    pub fn react_group(&mut self, g: usize, m: usize, reaction: Option<u8>) -> Result<(), String> {
        if reaction.is_some_and(|r| r >= proto::REACTIONS) {
            return Err("no such reaction".into());
        }
        let target = self.group_target(g, m)?;
        let me = self.me();
        self.data.groups[g].set_reaction(target.id, me, reaction);
        self.data.groups[g]
            .outbox
            .push(Body::React { target, reaction });
        self.persist();
        self.flush_group(g);
        Ok(())
    }

    /// Sends what waits in group `g`, when its link is up.
    pub(super) fn flush_group(&mut self, g: usize) {
        let gid = self.data.groups[g].id;
        if self.data.groups[g].left {
            // Only the goodbye is left to send; then the keys go.
            while self.group_link_up(g)
                && let Some(body) = self.data.groups[g].outbox.first().cloned()
            {
                self.send_group_body(g, body);
                self.data.groups[g].outbox.remove(0);
            }
            if self.data.groups[g].outbox.is_empty() {
                self.core().group_forget(&gid);
            }
            self.persist();
            return;
        }
        if self.core().group_due(&gid, now()) {
            self.rotate_group(g);
        }
        let expires_in = self.data.groups[g].timer();
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
        // Edits, deletes and reactions, in the order they were made.
        while self.group_link_up(g)
            && let Some(body) = self.data.groups[g].outbox.first().cloned()
        {
            self.send_group_body(g, body);
            self.data.groups[g].outbox.remove(0);
        }
        self.persist();
    }

    fn group_link_up(&self, g: usize) -> bool {
        let group = &self.data.groups[g];
        let link = Link {
            who: link_who(&group.id),
            relay: group.relay_url.clone(),
        };
        self.links.get(&link).is_some_and(|l| l.connected) && group.send_id != [0; 16]
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
        group.since = group.devices().iter().map(|m| (m.ed25519, 0)).collect();
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
        let members: Vec<RosterMember> = self.data.groups[g].devices();
        for m in members {
            if m.ed25519 == me || self.data.groups[g].shared.contains(&m.ed25519) {
                continue;
            }
            let contact = self.index(&m.ed25519).filter(|&i| {
                self.data.contacts[i].profile.is_none() && self.has_session(&m.ed25519)
            });
            // ADR-028: a contact's linked device, over the chat with them.
            let owner = self
                .owner_of_device(&m.ed25519)
                .map(|c| self.data.contacts[c].key);
            if contact.is_some() || owner.is_some() {
                let Some(key) = self.key_for(g, &m.ed25519) else {
                    return;
                };
                let body = Body::GroupKey {
                    group: gid,
                    key: SecretBytes(key.to_vec()),
                };
                match owner.filter(|_| contact.is_none()) {
                    Some(owner) => {
                        self.transmit_device(&owner, m.ed25519, body);
                        self.data.groups[g].shared.push(m.ed25519);
                    }
                    None => {
                        if self.transmit(&m.ed25519, body).is_ok() {
                            self.data.groups[g].shared.push(m.ed25519);
                        }
                    }
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
        let members: Vec<RosterMember> = self.data.groups[g].devices();
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
        if self.data.groups[g].left {
            return self.flush_group(g);
        }
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
        self.subscribe_group(g);
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
                match group.moving.as_mut() {
                    // The new queue of a move; the old one goes once all moved.
                    Some(m) if m.old.is_none() => m.old = group.admin.take(),
                    _ if group.admin.is_some() => return,
                    _ => {}
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
                let readers = &mut self.data.groups[g].readers;
                readers.retain(|(k, _)| *k != who);
                readers.push((who, recv_id));
                if let Some(m) = self.data.groups[g].moving.as_mut() {
                    m.grants.push((who, grant));
                    if self.data.groups[g].asking.is_empty() {
                        self.finish_move(g);
                    }
                    return self.persist_unit();
                }
                if who == self.me() {
                    self.data.groups[g].reader = Some(grant.clone());
                    self.add_to_roster(g, who, grant.send_id);
                    self.subscribe_group(g);
                    self.persist();
                    self.flush_group(g);
                    // Our own linked devices join our new group (ADR-028).
                    self.group_devices_changed(who);
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
                    // A linked device of a contact gets its invite alone (ADR-028).
                    match self.index(&who) {
                        Some(o) => {
                            // Our own device: we are in the group, so it joins.
                            if self.data.contacts[o].own {
                                let gid = self.data.groups[g].id;
                                self.data.contacts[o]
                                    .outbox
                                    .push(Body::OwnGroupJoin { group: gid });
                                self.deliver(o);
                            }
                            if let Err(e) = self.transmit(&who, body) {
                                self.note(e);
                            }
                        }
                        None => {
                            if let Some(c) = self.owner_of_device(&who) {
                                let key = self.data.contacts[c].key;
                                self.transmit_device(&key, who, body);
                            }
                        }
                    }
                    // A device of someone in the group already: everyone hears.
                    let owner = match self.owner_of_device(&who) {
                        Some(c) => Some(self.data.contacts[c].key),
                        None => self.lists(&who).then(|| self.identity_key()),
                    };
                    if let Some(owner) = owner.filter(|o| {
                        self.data.groups[g]
                            .members()
                            .iter()
                            .any(|m| m.ed25519 == *o)
                    }) {
                        self.publish_devices(g, owner);
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
                    .devices()
                    .into_iter()
                    .find(|m| m.reader == send_id)
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
        if self.data.groups[g].owner(&sender).is_none() {
            // A newcomer's key before the roster that names them: keep it
            // until then (`take_roster`), the latest one per sender.
            let pending = &mut self.data.groups[g].pending_keys;
            pending.retain(|(k, _)| *k != sender);
            if pending.len() < MAX_PENDING_KEYS {
                pending.push((sender, key));
                self.persist();
            }
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
    fn group_body(&mut self, g: usize, device: Ed25519Key, body: Body) {
        if device == self.me() {
            return;
        }
        // ADR-028: a device speaks for its member; ours for us.
        let Some(sender) = self.data.groups[g].owner(&device) else {
            return;
        };
        let mine = sender == self.identity_key();
        // The admin's roster and device lists still count from our primary.
        let change = matches!(
            body,
            Body::Edit { .. } | Body::Delete { .. } | Body::React { .. } | Body::GroupLeave { .. }
        );
        if mine && change {
            // shortcut: edits, deletes and reactions from our other devices
            // are not mirrored in groups; mirror them if people miss them.
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
                    mine,
                    text,
                    at,
                    expires_at: expires_in.map(|s| at + u64::from(s)),
                    status: if mine { Status::Sent } else { Status::Received },
                    file: None,
                    author: (!mine).then_some(sender),
                    id: Some(id),
                    reply_to,
                    unfetched: !mine,
                    ..Default::default()
                });
                if mine {
                    return;
                }
                if self.open_group != Some(group.id) {
                    group.unread += 1;
                }
                let id = group.id;
                self.happened.push(Happened::GroupAlert { group: id });
            }
            Body::GroupRoster { roster } => self.take_roster(g, sender, roster),
            Body::GroupDevices {
                member,
                list,
                readers,
                ..
            } => self.take_member_devices(g, sender, member, list, readers),
            // Edits and deletes reach only the sender's own messages.
            Body::Edit { id, text } => {
                if let Some(m) = self.data.groups[g]
                    .messages
                    .iter_mut()
                    .find(|m| m.id == Some(id) && m.author == Some(sender) && !m.deleted)
                {
                    m.replace_text(text);
                    m.edited = true;
                }
            }
            Body::Delete { id } => {
                if let Some(m) = self.data.groups[g]
                    .messages
                    .iter_mut()
                    .find(|m| m.id == Some(id) && m.author == Some(sender) && !m.deleted)
                {
                    m.wipe();
                    self.data.groups[g].reactions.retain(|(x, _, _)| *x != id);
                }
            }
            Body::React { target, reaction } => {
                if reaction.is_some_and(|r| r >= proto::REACTIONS) {
                    return;
                }
                let group = &mut self.data.groups[g];
                if group
                    .messages
                    .iter()
                    .any(|m| m.id == Some(target.id) && !m.deleted)
                {
                    group.set_reaction(target.id, sender, reaction);
                }
            }
            Body::GroupLeave { .. } => {
                let name = self.member_name(g, &sender);
                self.group_system(g, format!("{name} left the group"));
                if self.data.groups[g].is_admin() {
                    let _ = self.remove_from_group(g, &sender);
                }
            }
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
        let me = self.me();
        self.data.groups[g].set_roster(roster, &me);
        self.know_members(g);
        self.replay_pending_keys(g);
        self.share_key(g);
    }

    /// Keys that came before the roster or device list that names their
    /// sender; those still unnamed wait again.
    fn replay_pending_keys(&mut self, g: usize) {
        let gid = self.data.groups[g].id;
        let pending = std::mem::take(&mut self.data.groups[g].pending_keys);
        for (sender, key) in pending {
            self.group_key_from(sender, Body::GroupKey { group: gid, key });
        }
    }

    fn persist_unit(&mut self) {
        self.persist();
    }

    /// The admin takes `who` out of group `g`: the group moves to a new
    /// queue without them, everyone gets a new reader and a new roster, and
    /// every member starts a new session (ADR-027).
    pub fn remove_from_group(&mut self, g: usize, who: &Ed25519Key) -> Result<(), String> {
        let me = self.me();
        let group = self.data.groups.get(g).ok_or("no such group")?;
        if !group.is_admin() {
            return Err("only the group's admin removes members".into());
        }
        if *who == me {
            return Err("the admin cannot remove themselves; leave the group instead".into());
        }
        if !group.members().iter().any(|m| m.ed25519 == *who) {
            return Err("not in this group".into());
        }
        // Their linked devices go with them (ADR-028).
        let mut gone = vec![*who];
        for (m, list, _) in &group.member_devices {
            if m == who {
                gone.extend(list.devices.iter().map(|d| d.ed25519).filter(|k| k != who));
            }
        }
        self.data.groups[g]
            .member_devices
            .retain(|(m, ..)| m != who);
        self.start_move(g, gone);
        let name = self.data.groups[g]
            .name_of(who)
            .unwrap_or("someone")
            .to_string();
        self.group_system(g, format!("you removed {name}"));
        self.persist();
        Ok(())
    }

    /// Admin: the group moves to a new queue without `gone`, members or
    /// devices; every device that stays gets a reader of the new one.
    fn start_move(&mut self, g: usize, gone: Vec<Ed25519Key>) {
        // shortcut: a reader per device and one queue, all at once; past the
        // relay's queue budget (20 an hour per IP) the move waits stuck, as
        // a large member removal already did. Retry refused frames if seen.
        let stay: Vec<Ed25519Key> = self.data.groups[g]
            .devices()
            .iter()
            .map(|m| m.ed25519)
            .filter(|k| !gone.contains(k))
            .collect();
        let group = &mut self.data.groups[g];
        match group.moving.as_mut() {
            // Removed while moving: out of the move too.
            Some(m) => {
                m.removed.extend(&gone);
                m.stay.retain(|k| !gone.contains(k));
                group.asking.retain(|k| !gone.contains(k));
            }
            None => {
                // Invites not answered yet point at the old queue: they are
                // made again for the new one.
                let reinvite: Vec<Ed25519Key> = group.invited.drain(..).map(|(k, _)| k).collect();
                let mut asking = stay.clone();
                asking.extend(&reinvite);
                group.moving = Some(GroupMoving {
                    stay,
                    removed: gone,
                    grants: Vec::new(),
                    old: None,
                    reinvite,
                });
                group.asking = asking;
                self.group_send(g, ClientFrame::CreateGroup);
            }
        }
    }

    /// `body` to `who`: a contact, one of our devices, or a contact's device.
    fn group_tell(&mut self, who: &Ed25519Key, body: Body) {
        if self.index(who).is_some() {
            if let Err(e) = self.transmit(who, body) {
                self.note(e);
            }
        } else if let Some(c) = self.owner_of_device(who) {
            let key = self.data.contacts[c].key;
            self.transmit_device(&key, *who, body);
        }
    }

    /// Every remaining member has a reader of the new queue: tell them, tell
    /// the removed, delete the old queue, start a new session.
    fn finish_move(&mut self, g: usize) {
        let me = self.me();
        let Some(moving) = self.data.groups[g].moving.take() else {
            return;
        };
        let Some(old_roster) = self.data.groups[g].roster.clone() else {
            return;
        };
        let reader_of = |k: &Ed25519Key| {
            moving
                .grants
                .iter()
                .find(|(x, _)| x == k)
                .map(|(_, r)| r.clone())
        };
        let mut roster = old_roster.clone();
        roster.members = old_roster
            .members
            .iter()
            .filter(|m| !moving.removed.contains(&m.ed25519))
            .filter_map(|m| {
                let r = reader_of(&m.ed25519)?;
                Some(RosterMember {
                    reader: r.send_id,
                    ..m.clone()
                })
            })
            .collect();
        roster.no += 1;
        roster.signature = self.core().sign_roster(&roster.signed_bytes());
        let group = &mut self.data.groups[g];
        group.set_roster(roster.clone(), &me);
        if let Some(mine) = reader_of(&me) {
            group.reader = Some(mine);
        }
        // Linked devices' readers of the new queue (ADR-028).
        for (_, list, readers) in &mut group.member_devices {
            *readers = list
                .devices
                .iter()
                .filter_map(|d| reader_of(&d.ed25519).map(|r| (d.ed25519, r.send_id)))
                .filter(|(k, _)| *k != list.identity)
                .collect();
        }
        let send_id = group.send_id;
        self.persist();
        for (who, grant) in &moving.grants {
            if *who == me || moving.removed.contains(who) || moving.reinvite.contains(who) {
                continue;
            }
            let body = Body::GroupMove {
                roster: roster.clone(),
                send_id,
                reader: grant.clone(),
            };
            self.group_tell(who, body);
        }
        for who in &moving.reinvite {
            let Some(grant) = reader_of(who) else {
                continue;
            };
            self.data.groups[g].invited.push((*who, grant.clone()));
            let group = &self.data.groups[g];
            let body = Body::GroupInvite {
                roster: roster.clone(),
                relay_url: group.relay_url.clone(),
                onion_url: group.onion_url.clone(),
                send_id,
                reader: grant,
            };
            self.group_tell(who, body);
        }
        let gid = self.data.groups[g].id;
        for who in &moving.removed {
            self.group_tell(who, Body::GroupLeave { group: gid });
        }
        if let Some((group_id, token)) = moving.old {
            self.group_send(g, ClientFrame::DeleteGroup { group_id, token });
        }
        self.subscribe_group(g);
        self.after_members_left(g, &moving.removed);
        let members: Vec<Ed25519Key> = self.data.groups[g]
            .members()
            .iter()
            .map(|m| m.ed25519)
            .collect();
        for who in members {
            self.group_devices_changed(who);
        }
        self.resend_devices(g);
    }

    /// Admin: every member's device list again, for those who joined or
    /// moved since it was sent.
    fn resend_devices(&mut self, g: usize) {
        let gid = self.data.groups[g].id;
        let entries = self.data.groups[g].member_devices.clone();
        for (member, list, readers) in entries {
            self.send_group_body(
                g,
                Body::GroupDevices {
                    group: gid,
                    member,
                    list,
                    readers,
                },
            );
        }
    }

    /// Members gone from the roster: their sessions are forgotten and ours
    /// starts again, so they read nothing more.
    fn after_members_left(&mut self, g: usize, gone: &[Ed25519Key]) {
        let gid = self.data.groups[g].id;
        {
            let mut core = self.core();
            for who in gone {
                core.group_forget_member(&gid, who);
            }
        }
        let group = &mut self.data.groups[g];
        group.since.retain(|(k, _)| !gone.contains(k));
        group.shared.retain(|k| !gone.contains(k));
        group.readers.retain(|(k, _)| !gone.contains(k));
        self.rotate_group(g);
        self.persist();
    }

    /// What a member is called in group `g`: their roster nickname.
    fn member_name(&self, g: usize, who: &Ed25519Key) -> String {
        self.data.groups[g]
            .members()
            .iter()
            .find(|m| m.ed25519 == *who)
            .map_or_else(|| "someone".to_string(), |m| clean(&m.nickname))
    }

    fn group_system(&mut self, g: usize, text: String) {
        self.data.groups[g].messages.push(Msg {
            mine: false,
            text,
            at: now(),
            expires_at: None,
            status: Status::System,
            file: None,
            ..Default::default()
        });
    }

    /// The admin sets group `g`'s disappearing timer: a new roster for all.
    pub fn set_group_timer(&mut self, g: usize, expires_in: Option<u32>) -> Result<(), String> {
        let group = self.data.groups.get(g).ok_or("no such group")?;
        if !group.is_admin() {
            return Err("only the group's admin sets the timer".into());
        }
        let Some(mut roster) = group.roster.clone() else {
            return Err("the group is not ready yet".into());
        };
        roster.expires_in = expires_in;
        roster.no += 1;
        roster.signature = self.core().sign_roster(&roster.signed_bytes());
        let me = self.me();
        self.data.groups[g].set_roster(roster.clone(), &me);
        self.persist();
        if roster.members.len() > 1 {
            self.send_group_body(g, Body::GroupRoster { roster });
        }
        Ok(())
    }

    /// We leave group `g`: the others are told, and it becomes read-only here.
    pub fn leave_group(&mut self, g: usize) -> Result<(), String> {
        let group = self.data.groups.get(g).ok_or("no such group")?;
        if group.left {
            return Ok(());
        }
        let gid = group.id;
        self.send_group_body(g, Body::GroupLeave { group: gid });
        self.data.groups[g].left = true;
        // The keys go once the goodbye is out (`flush_group`).
        self.flush_group(g);
        Ok(())
    }

    /// The admin puts `who` on the roster, signs it and tells the group.
    fn add_to_roster(&mut self, g: usize, who: Ed25519Key, reader: QueueId) {
        // The name they chose, as every member's app shows names: not our
        // own petname for them.
        let theirs = self.core().nickname(&who).map(clean);
        let nickname = theirs.unwrap_or_else(|| self.data.nickname.clone());
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
        let new = !roster.members.iter().any(|m| m.ed25519 == who);
        roster.members.retain(|m| m.ed25519 != who);
        roster.members.push(RosterMember {
            ed25519: who,
            curve25519,
            nickname: nickname.clone(),
            reader,
        });
        roster.no += 1;
        if new && who != me {
            self.group_system(g, format!("{nickname} joined the group"));
        }
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
    /// Sends `body` to the group now, or keeps it in the group's outbox
    /// until the link is up: right after a start it is not yet.
    /// Admin: member `who`'s device list and the readers made for their
    /// linked devices, to everyone in the group (ADR-028).
    fn publish_devices(&mut self, g: usize, who: Ed25519Key) {
        let Some(list) = self.device_list_of(&who) else {
            return;
        };
        let group = &mut self.data.groups[g];
        let listed = |k: &Ed25519Key| list.devices.iter().any(|d| d.ed25519 == *k && *k != who);
        let had = group.member_devices.iter().find(|(m, ..)| *m == who);
        let mut readers: Vec<(Ed25519Key, QueueId)> = had
            .map(|(_, _, r)| r.iter().filter(|(k, _)| listed(k)).copied().collect())
            .unwrap_or_default();
        for (k, grant) in group.invited.iter().filter(|(k, _)| listed(k)) {
            readers.retain(|(x, _)| x != k);
            readers.push((*k, grant.send_id));
        }
        group.invited.retain(|(k, _)| !listed(k));
        group.member_devices.retain(|(m, ..)| *m != who);
        group
            .member_devices
            .push((who, list.clone(), readers.clone()));
        let gid = group.id;
        self.know_members(g);
        // The roster again: the new devices read only from here on.
        if let Some(roster) = self.data.groups[g].roster.clone() {
            self.send_group_body(g, Body::GroupRoster { roster });
        }
        self.send_group_body(
            g,
            Body::GroupDevices {
                group: gid,
                member: who,
                list,
                readers,
            },
        );
        self.share_key(g);
        self.persist();
    }

    /// The signed device list of `who`: ours, or a contact's.
    fn device_list_of(&self, who: &Ed25519Key) -> Option<proto::DeviceList> {
        if *who == self.identity_key() {
            return self.data.devices.clone();
        }
        self.index(who)
            .and_then(|i| self.data.contacts[i].their_devices.clone())
    }

    /// Whether linked devices may take part in group `g`: every member we
    /// know of understands devices (ADR-028, amendment point 8).
    fn devices_welcome(&self, g: usize) -> bool {
        let me = self.identity_key();
        self.data.groups[g].members().iter().all(|m| {
            m.ed25519 == me
                || self
                    .index(&m.ed25519)
                    .is_some_and(|i| self.data.contacts[i].understands >= proto::UNDERSTANDS)
        })
    }

    /// Admin: `who`'s device list changed. New devices get readers and
    /// invites; devices gone from it make the group move without them.
    pub(super) fn group_devices_changed(&mut self, who: Ed25519Key) {
        let Some(list) = self.device_list_of(&who) else {
            return;
        };
        for g in 0..self.data.groups.len() {
            let group = &self.data.groups[g];
            if !group.is_admin() || group.left || !group.members().iter().any(|m| m.ed25519 == who)
            {
                continue;
            }
            // A move under way: new devices wait for it (`finish_move` asks again).
            if !self.devices_welcome(g) || group.moving.is_some() {
                continue;
            }
            let listed = |k: &Ed25519Key| list.devices.iter().any(|d| d.ed25519 == *k);
            let had: Vec<Ed25519Key> = group
                .member_devices
                .iter()
                .filter(|(m, ..)| *m == who)
                .flat_map(|(_, _, r)| r.iter().map(|(k, _)| *k))
                .collect();
            let gone: Vec<Ed25519Key> = had.iter().filter(|k| !listed(k)).copied().collect();
            let new: Vec<Ed25519Key> = list
                .devices
                .iter()
                .map(|d| d.ed25519)
                .filter(|k| {
                    *k != who
                        && *k != self.me()
                        && !had.contains(k)
                        && !group.asking.contains(k)
                        && !group.invited.iter().any(|(x, _)| x == k)
                })
                .collect();
            let group = &mut self.data.groups[g];
            if let Some(entry) = group.member_devices.iter_mut().find(|(m, ..)| *m == who) {
                entry.1 = list.clone();
                entry.2.retain(|(k, _)| listed(k));
            }
            if !gone.is_empty() {
                self.start_move(g, gone);
            }
            if new.is_empty() {
                continue;
            }
            let group = &mut self.data.groups[g];
            group.asking.extend(&new);
            if let (Some((group_id, token)), None) = (group.admin, group.moving.as_ref()) {
                for _ in &new {
                    self.group_send(g, ClientFrame::AddReader { group_id, token });
                }
            }
        }
        self.persist();
    }

    /// `GroupDevices` from the admin: kept once the member signed the list.
    fn take_member_devices(
        &mut self,
        g: usize,
        sender: Ed25519Key,
        member: Ed25519Key,
        list: proto::DeviceList,
        readers: Vec<(Ed25519Key, QueueId)>,
    ) {
        let group = &self.data.groups[g];
        let admin = group.roster.as_ref().map(|r| r.admin);
        let on_roster = group.members().iter().any(|m| m.ed25519 == member);
        if Some(sender) != admin
            || !on_roster
            || list.identity != member
            || termchat_core::verify_devices(&list).is_err()
        {
            return;
        }
        let group = &mut self.data.groups[g];
        let gone: Vec<Ed25519Key> = group
            .member_devices
            .iter()
            .filter(|(m, ..)| *m == member)
            .flat_map(|(_, _, r)| r.iter().map(|(k, _)| *k))
            .filter(|k| !readers.iter().any(|(x, _)| x == k))
            .collect();
        group.member_devices.retain(|(m, ..)| *m != member);
        group.member_devices.push((member, list, readers));
        self.know_members(g);
        self.replay_pending_keys(g);
        if gone.is_empty() {
            self.share_key(g);
        } else {
            self.after_members_left(g, &gone);
        }
        self.persist();
    }

    fn send_group_body(&mut self, g: usize, body: Body) {
        if !self.group_link_up(g) {
            self.data.groups[g].outbox.push(body);
            self.persist();
            return;
        }
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
                // We joined already, through an invite to a queue the group
                // has since left: this one moves us, as a `GroupMove` would.
                if self
                    .data
                    .groups
                    .iter()
                    .any(|x| x.id == roster.group && !x.left)
                {
                    let body = Body::GroupMove {
                        roster,
                        send_id,
                        reader,
                    };
                    return self.group_from_contact(i, body);
                }
                let name = roster.name.clone();
                self.data
                    .group_invites
                    .retain(|x| x.roster.group != roster.group);
                let group = roster.group;
                self.data.group_invites.push(GroupInvite {
                    from,
                    roster,
                    relay_url,
                    onion_url,
                    send_id,
                    reader,
                });
                // Another device of ours took this group already (ADR-028).
                if self.data.pending_group_joins.contains(&group) {
                    let n = self.data.group_invites.len() - 1;
                    if let Err(e) = self.take_group_invite(n, false) {
                        self.note(e);
                    }
                    return self.persist_unit();
                }
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
                self.resend_devices(g);
                self.publish_devices(g, from);
            }
            body @ Body::GroupKey { .. } => self.group_key_from(from, body),
            Body::GroupMove {
                roster,
                send_id,
                reader,
            } => {
                let Some(g) = self
                    .data
                    .groups
                    .iter()
                    .position(|x| x.id == roster.group && !x.left)
                else {
                    return;
                };
                let current = self.data.groups[g].roster.clone();
                let ok = current
                    .as_ref()
                    .is_some_and(|r| r.admin == from && roster.admin == from && roster.no > r.no)
                    && verify_roster(&roster).is_ok();
                if !ok {
                    return;
                }
                let gone: Vec<Ed25519Key> = current
                    .map(|r| r.members)
                    .unwrap_or_default()
                    .iter()
                    .map(|m| m.ed25519)
                    .filter(|k| !roster.members.iter().any(|m| m.ed25519 == *k))
                    .collect();
                let me = self.me();
                let group = &mut self.data.groups[g];
                group.set_roster(roster, &me);
                group.send_id = send_id;
                group.reader = Some(reader.clone());
                self.subscribe_group(g);
                self.after_members_left(g, &gone);
            }
            // From the admin, over our chat: we were removed.
            Body::GroupLeave { group } => {
                let Some(g) = self
                    .data
                    .groups
                    .iter()
                    .position(|x| x.id == group && !x.left)
                else {
                    return;
                };
                if self.data.groups[g]
                    .roster
                    .as_ref()
                    .is_none_or(|r| r.admin != from)
                {
                    return;
                }
                self.group_system(g, "the admin removed you from the group".to_string());
                self.data.groups[g].left = true;
                self.core().group_forget(&group);
            }
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
        expires_in: None,
        signature: [0; 64],
    }
}

fn verify_roster(r: &Roster) -> Result<(), termchat_core::Error> {
    termchat_core::verify_roster(&r.admin, &r.signed_bytes(), &r.signature)
}
