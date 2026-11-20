//! Group sessions (ADR-027): Megolm from vodozemac, as Matrix uses it. Each
//! member sends with an outbound session of their own; its key reaches the
//! others over Olm, one to one, and becomes their inbound session for that
//! member. No new cryptography: the caller moves the keys, this module keeps
//! the sessions and checks who wrote what.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use vodozemac::megolm::{
    ExportedSessionKey, GroupSession, GroupSessionPickle, InboundGroupSession,
    InboundGroupSessionPickle, MegolmMessage, SessionConfig, SessionKey,
};
use zeroize::Zeroizing;

use crate::Error;
use crate::proto::{Body, Ed25519Key, GroupCiphertext, GroupId, GroupInner};

/// Sessions kept per member: the newest few, so messages sent just before a
/// rotation still read.
const SESSIONS_PER_MEMBER: usize = 4;
/// ADR-027: a new outbound session after this many messages or this long.
const ROTATE_AFTER_MESSAGES: u32 = 100;
const ROTATE_AFTER_SECS: u64 = 7 * 86_400;
/// Message indexes remembered per inbound session, to refuse replays.
const SEEN_PER_SESSION: usize = 1000;

/// One group's sessions. No `Debug`: holds secret keys.
pub(crate) struct Group {
    outbound: GroupSession,
    /// When `outbound` was made (unix seconds, from the caller).
    made_at: u64,
    sent: u32,
    inbound: Vec<Inbound>,
}

struct Inbound {
    member: Ed25519Key,
    session: InboundGroupSession,
    seen: BTreeSet<u32>,
}

impl Group {
    fn new(now: u64) -> Self {
        Self {
            outbound: GroupSession::new(SessionConfig::version_1()),
            made_at: now,
            sent: 0,
            inbound: Vec::new(),
        }
    }

    /// Our current session key, to share with members over Olm. Only what is
    /// sent from now on reads with it: no history (ADR-027).
    fn key(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.outbound.session_key().to_bytes())
    }

    /// Our session's key from message `index` on: a member gets it from the
    /// message they joined at, so nothing before is theirs (ADR-027).
    fn key_at(&mut self, me: &Ed25519Key, index: u32) -> Option<Zeroizing<Vec<u8>>> {
        let id = self.outbound.session_id();
        let own = self
            .inbound
            .iter_mut()
            .find(|s| &s.member == me && s.session.session_id() == id)?;
        Some(Zeroizing::new(own.session.export_at(index)?.to_bytes()))
    }

    fn add(&mut self, member: Ed25519Key, key: &[u8]) -> Result<(), Error> {
        let malformed = || Error::Proto(crate::proto::Error::Malformed);
        let session = match SessionKey::from_bytes(key) {
            Ok(key) => InboundGroupSession::new(&key, SessionConfig::version_1()),
            Err(_) => {
                let key = ExportedSessionKey::from_bytes(key).map_err(|_| malformed())?;
                InboundGroupSession::import(&key, SessionConfig::version_1())
            }
        };
        let id = session.session_id();
        if self.inbound.iter().any(|s| s.session.session_id() == id) {
            return Ok(());
        }
        self.inbound.push(Inbound {
            member,
            session,
            seen: BTreeSet::new(),
        });
        let theirs = self.inbound.iter().filter(|s| s.member == member).count();
        if theirs > SESSIONS_PER_MEMBER
            && let Some(oldest) = self.inbound.iter().position(|s| s.member == member)
        {
            self.inbound.remove(oldest);
        }
        Ok(())
    }
}

/// Every group of this identity, by its id.
#[derive(Default)]
pub(crate) struct Groups(HashMap<GroupId, Group>);

impl Groups {
    /// A new outbound session for `group` (a new group, or a rotation), our
    /// own key added to read our messages when they come back. Returns the
    /// key to share.
    pub(crate) fn start(
        &mut self,
        group: GroupId,
        me: Ed25519Key,
        now: u64,
    ) -> Result<Zeroizing<Vec<u8>>, Error> {
        let inbound = self.0.remove(&group).map(|g| g.inbound).unwrap_or_default();
        let mut g = Group::new(now);
        g.inbound = inbound;
        let key = g.key();
        g.add(me, &key)?;
        self.0.insert(group, g);
        Ok(key)
    }

    pub(crate) fn key(&self, group: &GroupId) -> Option<Zeroizing<Vec<u8>>> {
        self.0.get(group).map(Group::key)
    }

    /// The index of the next message we send in `group`.
    pub(crate) fn index(&self, group: &GroupId) -> Option<u32> {
        self.0.get(group).map(|g| g.sent)
    }

    pub(crate) fn key_at(
        &mut self,
        group: &GroupId,
        me: &Ed25519Key,
        index: u32,
    ) -> Option<Zeroizing<Vec<u8>>> {
        self.0.get_mut(group)?.key_at(me, index)
    }

    /// Whether our session is due for rotation (ADR-027).
    pub(crate) fn due(&self, group: &GroupId, now: u64) -> bool {
        self.0.get(group).is_some_and(|g| {
            g.sent >= ROTATE_AFTER_MESSAGES || now.saturating_sub(g.made_at) >= ROTATE_AFTER_SECS
        })
    }

    pub(crate) fn add_member_key(
        &mut self,
        group: &GroupId,
        member: Ed25519Key,
        key: &[u8],
    ) -> Result<(), Error> {
        self.0
            .get_mut(group)
            .ok_or(Error::NoSession)?
            .add(member, key)
    }

    /// Forgets a member's sessions: nothing they send reads any more.
    pub(crate) fn forget_member(&mut self, group: &GroupId, member: &Ed25519Key) {
        if let Some(g) = self.0.get_mut(group) {
            g.inbound.retain(|s| &s.member != member);
        }
    }

    pub(crate) fn forget(&mut self, group: &GroupId) {
        self.0.remove(group);
    }

    pub(crate) fn encrypt(
        &mut self,
        group: GroupId,
        me: Ed25519Key,
        body: Body,
    ) -> Result<Vec<u8>, Error> {
        let g = self.0.get_mut(&group).ok_or(Error::NoSession)?;
        let plain = GroupInner {
            sender: me,
            group,
            body,
        }
        .to_padded()?;
        let message = g.outbound.encrypt(&plain);
        g.sent = g.sent.saturating_add(1);
        Ok(GroupCiphertext {
            session_id: g.outbound.session_id(),
            message: message.to_bytes(),
        }
        .encode())
    }

    /// The sender and what they wrote. The sender is the member whose session
    /// it is; a message naming anyone else, another group, or one read
    /// before, is refused.
    pub(crate) fn decrypt(
        &mut self,
        group: &GroupId,
        bytes: &[u8],
    ) -> Result<(Ed25519Key, Body), Error> {
        let wire = GroupCiphertext::decode(bytes)?;
        let g = self.0.get_mut(group).ok_or(Error::NoSession)?;
        let s = g
            .inbound
            .iter_mut()
            .find(|s| s.session.session_id() == wire.session_id)
            .ok_or(Error::UnknownSender)?;
        let message = MegolmMessage::from_bytes(&wire.message).map_err(|_| Error::DecryptFailed)?;
        let plain = s
            .session
            .decrypt(&message)
            .map_err(|_| Error::DecryptFailed)?;
        let plain = Zeroizing::new(plain.plaintext);
        let inner = GroupInner::from_padded(&plain)?;
        if inner.sender != s.member || inner.group != *group {
            return Err(Error::SenderMismatch);
        }
        let index = message.message_index();
        if (s.seen.len() >= SEEN_PER_SESSION
            && s.seen.first().is_some_and(|oldest| index < *oldest))
            || !s.seen.insert(index)
        {
            return Err(Error::Duplicate);
        }
        if s.seen.len() > SEEN_PER_SESSION {
            s.seen.pop_first();
        }
        Ok((inner.sender, inner.body))
    }

    /// ADR-030, section 6: group messages we never got, over every member's
    /// session, as a chat counts its `seq`: indexes skipped between the first
    /// one we can read and the newest we read. Messages at the end are not
    /// seen as missing until a later one comes.
    pub(crate) fn missed(&self, group: &GroupId) -> u64 {
        let Some(g) = self.0.get(group) else {
            return 0;
        };
        g.inbound
            .iter()
            .map(|s| {
                let Some(&newest) = s.seen.last() else {
                    return 0;
                };
                // Past the window only the window counts.
                let from = match s.seen.len() >= SEEN_PER_SESSION {
                    true => s.seen.first().copied().unwrap_or(newest),
                    false => s.session.first_known_index(),
                };
                let span = u64::from(newest.saturating_sub(from)) + 1;
                span.saturating_sub(s.seen.len() as u64)
            })
            .sum()
    }

    pub(crate) fn pickle(&self) -> Vec<GroupState> {
        self.0
            .iter()
            .map(|(id, g)| GroupState {
                id: *id,
                outbound: g.outbound.pickle(),
                made_at: g.made_at,
                sent: g.sent,
                inbound: g
                    .inbound
                    .iter()
                    .map(|s| (s.member, s.session.pickle(), s.seen.clone()))
                    .collect(),
            })
            .collect()
    }

    pub(crate) fn from_pickle(state: Vec<GroupState>) -> Self {
        Self(
            state
                .into_iter()
                .map(|g| {
                    let inbound = g
                        .inbound
                        .into_iter()
                        .map(|(member, p, seen)| Inbound {
                            member,
                            session: InboundGroupSession::from_pickle(p),
                            seen,
                        })
                        .collect();
                    let group = Group {
                        outbound: GroupSession::from_pickle(g.outbound),
                        made_at: g.made_at,
                        sent: g.sent,
                        inbound,
                    };
                    (g.id, group)
                })
                .collect(),
        )
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct GroupState {
    id: GroupId,
    outbound: GroupSessionPickle,
    made_at: u64,
    sent: u32,
    inbound: Vec<(Ed25519Key, InboundGroupSessionPickle, BTreeSet<u32>)>,
}
