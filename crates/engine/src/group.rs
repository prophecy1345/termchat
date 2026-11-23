//! Small private groups (ADR-027): the group queue and its readers on the
//! admin's relay, Megolm sessions shared over Olm, a roster the admin signs.
//! A child of `engine`, for its private state.

use super::*;
use crate::data::{FileMsg, Group, GroupInvite, GroupMoving};
use std::sync::atomic::Ordering;
use termchat_core::check_admin_change;
use termchat_core::proto::{
    AdminList, Envelope, GroupId, OLM_TYPE_MEGOLM, ReaderGrant, Ref, Roster, RosterMember,
    SecretBytes,
};
use zeroize::Zeroizing;

/// Group messages kept while their sender's key is on its way.
const MAX_EARLY: usize = 50;
/// Keys from members a roster has not named yet; a group has at most 32.
const MAX_PENDING_KEYS: usize = 32;
/// The `Understands` level that knows groups (ADR-027).
const GROUPS_LEVEL: u16 = 2;
const MOVE_RETRY: Duration = Duration::from_secs(30);
/// ADR-042, amendment 3: the longest an old queue outlives a move, seconds.
const RETIRE_AFTER: u64 = 24 * 3600;

const GROUP_ACCESS_LOST: &str = "this device is not in the group";
const GROUP_SAVE_FAILED: &str = "could not save the group message before sending";
const GROUP_ENCRYPT_FAILED: &str = "could not encrypt the group message";

#[derive(Debug)]
pub enum GroupScriptError {
    State(String),
    Refused(String),
}

impl GroupScriptError {
    fn message(self) -> String {
        match self {
            Self::State(why) | Self::Refused(why) => why,
        }
    }
}

/// The link that carries a group's queue: its own, like a contact's.
pub(super) fn link_who(id: &GroupId) -> Ed25519Key {
    let mut who = [0xff; 32];
    who[..16].copy_from_slice(id);
    who
}

impl Engine {
    pub fn group_index(&self, id: &GroupId) -> Option<usize> {
        self.data.groups.iter().position(|g| g.id == *id)
    }

    /// A locally accepted reader grant and membership of this identity.
    pub fn group_access(&self, g: usize) -> bool {
        let Some(group) = self.data.groups.get(g) else {
            return false;
        };
        self.group_locally_joined(g)
            && (self.is_primary() || self.lists(&self.me()))
            && group.reader.is_some()
            && group
                .members()
                .iter()
                .any(|m| m.ed25519 == self.identity_key())
    }

    fn group_locally_joined(&self, g: usize) -> bool {
        !self.gone && !self.removed() && !self.data.groups[g].left
    }

    fn group_moving(&self, g: usize) -> bool {
        let group = &self.data.groups[g];
        group.moving.is_some() || group.waiting_for_move
    }

    pub(super) fn stop_group_writes(&mut self, g: usize, leaving: bool) {
        let group = &mut self.data.groups[g];
        if let Some(session) = self.group_writes.remove(&group.id) {
            session.store(false, Ordering::Release);
        }
        for m in &mut group.messages {
            if matches!(m.status, Status::Sending(_)) || (leaving && m.status == Status::Queued) {
                m.status = Status::Failed(
                    if leaving {
                        GROUP_ACCESS_LOST
                    } else {
                        "the group moved before confirmation"
                    }
                    .into(),
                );
            }
        }
        if leaving {
            group.outbox.clear();
        }
    }

    fn wait_for_group_move(&mut self, g: usize) {
        self.stop_group_writes(g, false);
        self.data.groups[g].waiting_for_move = true;
    }

    fn retry_group_move_later(&mut self, g: usize) {
        self.group_move_retries
            .entry(self.data.groups[g].id)
            .or_insert_with(|| Instant::now() + MOVE_RETRY);
    }

    pub(super) fn retry_group_moves(&mut self) {
        let now = Instant::now();
        let mut due = Vec::new();
        self.group_move_retries.retain(|id, at| {
            if *at > now {
                return true;
            }
            due.push(*id);
            false
        });
        for id in due {
            if let Some(g) = self.group_index(&id)
                && self.group_locally_joined(g)
                && self.data.groups[g].moving.is_some()
            {
                self.group_connected(g);
            }
        }
    }

    pub(super) fn group_of_link(&self, link: &Link) -> Option<usize> {
        self.data
            .groups
            .iter()
            .position(|g| link_who(&g.id) == link.who && same_url(&g.relay_url, &link.relay))
    }

    /// The links groups need, for `start_links`.
    pub(super) fn group_links(&self) -> Vec<(Link, Option<String>)> {
        if self.removed() {
            return Vec::new();
        }
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
        self.group_send_scoped(g, frame, None)
    }

    fn group_send_scoped(
        &self,
        g: usize,
        frame: ClientFrame,
        session: Option<Arc<AtomicBool>>,
    ) -> bool {
        let group = &self.data.groups[g];
        let link = Link {
            who: link_who(&group.id),
            relay: group.relay_url.clone(),
        };
        match self.links.get(&link) {
            Some(l) if l.connected => l.relay.send_scoped(frame, session).is_ok(),
            _ => false,
        }
    }

    fn group_send_ciphertext(&mut self, g: usize, frame: ClientFrame) -> bool {
        let id = self.data.groups[g].id;
        let session = self
            .group_writes
            .entry(id)
            .or_insert_with(|| Arc::new(AtomicBool::new(true)))
            .clone();
        self.group_send_scoped(g, frame, Some(session))
    }

    /// Reads our reader of group `g`, and asks the relay to wake us for it
    /// when a push endpoint is set (ADR-012): a reader is a queue like any.
    fn subscribe_group(&mut self, g: usize) {
        let group = &self.data.groups[g];
        let Some(r) = group.reader.clone() else {
            return;
        };
        // Until the relay's key count says the backlog is out.
        self.group_read.remove(&group.id);
        let group = &self.data.groups[g];
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
            if self.data.hurry_open {
                l.relay.hurry();
            }
        }
    }

    /// Group `g`'s link, if it runs.
    pub(super) fn group_live(&self, g: usize) -> Option<&super::Live> {
        let group = &self.data.groups[g];
        self.links.get(&Link {
            who: link_who(&group.id),
            relay: group.relay_url.clone(),
        })
    }

    /// ADR-038: group `g`'s link has not reached the relay yet.
    pub fn group_connecting(&self, g: usize) -> bool {
        self.group_live(g).is_some_and(|l| !l.connected)
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
                // Our own devices join by themselves (ADR-028), never by invite.
                !c.own
                    && c.profile.is_none()
                    && !c.left
                    && !c.blocked
                    && !self.is_request(&c.key)
                    && !group.members().iter().any(|m| m.ed25519 == c.key)
            })
            .collect()
    }

    pub fn invite_to_group(&mut self, g: usize, i: usize) -> Result<(), String> {
        if self.data.contacts.get(i).is_some_and(|c| c.own) {
            return Err("your own devices join your groups by themselves".into());
        }
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
        let level = c.understands;
        let group = &self.data.groups[g];
        if !group.is_admin() {
            return Err("only the group's admins add members".into());
        }
        // ADR-042, section 7: a group with helpers takes only apps that read them.
        if !unheard && !group.admins.is_empty() && level < proto::ADMINS_LEVEL {
            return Err("their app is older; this group comes after they update".into());
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
            self.devices_welcome(g) && self.data.contacts[i].understands >= proto::DEVICES_LEVEL;
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
        // ADR-042: with admin lists, the inviter and the roster's signer are
        // admins on the last one, and every admin is a member. The inviter
        // is trusted with the list as with the roster.
        let list = inv.admins.last();
        let ok = match list {
            None => inv.roster.admin == inv.from,
            Some(l) => {
                termchat_core::verify_admins(l).is_ok()
                    && l.group == inv.roster.group
                    && l.no <= inv.roster.no
                    && l.admins.contains(&inv.from)
                    && l.admins.contains(&inv.roster.admin)
                    && l.admins
                        .iter()
                        .all(|a| inv.roster.members.iter().any(|m| m.ed25519 == *a))
            }
        };
        if !ok {
            return Err("the invite does not come from the group's admin".into());
        }
        let admins: Vec<AdminList> = list.cloned().into_iter().collect();
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
            admins,
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
            .map(|_| ())
            .map_err(GroupScriptError::message)
    }

    pub fn send_group_text(&mut self, g: usize, text: &str) -> Result<u64, GroupScriptError> {
        if !self.group_access(g) {
            return Err(GroupScriptError::Refused(GROUP_ACCESS_LOST.into()));
        }
        self.send_group_with(g, text, None)
    }

    pub fn group_text_accepted(
        &self,
        group: &GroupId,
        id: u64,
    ) -> Result<Option<Instant>, GroupScriptError> {
        use GroupScriptError::{Refused, State};
        let g = self
            .group_index(group)
            .ok_or_else(|| State("group disappeared".into()))?;
        if !self.group_access(g) {
            return Err(Refused(GROUP_ACCESS_LOST.into()));
        }
        let message = self.data.groups[g]
            .messages
            .iter()
            .find(|m| m.group_local && m.id == Some(id) && !m.deleted);
        match message.map(|m| (&m.status, m.group_accepted_at)) {
            Some((Status::Sent, Some(at))) => Ok(Some(at)),
            Some((Status::Queued | Status::Sending(_), _)) => Ok(None),
            Some((Status::Failed(why), _))
                if matches!(why.as_str(), GROUP_SAVE_FAILED | GROUP_ENCRYPT_FAILED) =>
            {
                Err(State(why.clone()))
            }
            Some((Status::Failed(why), _)) => Err(Refused(why.clone())),
            _ => Err(State(
                "group message disappeared before confirmation".into(),
            )),
        }
    }

    /// Takes authenticated group texts once, committing the markers before handoff.
    pub fn take_group_texts(
        &mut self,
        g: usize,
        limit: usize,
    ) -> Result<(Vec<Msg>, bool), GroupScriptError> {
        if !self.group_access(g) {
            return Err(GroupScriptError::Refused(GROUP_ACCESS_LOST.into()));
        }
        let at = now();
        let mut taken = Vec::new();
        let mut fresh = Vec::new();
        let mut more = false;
        for (i, m) in self.data.groups[g].messages.iter_mut().enumerate() {
            if !m.group_unfetched {
                continue;
            }
            let visible = !m.mine
                && !m.deleted
                && m.file.is_none()
                && m.author.is_some()
                && m.id.is_some()
                && m.status == Status::Received
                && m.expires_at.is_none_or(|expiry| expiry > at);
            if visible && fresh.len() == limit {
                more = true;
                break;
            }
            m.group_unfetched = false;
            taken.push(i);
            if visible {
                fresh.push(m.clone());
            }
        }
        if !taken.is_empty() && !self.persist() {
            for i in taken {
                self.data.groups[g].messages[i].group_unfetched = true;
            }
            // A directory sync can fail after the atomic rename.
            self.persist();
            return Err(GroupScriptError::State(
                "could not save group read markers".into(),
            ));
        }
        Ok((fresh, more))
    }

    fn send_group_with(
        &mut self,
        g: usize,
        text: &str,
        reply_to: Option<Ref>,
    ) -> Result<u64, GroupScriptError> {
        use GroupScriptError::{Refused, State};
        if text.len() > proto::MAX_TEXT_BYTES {
            return Err(Refused("message too long".into()));
        }
        let group = self
            .data
            .groups
            .get(g)
            .ok_or_else(|| State("no such group".into()))?;
        if !self.group_locally_joined(g) {
            return Err(Refused(GROUP_ACCESS_LOST.into()));
        }
        let id = getrandom::u64().map_err(|e| State(e.to_string()))?;
        if group.messages.iter().any(|m| m.mine && m.id == Some(id)) {
            return Err(State("group message ID collision".into()));
        }
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
            group_local: true,
            ..Default::default()
        });
        if !self.persist() {
            self.data.groups[g].messages.pop();
            self.persist();
            return Err(State(GROUP_SAVE_FAILED.into()));
        }
        self.flush_group(g);
        Ok(id)
    }

    /// A document of up to 1 MiB into group `g` (ADR-027 amendment): sent in
    /// chunks at the chat pace, one at a time.
    pub fn send_group_file(&mut self, g: usize, name: &str, bytes: Vec<u8>) -> Result<(), String> {
        let group = self.data.groups.get(g).ok_or("no such group")?;
        if !self.group_locally_joined(g) {
            return Err(GROUP_ACCESS_LOST.into());
        }
        let name = name.trim();
        proto::check_file_name(name).map_err(|_| "this file name cannot be sent")?;
        if bytes.is_empty() {
            return Err("the file is empty".into());
        }
        let size = match u32::try_from(bytes.len()) {
            Ok(n) if n <= proto::MAX_FILE_BYTES => n,
            _ => return Err("files are limited to 1 MiB".into()),
        };
        let busy = |m: &Msg| m.mine && m.file.as_ref().is_some_and(|f| f.data.is_some());
        if group.messages.iter().any(busy) {
            return Err("wait until the previous file is sent".into());
        }
        let mut id = [0; 4];
        getrandom::fill(&mut id).map_err(|e| e.to_string())?;
        let at = now();
        let expires_in = group.timer();
        self.data.groups[g].messages.push(Msg {
            mine: true,
            text: name.to_string(),
            at,
            expires_at: expires_in.map(|s| at + u64::from(s)),
            status: Status::Queued,
            file: Some(FileMsg {
                id: u32::from_le_bytes(id),
                name: name.to_string(),
                size,
                done: 0,
                data: Some(bytes),
            }),
            unfetched: false,
            ..Default::default()
        });
        self.persist();
        self.send_chunks();
        Ok(())
    }

    /// Chunks of our documents in groups, from the shared budget: `true`
    /// if one went out.
    pub(super) fn send_group_chunks(&mut self) -> bool {
        let mut sent = false;
        for g in 0..self.data.groups.len() {
            let group = &self.data.groups[g];
            let queued = |m: &Msg| m.mine && m.status == Status::Queued && m.file.is_some();
            let Some(m) = group.messages.iter().position(queued) else {
                continue;
            };
            if !self.group_access(g) || self.group_moving(g) || !self.group_link_up(g) {
                continue;
            }
            let (gid, expires_in) = (group.id, group.timer());
            while self.chunk_budget.0 >= 1.0 {
                let Some(f) = self.data.groups[g].messages[m].file.as_mut() else {
                    break;
                };
                let Some(body) = f.next_chunk(expires_in) else {
                    break;
                };
                let Ok(bytes) = self.core().group_encrypt(gid, body) else {
                    break;
                };
                // Counted before the send, so a restart never repeats a chunk.
                let group = &mut self.data.groups[g];
                let Some(f) = group.messages[m].file.as_mut() else {
                    break;
                };
                f.done += 1;
                let last = f.complete();
                let client_id = group.next_client_id;
                group.next_client_id += 1;
                self.chunk_budget.0 -= 1.0;
                sent = true;
                if !self.persist() {
                    return sent;
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
                self.group_send_ciphertext(g, frame);
                if last {
                    let msg = &mut self.data.groups[g].messages[m];
                    msg.status = Status::Sending(client_id);
                    if let Some(f) = msg.file.as_mut() {
                        f.data = None;
                    }
                    break;
                }
            }
        }
        sent
    }

    /// Name and bytes of received document `m` of group `g`, whole and not
    /// yet saved or discarded.
    pub fn group_file_bytes(&self, g: usize, m: usize) -> Option<(String, Vec<u8>)> {
        let msg = self.data.groups.get(g)?.messages.get(m)?;
        let f = msg.file.as_ref().filter(|f| !msg.mine && f.complete())?;
        Some((f.name.clone(), f.data.clone()?))
    }

    /// The newest received document of group `g` that waits to be saved.
    pub fn unsaved_group_file(&self, g: usize) -> Option<usize> {
        let group = self.data.groups.get(g)?;
        group.messages.iter().rposition(|m| {
            !m.mine
                && m.file
                    .as_ref()
                    .is_some_and(|f| f.complete() && f.data.is_some())
        })
    }

    /// Forgets the bytes of received document `m`; the line stays.
    pub fn discard_group_file(&mut self, g: usize, m: usize) {
        let msg = self
            .data
            .groups
            .get_mut(g)
            .and_then(|x| x.messages.get_mut(m));
        if let Some(f) = msg.filter(|m| !m.mine).and_then(|m| m.file.as_mut()) {
            f.data = None;
            self.persist();
        }
    }

    /// Writes received document `m` of group `g` into `dir`, mode 0600,
    /// under a name not taken yet, then forgets its bytes (ADR-017).
    pub fn save_group_file(&mut self, g: usize, m: usize, dir: &Path) -> Result<PathBuf, String> {
        let (name, bytes) = self.group_file_bytes(g, m).ok_or("no file to save")?;
        let path = data::save_new(dir, &safe_name(&name), &bytes)
            .map_err(|e| format!("cannot save: {e}"))?;
        self.discard_group_file(g, m);
        Ok(path)
    }

    /// `text` in group `g` as an answer to message `m`.
    pub fn reply_group(&mut self, g: usize, m: usize, text: &str) -> Result<(), String> {
        let target = self.group_target(g, m)?;
        self.send_group_with(g, text, Some(target))
            .map(|_| ())
            .map_err(GroupScriptError::message)
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
        if !self.group_access(g) || self.group_moving(g) || !self.group_link_up(g) {
            return;
        }
        if self.core().group_due(&gid, now()) {
            self.start_group_session(g);
            self.share_key(g);
        }
        let expires_in = self.data.groups[g].timer();
        for m in 0..self.data.groups[g].messages.len() {
            let msg = &self.data.groups[g].messages[m];
            // Documents go chunk by chunk, at their own pace (`send_chunks`).
            if !msg.mine
                || msg.status != Status::Queued
                || msg.file.is_some()
                || msg.expires_at.is_some_and(|expiry| expiry <= now())
            {
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
                Err(e) => {
                    self.note(format!("group: {e}"));
                    self.data.groups[g].messages[m].status =
                        Status::Failed(GROUP_ENCRYPT_FAILED.into());
                    continue;
                }
            };
            // The ratchet moved: saved before anything goes out (as `transmit`).
            let client_id = self.data.groups[g].next_client_id;
            self.data.groups[g].next_client_id += 1;
            self.data.groups[g].messages[m].status = Status::Sending(client_id);
            if !self.persist() {
                self.data.groups[g].messages[m].status = Status::Failed(GROUP_SAVE_FAILED.into());
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
            if !self.group_send_ciphertext(g, frame) {
                self.data.groups[g].messages[m].status = Status::Queued;
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
        self.links.get(&link).is_some_and(|l| l.connected)
            && group.send_id != [0; 16]
            // A goodbye from a group we left does not wait: we read no more.
            && (group.reader.is_none() || group.left || self.group_read.contains(&group.id))
    }

    /// A new Megolm session for group `g`.
    fn start_group_session(&mut self, g: usize) {
        let gid = self.data.groups[g].id;
        if self.core().group_start(gid, now()).is_err() {
            return;
        }
        let group = &mut self.data.groups[g];
        group.shared.clear();
        // Everyone in the group now reads the new session from its start.
        group.since = group.devices().iter().map(|m| (m.ed25519, 0)).collect();
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
        // ADR-042, amendment 2: words for readers that still lack a key.
        let waiting: Vec<QueueId> = self.data.groups[g]
            .reader_outbox
            .iter()
            .map(|(_, _, at)| *at)
            .collect();
        for send_id in waiting {
            self.group_send(g, ClientFrame::ClaimKey { send_id });
        }
        if self.removed() || self.data.groups[g].left || self.group_moving(g) {
            return;
        }
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
            // ADR-030: a roster never changes a key we hold; that member's
            // messages then do not read, which is safer than a planted key.
            let _ = core.add_member(m.ed25519, m.curve25519);
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
        if self.removed() {
            return;
        }
        // ADR-042, amendment 3, point 5: the link fell on `RemoveReaderAt`
        // and no answer came: the relay is too old; the old queue goes now.
        if self.data.groups[g]
            .retiring
            .as_ref()
            .is_some_and(|r| r.probe)
        {
            self.note(
                "this group's relay is older: members not in your contacts may need a new invite",
            );
            if let Some(r) = self.data.groups[g].retiring.as_mut() {
                r.probe = false;
                r.until = 0;
            }
        }
        self.retire_old_queues();
        if self.data.groups[g].left {
            return self.flush_group(g);
        }
        if self.data.groups[g].moving.is_some() {
            if !self.persist() {
                return self.retry_group_move_later(g);
            }
            let group = &self.data.groups[g];
            if group.asking.is_empty() && group.moving.as_ref().is_some_and(|m| m.old.is_some()) {
                return self.finish_move(g);
            }
        }
        let group = &self.data.groups[g];
        match (group.admin, group.moving.as_ref()) {
            (_, Some(m)) if m.old.is_none() => {
                self.group_send(g, ClientFrame::CreateGroup);
            }
            (None, None) if group.is_admin_to_be() => {
                self.group_send(g, ClientFrame::CreateGroup);
            }
            (Some((group_id, token)), _) => {
                for _ in 0..group.asking.len() {
                    self.group_send(g, ClientFrame::AddReader { group_id, token });
                }
            }
            _ => {}
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
                let creating = group.is_admin_to_be();
                match group.moving.as_mut() {
                    // The new queue of a move; the old one goes once all moved.
                    Some(m) if m.old.is_none() && group.admin.is_some() => {
                        m.old = group.admin.take();
                    }
                    None if creating => {}
                    _ => return,
                }
                group.admin = Some((group_id, token));
                group.send_id = send_id;
                if !self.persist() && self.data.groups[g].moving.is_some() {
                    return self.retry_group_move_later(g);
                }
                for _ in 0..self.data.groups[g].asking.len() {
                    self.group_send(g, ClientFrame::AddReader { group_id, token });
                }
            }
            ServerFrame::ReaderAdded {
                recv_id,
                send_id,
                token,
                group_id,
            } => {
                let group = &self.data.groups[g];
                if group.admin.is_none_or(|(id, _)| id != group_id)
                    || group.moving.as_ref().is_some_and(|m| m.old.is_none())
                {
                    return;
                }
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
                    self.group_devices_changed(who, &[]);
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
                    // ADR-042: the admin lists go just before the invite.
                    let group = &self.data.groups[g];
                    if !group.admins.is_empty() {
                        let lists = Body::GroupAdmins {
                            group: group.id,
                            chain: group.admins.clone(),
                        };
                        self.group_tell(&who, lists);
                    }
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
                if r.recv_id != recv_id {
                    return;
                }
                // ADR-042, amendment 3: what waited for us is read; we write now.
                if self.group_read.insert(self.data.groups[g].id) {
                    self.flush_group(g);
                }
                if one_time_left >= CONTACT_KEYS {
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
                // ADR-042, amendment 2: a word for a reader, maybe in a queue
                // the group has left.
                let waiting = self.data.groups[g]
                    .reader_outbox
                    .iter()
                    .find(|(_, _, at)| *at == send_id)
                    .map(|(k, ..)| *k);
                if let Some(who) = waiting {
                    if self.has_session(&who) || self.core().start_session(&who, &key).is_ok() {
                        self.flush_reader_outbox(g, &who);
                    }
                    return;
                }
                if self.removed() || self.data.groups[g].left || self.group_moving(g) {
                    return;
                }
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
                if self.group_send_ciphertext(
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
            ServerFrame::ReaderRemoved { .. } => {
                if let Some(r) = self.data.groups[g].retiring.as_mut() {
                    r.probe = false;
                    self.persist();
                }
            }
            ServerFrame::Accepted { client_id } => {
                let at = Instant::now();
                for m in &mut self.data.groups[g].messages {
                    if m.status == Status::Sending(client_id) {
                        m.status = Status::Sent;
                        m.group_accepted_at = Some(at);
                    }
                }
                self.persist();
            }
            ServerFrame::Error {
                client_id: Some(client_id),
                code,
            } => {
                for m in &mut self.data.groups[g].messages {
                    if m.status == Status::Sending(client_id) {
                        m.status = Status::Failed(why(code));
                    }
                }
                self.persist();
            }
            ServerFrame::Error {
                client_id: None,
                code: ErrorCode::RateLimited | ErrorCode::Internal,
            } if self.data.groups[g].moving.is_some() => {
                self.retry_group_move_later(g);
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
        if self.removed() || self.data.groups[g].left {
            return true;
        }
        let gid = self.data.groups[g].id;
        if envelope.olm_type == OLM_TYPE_MEGOLM {
            let got = self.core().group_decrypt(&gid, &envelope.body);
            match got {
                Ok((sender, body)) => {
                    self.group_body(g, sender, body);
                    self.group_gaps(g);
                }
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
        // A session key from a member, or an admin's word to us alone, over Olm.
        let got = self.core().decrypt_on_reader(envelope);
        if let Ok(r) = got {
            self.group_from_reader(r.sender, r.body);
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
        self.seen_on_new_queue(g, &sender);
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
        if device == self.me() || self.removed() || self.data.groups[g].left {
            return;
        }
        // ADR-028: a device speaks for its member; ours for us.
        let Some(sender) = self.data.groups[g].owner(&device) else {
            return;
        };
        self.seen_on_new_queue(g, &device);
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
                if group
                    .messages
                    .iter()
                    .any(|m| m.id == Some(id) && (m.author == Some(sender) || (m.mine && mine)))
                {
                    return;
                }
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
                    group_unfetched: !mine,
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
            // shortcut: a document sent from another device of ours does not
            // show here; mirror it as chats do (`own_copy`) if people miss it.
            body @ Body::File { .. } if !mine => {
                if self.data.take_group_chunk(g, sender, body, now()) {
                    let group = &mut self.data.groups[g];
                    if self.open_group != Some(group.id) {
                        group.unread += 1;
                    }
                    let id = group.id;
                    self.happened.push(Happened::GroupAlert { group: id });
                }
            }
            Body::GroupRoster { roster } if device == sender => self.take_roster(g, sender, roster),
            Body::GroupAdmins { group, chain }
                if device == sender && group == self.data.groups[g].id =>
            {
                self.hear_admins(g, sender, chain);
            }
            // ADR-042: kept by the helpers for when the main admin's goodbye comes.
            Body::GroupResign { group, resign }
                if device == sender
                    && group == self.data.groups[g].id
                    && self.data.groups[g].main_admin() == Some(sender) =>
            {
                self.data.groups[g].resign = Some(resign);
            }
            Body::GroupDevices {
                member,
                list,
                readers,
                ..
            } if device == sender => self.take_member_devices(g, sender, member, list, readers),
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
            Body::GroupLeave { group } if group == self.data.groups[g].id => {
                let name = self.member_name(g, &sender);
                self.group_system(g, format!("{name} left the group"));
                let me = self.me();
                let admins = self.data.groups[g].admin_keys();
                if admins.first() == Some(&me) {
                    let _ = self.remove_from_group(g, &sender);
                } else if admins.first() == Some(&sender) {
                    // ADR-042, section 4: the next admin takes over with the
                    // main admin's word; the others wait for their move.
                    if admins.get(1) == Some(&me) {
                        self.take_over(g);
                    } else if admins.len() > 1 {
                        self.wait_for_group_move(g);
                    }
                } else {
                    self.wait_for_group_move(g);
                }
            }
            _ => {}
        }
    }

    /// A newer roster, from the admin, signed: new members learn our key.
    fn take_roster(&mut self, g: usize, sender: Ed25519Key, roster: Roster) {
        if sender != roster.admin {
            return;
        }
        let Some(lists) = self.roster_lists(g, &roster, false) else {
            return;
        };
        let current = self.data.groups[g].roster.as_ref();
        let had_access = self.group_access(g);
        let losing_member = current.is_some_and(|r| {
            r.members
                .iter()
                .any(|old| !roster.members.iter().any(|new| new.ed25519 == old.ed25519))
        });
        let no = roster.no;
        let me = self.me();
        self.data.groups[g].set_roster(roster, &me);
        let moves = self.take_admin_lists(g, lists, no);
        if had_access && !self.group_access(g) {
            self.lose_group_access(g);
            return;
        }
        if moves {
            self.move_as_main(g);
        } else if losing_member && self.data.groups[g].main_admin() != Some(me) {
            self.wait_for_group_move(g);
        }
        self.know_members(g);
        self.replay_pending_keys(g);
        self.share_key(g);
        self.flush_group(g);
    }

    /// ADR-042: whether `roster`, signed by `roster.admin`, may follow ours,
    /// with the admin lists heard for it; those lists, oldest first, if so.
    /// `by_main`: only the main admin's will do, as for a move.
    fn roster_lists(&self, g: usize, roster: &Roster, by_main: bool) -> Option<Vec<AdminList>> {
        let group = &self.data.groups[g];
        let current = group.roster.as_ref()?;
        if roster.group != group.id || roster.no <= current.no || verify_roster(roster).is_err() {
            return None;
        }
        let signer = roster.admin;
        // Lists for rosters we missed, then this roster's own, from its signer.
        let mut heard: Vec<&AdminList> = group
            .pending_admins
            .iter()
            .map(|(_, l)| l)
            .filter(|l| l.no > current.no && (l.no < roster.no || l.signer == signer))
            .filter(|l| l.no <= roster.no)
            .collect();
        heard.sort_by_key(|l| l.no);
        let mut set = group.admin_keys();
        let mut before = set.clone();
        let mut since = current.no;
        let mut lists: Vec<AdminList> = Vec::new();
        for l in heard {
            if l.no == since || check_admin_change(&group.id, &set, since, l).is_err() {
                continue;
            }
            before = std::mem::replace(&mut set, l.admins.clone());
            since = l.no;
            lists.push(l.clone());
        }
        let own = lists.last().filter(|l| l.no == roster.no);
        let rights = if own.is_some() { &before } else { &set };
        let main = rights.first() == Some(&signer)
            || own.is_some_and(|l| l.resign.is_some() && set.first() == Some(&signer));
        // Review: a chain over missed rosters only from the main admin, so a
        // helper cannot slip in a list that lost its race.
        let gap = lists.iter().any(|l| l.no < roster.no);
        let helper_ok = !by_main
            && !gap
            && rights.contains(&signer)
            && roster.name == current.name
            && roster.expires_in == current.expires_in
            && roster.members.starts_with(&current.members);
        if !(main && !(gap && set.first() != Some(&signer)) || helper_ok) {
            return None;
        }
        // Every admin in force is a member. Before any list the creator
        // alone signs, and a roster of theirs may still be empty.
        let named = set
            .iter()
            .all(|a| roster.members.iter().any(|m| m.ed25519 == *a));
        (named || group.admins.is_empty() && lists.is_empty()).then_some(lists)
    }

    /// ADR-042: takes the lists that came with roster `no` and says what
    /// changed. `true`: we are the main admin and the group must move.
    fn take_admin_lists(&mut self, g: usize, lists: Vec<AdminList>, no: u64) -> bool {
        let me = self.me();
        let group = &mut self.data.groups[g];
        group.pending_admins.retain(|(_, l)| l.no > no);
        if lists.is_empty() {
            return false;
        }
        let old = group.admin_keys();
        group.admins.extend(lists);
        let drop = group.admins.len().saturating_sub(proto::MAX_ADMIN_CHAIN);
        group.admins.drain(..drop);
        group.resign = None;
        let new = group.admin_keys();
        let signer = group.admins.last().map(|l| l.signer).unwrap_or_default();
        if !new.contains(&me) {
            // Out of the role: the token is no longer ours to use.
            group.admin = None;
            group.pending_grant = None;
        }
        let name = |k: &Ed25519Key| group.name_of(k).map_or("someone".into(), clean);
        let mut lines = Vec::new();
        let handed = old.first() != new.first();
        match (old.first(), new.first()) {
            (Some(was), Some(main)) if handed && *was == signer => {
                lines.push(format!(
                    "{} handed the group over to {}",
                    name(was),
                    name(main)
                ));
            }
            (_, Some(main)) if handed => {
                lines.push(format!("{} is now the main admin", name(main)));
            }
            _ => {}
        }
        for k in new.iter().skip(1).filter(|k| !old.contains(k)) {
            lines.push(format!("{} made {} an admin", name(&signer), name(k)));
        }
        for k in old.iter().filter(|k| !new.contains(k)) {
            if handed && old.first() == Some(k) {
                continue;
            }
            if *k == signer {
                lines.push(format!("{} stepped down as admin", name(k)));
            } else if old.first() != Some(k) {
                lines.push(format!("{} removed {} as admin", name(&signer), name(k)));
            }
        }
        for l in lines {
            self.group_system(g, l);
        }
        self.take_pending_grant(g);
        let group = &self.data.groups[g];
        // Who knew the token and lost the role: the main admin moves.
        let lost = old.iter().any(|k| !new.contains(k));
        group.main_admin() == Some(me) && lost
    }

    /// ADR-042: we are the main admin and someone who knew the token lost
    /// the role; once we hold it, the group moves without it.
    fn move_as_main(&mut self, g: usize) {
        if self.data.groups[g].admin.is_none() {
            self.data.groups[g].owe_move = true;
            return self.persist_unit();
        }
        self.data.groups[g].owe_move = false;
        self.start_move(g, Vec::new());
        self.persist();
    }

    /// ADR-042: admin lists heard, kept for the roster they come with.
    fn hear_admins(&mut self, g: usize, from: Ed25519Key, chain: Vec<AdminList>) {
        let at = self.data.groups[g].roster.as_ref().map_or(0, |r| r.no);
        let gid = self.data.groups[g].id;
        let pending = &mut self.data.groups[g].pending_admins;
        // Review: a sender's new chain replaces their old one, so a member
        // who floods lists crowds out only their own.
        pending.retain(|(k, _)| *k != from);
        let fresh = chain.into_iter().filter(|l| l.no > at && l.group == gid);
        pending.extend(fresh.take(proto::MAX_ADMIN_CHAIN).map(|l| (from, l)));
    }

    /// ADR-042: the main admin's token, once a roster names us an admin.
    fn take_pending_grant(&mut self, g: usize) {
        let me = self.me();
        let group = &mut self.data.groups[g];
        let Some((from, no, _)) = &group.pending_grant else {
            return;
        };
        let at = group.roster.as_ref().map_or(0, |r| r.no);
        if *no > at {
            return;
        }
        // Kept until a roster makes us an admin: the token may come first.
        if !group.admin_keys().contains(&me) {
            return;
        }
        // The main admin's, or the one who just handed the group to us.
        let handed = group.admins.last().is_some_and(|l| l.signer == *from);
        let ok = group.main_admin() == Some(*from) || handed;
        if let Some((_, _, grant)) = group.pending_grant.take().filter(|_| ok) {
            group.admin = Some((grant.queue, grant.token));
            if group.owe_move && group.main_admin() == Some(me) {
                self.move_as_main(g);
            }
        }
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
        self.main_check(g)
            .map_err(|_| "only the group's main admin removes members")?;
        let group = &self.data.groups[g];
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
        // The move first: it notes every reader in the old queue, theirs too.
        self.start_move(g, gone);
        self.data.groups[g]
            .member_devices
            .retain(|(m, ..)| m != who);
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
        self.stop_group_writes(g, false);
        let at: Vec<(Ed25519Key, QueueId)> = self.data.groups[g]
            .devices()
            .iter()
            .map(|m| (m.ed25519, m.reader))
            .collect();
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
                m.reinvite.retain(|k| !gone.contains(k));
            }
            None => {
                // Invites not answered yet point at the old queue: they are
                // made again for the new one.
                let mut reinvite: Vec<Ed25519Key> = group
                    .invited
                    .drain(..)
                    .map(|(k, _)| k)
                    .filter(|k| !gone.contains(k))
                    .collect();
                for k in &group.asking {
                    if !gone.contains(k) && !stay.contains(k) && !reinvite.contains(k) {
                        reinvite.push(*k);
                    }
                }
                let mut asking = stay.clone();
                asking.extend(&reinvite);
                group.moving = Some(GroupMoving {
                    stay,
                    removed: gone,
                    grants: Vec::new(),
                    old: None,
                    reinvite,
                    at,
                });
                group.asking = asking;
                self.group_connected(g);
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
        let Some(moving) = self.data.groups[g].moving.clone() else {
            return;
        };
        let Some(old_roster) = self.data.groups[g].roster.clone() else {
            return;
        };
        // ADR-042, amendment 2: those we have no chat with hear of the move
        // through their reader in the old queue.
        let old_readers: Vec<(Ed25519Key, QueueId)> = if moving.at.is_empty() {
            self.data.groups[g]
                .devices()
                .iter()
                .map(|m| (m.ed25519, m.reader))
                .collect()
        } else {
            moving.at.clone()
        };
        let old_at = |k: &Ed25519Key| old_readers.iter().find(|(x, _)| x == k).map(|(_, r)| *r);
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
        // ADR-042: `admin` names the roster's signer, us.
        roster.admin = me;
        roster.signature = self.core().sign_roster(&roster.signed_bytes());
        let group = &mut self.data.groups[g];
        group.waiting_for_move = false;
        group
            .outbox
            .retain(|body| !matches!(body, Body::GroupRoster { .. } | Body::GroupDevices { .. }));
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
                .filter(|(k, _)| *k != list.identity && !moving.removed.contains(k))
                .collect();
        }
        let send_id = group.send_id;
        self.reset_group_members(g, &moving.removed);
        if !self.persist() {
            return self.retry_group_move_later(g);
        }
        let gid = self.data.groups[g].id;
        let chain = self.data.groups[g].admins.clone();
        let mut through_reader = Vec::new();
        for (who, grant) in &moving.grants {
            if *who == me || moving.removed.contains(who) || moving.reinvite.contains(who) {
                continue;
            }
            if !self.has_chat(who) {
                through_reader.push(*who);
            }
            // ADR-042: the lists first, so a new main admin's move is taken.
            if !chain.is_empty() {
                let lists = Body::GroupAdmins {
                    group: gid,
                    chain: chain.clone(),
                };
                self.group_tell_in(g, who, lists, old_at(who));
            }
            let body = Body::GroupMove {
                roster: roster.clone(),
                send_id,
                reader: grant.clone(),
            };
            self.group_tell_in(g, who, body, old_at(who));
        }
        for who in &moving.reinvite {
            let Some(grant) = reader_of(who) else {
                continue;
            };
            let group = &mut self.data.groups[g];
            group.invited.retain(|(k, _)| k != who);
            group.invited.push((*who, grant.clone()));
            let body = Body::GroupInvite {
                roster: roster.clone(),
                relay_url: group.relay_url.clone(),
                onion_url: group.onion_url.clone(),
                send_id,
                reader: grant,
            };
            if !chain.is_empty() {
                let lists = Body::GroupAdmins {
                    group: gid,
                    chain: chain.clone(),
                };
                self.group_tell(who, lists);
            }
            self.group_tell(who, body);
        }
        for who in &moving.removed {
            self.group_tell_in(g, who, Body::GroupLeave { group: gid }, old_at(who));
            if let (Some((group_id, token)), Some(grant)) =
                (self.data.groups[g].admin, reader_of(who))
            {
                self.group_send(
                    g,
                    ClientFrame::RemoveReader {
                        group_id,
                        token,
                        recv_id: grant.recv_id,
                    },
                );
            }
        }
        if let Some((group_id, token)) = moving.old {
            if through_reader.is_empty() {
                self.group_send(g, ClientFrame::DeleteGroup { group_id, token });
            } else {
                // ADR-042, amendment 3: the old queue lives until those told
                // through it spoke on the new one, or a day; the removed lose
                // their readers in it now.
                for who in &moving.removed {
                    if let Some(send_id) = old_at(who) {
                        let frame = ClientFrame::RemoveReaderAt {
                            group_id,
                            token,
                            send_id,
                        };
                        self.group_send(g, frame);
                    }
                }
                self.data.groups[g].retiring = Some(crate::data::Retiring {
                    old: (group_id, token),
                    until: now() + RETIRE_AFTER,
                    waiting: through_reader,
                    probe: !moving.removed.is_empty(),
                });
            }
        }
        // ADR-042: the helpers get the new queue's token.
        for helper in self.data.groups[g].admin_keys().into_iter().skip(1) {
            self.send_admin_keys(g, &helper);
        }
        self.data.groups[g].moving = None;
        if !self.persist() {
            self.data.groups[g].moving = Some(moving);
            return self.retry_group_move_later(g);
        }
        self.group_move_retries.remove(&gid);
        self.subscribe_group(g);
        self.share_key(g);
        let members: Vec<Ed25519Key> = self.data.groups[g]
            .members()
            .iter()
            .map(|m| m.ed25519)
            .collect();
        for who in members {
            self.group_devices_changed(who, &[]);
        }
        self.resend_devices(g);
        self.flush_group(g);
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
        self.reset_group_members(g, gone);
        self.share_key(g);
        self.persist();
    }

    fn reset_group_members(&mut self, g: usize, gone: &[Ed25519Key]) {
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
        self.start_group_session(g);
    }

    /// What a member is called in group `g`: their roster nickname.
    fn member_name(&self, g: usize, who: &Ed25519Key) -> String {
        self.data.groups[g]
            .members()
            .iter()
            .find(|m| m.ed25519 == *who)
            .map_or_else(|| "someone".to_string(), |m| clean(&m.nickname))
    }

    /// ADR-030, section 6: a relay that withholds group messages is noticed.
    fn group_gaps(&mut self, g: usize) {
        let Some(group) = self.data.groups.get(g) else {
            return;
        };
        let missed = self.core().group_missed(&group.id);
        let group = &mut self.data.groups[g];
        crate::engine::show_missed_in(&mut group.messages, &mut group.missed, missed);
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
        self.main_check(g)
            .map_err(|_| "only the group's main admin sets the timer")?;
        let group = &self.data.groups[g];
        let Some(mut roster) = group.roster.clone() else {
            return Err("the group is not ready yet".into());
        };
        let me = self.me();
        roster.expires_in = expires_in;
        roster.no += 1;
        roster.admin = me;
        roster.signature = self.core().sign_roster(&roster.signed_bytes());
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
        let me = self.me();
        let admins = group.admin_keys();
        let roster_no = group.roster.as_ref().map_or(0, |r| r.no);
        // Frames still unconfirmed are dropped first, so what follows goes out.
        self.stop_group_writes(g, true);
        if admins.len() > 1 && admins.first() == Some(&me) {
            // ADR-042, section 4: our word, so the next admin can take over.
            let resign = self.core().sign_resign(&gid, roster_no);
            self.send_group_body(g, Body::GroupResign { group: gid, resign });
        } else if admins.len() > 1 && admins.contains(&me) {
            // A helper leaves the role first: the main admin moves the group.
            self.step_down(g)?;
        }
        self.data.groups[g].left = true;
        self.send_group_body(g, Body::GroupLeave { group: gid });
        // The keys go once the goodbye is out (`flush_group`).
        self.flush_group(g);
        Ok(())
    }

    pub(super) fn lose_group_access(&mut self, g: usize) {
        self.stop_group_writes(g, true);
        let group = &mut self.data.groups[g];
        group.left = true;
        self.core().group_forget(&self.data.groups[g].id);
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
        // ADR-042: a helper signs the rosters of those they invite.
        roster.admin = me;
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
                    .is_some_and(|i| self.data.contacts[i].understands >= proto::DEVICES_LEVEL)
        })
    }

    /// Admin: `who`'s device list changed. New devices get readers and
    /// invites; devices gone from it make the group move without them.
    pub(super) fn group_devices_changed(&mut self, who: Ed25519Key, revoked: &[Ed25519Key]) {
        let Some(list) = self.device_list_of(&who) else {
            return;
        };
        for g in 0..self.data.groups.len() {
            let group = &self.data.groups[g];
            if group.left {
                continue;
            }
            if group
                .member_devices
                .iter()
                .any(|(m, old, _)| *m == who && old.no > list.no)
            {
                continue;
            }
            let member = group.members().iter().any(|m| m.ed25519 == who);
            let pending = |key: &Ed25519Key| {
                group.asking.contains(key)
                    || group.invited.iter().any(|(k, _)| k == key)
                    || group
                        .moving
                        .as_ref()
                        .is_some_and(|m| m.grants.iter().any(|(k, _)| k == key))
            };
            // A move under way: new devices wait for it (`finish_move` asks again).
            let welcome = group.is_admin() && member && self.devices_welcome(g);
            let listed = |k: &Ed25519Key| list.devices.iter().any(|d| d.ed25519 == *k);
            let had: Vec<Ed25519Key> = group
                .member_devices
                .iter()
                .filter(|(m, ..)| *m == who)
                .flat_map(|(_, _, r)| r.iter().map(|(k, _)| *k))
                .collect();
            let gone: BTreeSet<Ed25519Key> = had
                .iter()
                .filter(|k| !listed(k))
                .copied()
                .chain(revoked.iter().copied().filter(|k| pending(k)))
                .collect();
            if !group.is_admin() {
                if !gone.is_empty() {
                    if let Some((_, known, readers)) = self.data.groups[g]
                        .member_devices
                        .iter_mut()
                        .find(|(m, ..)| *m == who)
                    {
                        *known = list.clone();
                        readers.retain(|(k, _)| listed(k));
                    }
                    self.wait_for_group_move(g);
                    self.after_members_left(g, &gone.into_iter().collect::<Vec<_>>());
                }
                continue;
            }
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
                if self.data.groups[g].main_admin() == Some(self.me()) {
                    self.start_move(g, gone.into_iter().collect());
                } else {
                    // ADR-042, amendment 2: a helper tells the group; the
                    // main admin moves it.
                    self.publish_devices(g, who);
                    self.wait_for_group_move(g);
                }
            }
            if !welcome || new.is_empty() || self.data.groups[g].moving.is_some() {
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
        // ADR-042: any admin vouches for the readers of those they invited.
        let admin = group.admin_keys().contains(&sender).then_some(sender);
        let on_roster = group.members().iter().any(|m| m.ed25519 == member);
        // ADR-030, section 3: a device key that is another member's, or another
        // member's device, is not this member's (review 2026-11c, finding 5).
        let taken = list.devices.iter().any(|d| {
            d.ed25519 != member
                && (group.members().iter().any(|m| m.ed25519 == d.ed25519)
                    || group.member_devices.iter().any(|(m, l, _)| {
                        *m != member && l.devices.iter().any(|x| x.ed25519 == d.ed25519)
                    }))
        });
        if Some(sender) != admin
            || !on_roster
            || taken
            || list.identity != member
            || termchat_core::verify_devices(&list).is_err()
            || group
                .member_devices
                .iter()
                .any(|(who, previous, _)| *who == member && previous.no > list.no)
        {
            return;
        }
        let me = self.me();
        if !self.is_primary()
            && member == self.identity_key()
            && !list.devices.iter().any(|d| d.ed25519 == me)
        {
            self.lose_group_access(g);
            return;
        }
        let group = &mut self.data.groups[g];
        let gone: Vec<Ed25519Key> = group
            .member_devices
            .iter()
            .filter(|(m, ..)| *m == member)
            .flat_map(|(_, _, r)| r.iter().map(|(k, _)| *k))
            .filter(|k| {
                !readers.iter().any(|(x, _)| x == k)
                    || !list.devices.iter().any(|d| &d.ed25519 == k)
            })
            .collect();
        group.member_devices.retain(|(m, ..)| *m != member);
        group.member_devices.push((member, list, readers));
        // ADR-042: a helper's word on devices gone; the main admin moves.
        let main = self.data.groups[g].main_admin() == Some(me) && self.is_primary();
        if !gone.is_empty() && !main {
            self.wait_for_group_move(g);
        }
        self.know_members(g);
        self.replay_pending_keys(g);
        if gone.is_empty() {
            self.share_key(g);
        } else if main {
            self.start_move(g, gone);
        } else {
            self.after_members_left(g, &gone);
        }
        self.persist();
    }

    fn send_group_body(&mut self, g: usize, body: Body) {
        let goodbye = matches!(body, Body::GroupLeave { .. });
        if !goodbye && (self.removed() || self.data.groups[g].left) {
            return;
        }
        if !self.group_link_up(g) || (!goodbye && self.group_moving(g)) {
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
        self.group_send_ciphertext(
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
        self.group_from(from, Some(i), body);
    }

    /// ADR-042, amendment 2: what an admin may say to us one to one through
    /// our reader, when we are not their contact.
    fn group_from_reader(&mut self, from: Ed25519Key, body: Body) {
        match body {
            body @ Body::GroupKey { .. } => self.group_key_from(from, body),
            body @ (Body::GroupAdmins { .. }
            | Body::GroupMove { .. }
            | Body::GroupLeave { .. }
            | Body::GroupAdminKeys { .. }) => self.group_from(from, None, body),
            _ => {}
        }
    }

    /// Group bodies from `from` one to one: over the chat with contact
    /// `chat`, or through our reader.
    fn group_from(&mut self, from: Ed25519Key, chat: Option<usize>, body: Body) {
        match body {
            // ADR-042: lists go just before the roster they came with.
            Body::GroupAdmins { group, chain } => {
                match self
                    .data
                    .groups
                    .iter()
                    .position(|x| x.id == group && !x.left)
                {
                    Some(g) => self.hear_admins(g, from, chain),
                    None if chat.is_some() => {
                        let stash = &mut self.data.invite_admins;
                        stash.retain(|(f, x, _)| !(*f == from && *x == group));
                        if stash.len() >= proto::MAX_ADMIN_CHAIN {
                            stash.remove(0);
                        }
                        stash.push((from, group, chain));
                    }
                    None => {}
                }
            }
            Body::GroupAdminKeys {
                group,
                roster_no,
                grant,
            } => {
                let Some(g) = self
                    .data
                    .groups
                    .iter()
                    .position(|x| x.id == group && !x.left)
                else {
                    return;
                };
                // ADR-028: the token lives on the primary device alone.
                if !self.is_primary() {
                    return;
                }
                // Review: only the main admin, or who just handed the group
                // over, may leave a token waiting; nobody else replaces it.
                let group = &self.data.groups[g];
                let giver = group.main_admin() == Some(from)
                    || group.admins.last().is_some_and(|l| l.signer == from);
                if !giver {
                    return;
                }
                self.data.groups[g].pending_grant = Some((from, roster_no, grant));
                self.take_pending_grant(g);
            }
            Body::GroupInvite {
                roster,
                relay_url,
                onion_url,
                send_id,
                reader,
            } => {
                let Some(i) = chat else {
                    return;
                };
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
                    return self.group_from(from, chat, body);
                }
                // Review 2026-11c, finding 3: a group on an address we cannot use.
                if !self.peer_addr_ok(&relay_url, onion_url.as_deref()) {
                    self.note("a contact invited you to a group at an address that cannot be used");
                    return self.persist_unit();
                }
                let name = roster.name.clone();
                self.data
                    .group_invites
                    .retain(|x| x.roster.group != roster.group);
                let group = roster.group;
                let at = self
                    .data
                    .invite_admins
                    .iter()
                    .position(|(f, x, _)| *f == from && *x == group);
                let admins = at
                    .map(|n| self.data.invite_admins.remove(n).2)
                    .unwrap_or_default();
                self.data.group_invites.push(GroupInvite {
                    from,
                    admins,
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
            Body::GroupJoin { group } if chat.is_some() => {
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
                // ADR-042, amendment 2: only the main admin moves the group.
                let lists = (send_id != self.data.groups[g].send_id && roster.admin == from)
                    .then(|| self.roster_lists(g, &roster, true))
                    .flatten();
                let Some(lists) = lists else {
                    return;
                };
                let no = roster.no;
                let gone: Vec<Ed25519Key> = current
                    .map(|r| r.members)
                    .unwrap_or_default()
                    .iter()
                    .map(|m| m.ed25519)
                    .filter(|k| !roster.members.iter().any(|m| m.ed25519 == *k))
                    .collect();
                let had_access = self.group_access(g);
                self.stop_group_writes(g, false);
                let me = self.me();
                let group = &mut self.data.groups[g];
                group.set_roster(roster, &me);
                group.send_id = send_id;
                group.reader = Some(reader.clone());
                self.take_admin_lists(g, lists, no);
                let group = &mut self.data.groups[g];
                group.waiting_for_move = false;
                group.early.clear();
                for (_, _, readers) in &mut group.member_devices {
                    // Grants on the old queue do not identify devices on the new one.
                    readers.clear();
                }
                if had_access && !self.group_access(g) {
                    self.lose_group_access(g);
                    return self.persist_unit();
                }
                self.subscribe_group(g);
                self.after_members_left(g, &gone);
                self.flush_group(g);
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
                if self.data.groups[g].main_admin() != Some(from) {
                    return;
                }
                self.group_system(g, "the admin removed you from the group".to_string());
                self.lose_group_access(g);
            }
            _ => {}
        }
        self.persist();
    }
}

impl Engine {
    /// ADR-042: whether every member reads rosters from several admins. We
    /// know it of our contacts; a group whose creator invited everyone has
    /// only those, and once it has helpers, every invite asks for it.
    fn admins_welcome(&self, g: usize) -> bool {
        let me = self.identity_key();
        let group = &self.data.groups[g];
        !group.admins.is_empty()
            || group.members().iter().all(|m| {
                m.ed25519 == me
                    || self
                        .index(&m.ed25519)
                        .is_some_and(|i| self.data.contacts[i].understands >= proto::ADMINS_LEVEL)
            })
    }

    /// ADR-042: we sign a new admin list and roster `no + 1` for group `g`
    /// and tell everyone; `members` replaces the roster's when given.
    /// `true`: the group must move.
    fn publish_admins(
        &mut self,
        g: usize,
        admins: Vec<Ed25519Key>,
        resign: Option<proto::Resign>,
        members: Option<Vec<RosterMember>>,
    ) -> Result<bool, String> {
        let me = self.me();
        let group = &self.data.groups[g];
        let mut roster = group.roster.clone().ok_or("the group is not ready yet")?;
        let old = group.admin_keys();
        let since = roster.no;
        if let Some(members) = members {
            roster.members = members;
        }
        roster.no += 1;
        roster.admin = me;
        roster.signature = self.core().sign_roster(&roster.signed_bytes());
        let mut list = AdminList {
            group: group.id,
            no: roster.no,
            signer: me,
            admins,
            resign,
            signature: [0; 64],
        };
        list.signature = self.core().sign_admins(&list);
        // Our own rules hold for us too: the same check every member makes.
        check_admin_change(&group.id, &old, since, &list).map_err(|_| "not allowed")?;
        self.data.groups[g].set_roster(roster.clone(), &me);
        let moves = self.take_admin_lists(g, vec![list], roster.no);
        let gid = self.data.groups[g].id;
        let chain = self.data.groups[g].admins.clone();
        self.persist();
        self.send_group_body(g, Body::GroupAdmins { group: gid, chain });
        self.send_group_body(g, Body::GroupRoster { roster });
        Ok(moves)
    }

    fn main_check(&self, g: usize) -> Result<(), String> {
        let group = self.data.groups.get(g).ok_or("no such group")?;
        if group.left || group.main_admin() != Some(self.me()) || !group.is_admin() {
            return Err("only the group's main admin does this".into());
        }
        if !self.is_primary() {
            return Err("do this on your main device".into());
        }
        if group.moving.is_some() {
            return Err("the group is moving to a new queue; try again in a moment".into());
        }
        Ok(())
    }

    /// ADR-042: the main admin makes member `who` an admin.
    pub fn make_admin(&mut self, g: usize, who: &Ed25519Key) -> Result<(), String> {
        self.main_check(g)?;
        let group = &self.data.groups[g];
        let mut admins = group.admin_keys();
        if *who == self.me() || !group.members().iter().any(|m| m.ed25519 == *who) {
            return Err("only another member can be made an admin".into());
        }
        if admins.contains(who) {
            return Err("already an admin".into());
        }
        if admins.len() >= proto::MAX_GROUP_ADMINS {
            return Err("a group has at most 4 admins".into());
        }
        if !self.admins_welcome(g) {
            return Err("every member's app must be updated first".into());
        }
        admins.push(*who);
        self.publish_admins(g, admins, None, None)?;
        self.send_admin_keys(g, who);
        Ok(())
    }

    /// ADR-042: the main admin takes the role from helper `who`; the group
    /// moves, so the token they knew opens nothing.
    pub fn remove_admin(&mut self, g: usize, who: &Ed25519Key) -> Result<(), String> {
        self.main_check(g)?;
        let mut admins = self.data.groups[g].admin_keys();
        if *who == self.me() || !admins.contains(who) {
            return Err("not a helper of this group".into());
        }
        admins.retain(|k| k != who);
        if self.publish_admins(g, admins, None, None)? {
            self.move_as_main(g);
        }
        Ok(())
    }

    /// ADR-042: a helper leaves the role; the main admin moves the group.
    pub fn step_down(&mut self, g: usize) -> Result<(), String> {
        let me = self.me();
        let group = self.data.groups.get(g).ok_or("no such group")?;
        let mut admins = group.admin_keys();
        if group.left || admins.first() == Some(&me) || !admins.contains(&me) {
            return Err("only a helper steps down; the main admin hands over".into());
        }
        if !self.is_primary() {
            return Err("do this on your main device".into());
        }
        admins.retain(|k| *k != me);
        self.publish_admins(g, admins, None, None)?;
        Ok(())
    }

    /// ADR-042: the main admin hands the group over to member `who`, and
    /// stays a member or leaves (`leave`). The successor moves the group.
    pub fn hand_over_group(
        &mut self,
        g: usize,
        who: &Ed25519Key,
        leave: bool,
    ) -> Result<(), String> {
        self.main_check(g)?;
        let me = self.me();
        let group = &self.data.groups[g];
        if *who == me || !group.members().iter().any(|m| m.ed25519 == *who) {
            return Err("hand over to another member".into());
        }
        if !self.admins_welcome(g) {
            return Err("every member's app must be updated first".into());
        }
        let old = group.admin_keys();
        let mut admins = vec![*who];
        admins.extend(old.iter().skip(1).filter(|k| *k != who));
        let members = leave.then(|| {
            group
                .members()
                .iter()
                .filter(|m| m.ed25519 != me)
                .cloned()
                .collect()
        });
        // A member who was no helper gets the token before the roster.
        if !old.contains(who) {
            self.send_admin_keys(g, who);
        }
        self.publish_admins(g, admins, None, members)?;
        if leave {
            self.leave_group(g)?;
        }
        Ok(())
    }

    /// ADR-042, section 4: the main admin left with their word; we are next.
    /// shortcut: a helper's roster that lands between the main admin's resign
    /// and goodbye makes the resign stale and nobody takes over; if that is
    /// ever seen, let helpers hold their rosters while a resign is known.
    fn take_over(&mut self, g: usize) {
        let group = &self.data.groups[g];
        let (Some(roster), Some(resign)) = (group.roster.as_ref(), group.resign.clone()) else {
            return;
        };
        let old = group.admin_keys();
        let Some(main) = old.first().copied() else {
            return;
        };
        if resign.roster_no != roster.no {
            return;
        }
        let members = roster
            .members
            .iter()
            .filter(|m| m.ed25519 != main)
            .cloned()
            .collect();
        let admins = old[1..].to_vec();
        self.data.groups[g]
            .member_devices
            .retain(|(m, ..)| *m != main);
        match self.publish_admins(g, admins, Some(resign), Some(members)) {
            Ok(true) => self.move_as_main(g),
            Ok(false) => {}
            Err(e) => self.note(e),
        }
    }

    /// ADR-042: the queue's admin token to admin `who`, from the main admin.
    fn send_admin_keys(&mut self, g: usize, who: &Ed25519Key) {
        let group = &self.data.groups[g];
        let (Some((queue, token)), Some(roster)) = (group.admin, group.roster.as_ref()) else {
            return;
        };
        let body = Body::GroupAdminKeys {
            group: group.id,
            roster_no: roster.no,
            grant: proto::AdminGrant { queue, token },
        };
        self.group_tell_in(g, who, body, None);
    }

    /// `body` to device `who` of group `g`: over a chat if we have one, else
    /// through their reader (ADR-042, amendment 2), at `at` when the roster
    /// no longer names it, as on a move.
    fn group_tell_in(&mut self, g: usize, who: &Ed25519Key, body: Body, at: Option<QueueId>) {
        if self.has_chat(who) {
            return self.group_tell(who, body);
        }
        let Some(send_id) = at.or_else(|| {
            self.data.groups[g]
                .devices()
                .iter()
                .find(|m| m.ed25519 == *who)
                .map(|m| m.reader)
        }) else {
            return;
        };
        self.data.groups[g]
            .reader_outbox
            .push((*who, body, send_id));
        self.persist();
        if self.has_session(who) {
            self.flush_reader_outbox(g, who);
        } else {
            self.group_send(g, ClientFrame::ClaimKey { send_id });
        }
    }

    /// Whether we talk to device `who` over a chat: a contact on our main
    /// identity, or a contact's linked device.
    fn has_chat(&self, who: &Ed25519Key) -> bool {
        self.index(who)
            .is_some_and(|i| self.data.contacts[i].profile.is_none())
            || self.owner_of_device(who).is_some()
    }

    /// What waits for device `who`'s reader, now that we have a session.
    fn flush_reader_outbox(&mut self, g: usize, who: &Ed25519Key) {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.data.groups[g].reader_outbox)
            .into_iter()
            .partition(|(k, ..)| k == who);
        self.data.groups[g].reader_outbox = rest;
        for (who, body, send_id) in mine {
            let Ok(envelope) = self.core().encrypt(&who, body) else {
                continue;
            };
            if !self.persist() {
                return;
            }
            let client_id = self.data.groups[g].next_client_id;
            self.data.groups[g].next_client_id += 1;
            self.group_send(
                g,
                ClientFrame::Send {
                    client_id,
                    send_id,
                    envelope,
                },
            );
        }
        self.persist();
    }

    /// ADR-042, amendment 3: `who` spoke on the new queue; the old one goes
    /// once nobody it was kept for is left.
    fn seen_on_new_queue(&mut self, g: usize, who: &Ed25519Key) {
        let Some(r) = self.data.groups[g].retiring.as_mut() else {
            return;
        };
        r.waiting.retain(|k| k != who);
        if r.waiting.is_empty() {
            self.retire_old_queue(g);
        }
    }

    fn retire_old_queue(&mut self, g: usize) {
        let Some(r) = self.data.groups[g].retiring.take() else {
            return;
        };
        let (group_id, token) = r.old;
        if !self.group_send(g, ClientFrame::DeleteGroup { group_id, token }) {
            self.data.groups[g].retiring = Some(r);
        }
        self.persist();
    }

    /// Old queues whose day is over (ADR-042, amendment 3).
    pub(super) fn retire_old_queues(&mut self) {
        let at = now();
        for g in 0..self.data.groups.len() {
            if self.data.groups[g]
                .retiring
                .as_ref()
                .is_some_and(|r| r.until <= at)
            {
                self.retire_old_queue(g);
            }
        }
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
