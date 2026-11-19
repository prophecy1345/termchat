//! Sans-IO core: E2EE sessions (vodozemac), protocol state, encrypted storage format.
//!
//! No networking, no files, no clock: callers pass bytes in and get bytes out.
//! After every call that takes `&mut self`, the caller must persist [`Client::save`]:
//! the Olm ratchet has moved and an old state breaks sessions.

mod fingerprint;
mod group;
pub mod store;

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use vodozemac::olm::{Account, AccountPickle, OlmMessage, Session, SessionConfig, SessionPickle};
use vodozemac::{Curve25519PublicKey, Ed25519PublicKey, Ed25519Signature};
use zeroize::Zeroizing;

pub use fingerprint::{fingerprint, safety_number};
pub use store::{FileKey, KdfParams, create as create_file_key, opens_duress, rekey, set_duress};
pub use termchat_proto as proto;

use proto::{
    Body, Curve25519Key, DeviceList, Ed25519Key, Envelope, Identity, Inner, Signature, SignedKey,
};

/// ADR-007: unanswered contact requests kept at once.
const MAX_REQUESTS: usize = 50;
/// ADR-002: old sessions kept per contact.
const MAX_SESSIONS: usize = 5;
/// protocol.md, section 7.
const MAX_MISSING: usize = 1000;
/// ADR-019: remembered inbound session ids, per sender and in all.
const MAX_SEEN_PER_SENDER: usize = 200;
const MAX_SEEN: usize = 10_000;
const ID_CONTEXT: &[u8] = b"termchat-id-v1";
const OTK_CONTEXT: &[u8] = b"termchat-otk-v1";
/// ADR-027: what a group admin signs.
const ROSTER_CONTEXT: &[u8] = b"termchat-roster-v1";
const DEVICES_CONTEXT: &[u8] = b"termchat-devices-v1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Proto(#[from] proto::Error),
    #[error("message is not addressed to us")]
    WrongRecipient,
    #[error("sender does not match the session")]
    SenderMismatch,
    #[error("unknown sender")]
    UnknownSender,
    #[error("no session with this contact")]
    NoSession,
    #[error("bad signature")]
    BadSignature,
    #[error("olm session error")]
    Olm,
    #[error("decryption failed")]
    DecryptFailed,
    #[error("duplicate message")]
    Duplicate,
    #[error("wrong password")]
    WrongPassword,
    /// The duress password was entered: the caller must wipe (ADR-003).
    #[error("wrong password")]
    Duress,
    #[error("corrupt state")]
    CorruptState,
    /// ADR-007: too many unanswered contact requests; this one was not stored.
    #[error("too many pending contact requests")]
    TooManyRequests,
    #[error("random number generator failed")]
    Rng,
}

/// Keys to upload to the relay.
#[derive(Debug, Clone)]
pub struct Prekeys {
    pub one_time: Vec<SignedKey>,
    pub fallback: Option<SignedKey>,
}

/// A successfully decrypted message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// Who wrote it: the contact of the queue, or, from the invite queue, the
    /// key Olm and the contact request vouch for.
    pub sender: Ed25519Key,
    pub body: Body,
    /// Messages known to be missing in this session (dropped or not yet arrived).
    pub missed: u64,
    /// This message started a new session (first contact or the peer reset its state).
    pub new_session: bool,
    /// The sender is a contact request not accepted yet: the app keeps such
    /// messages out of sight until it is (ADR-007).
    pub request: bool,
}

/// No `Debug`: holds secret keys.
#[derive(Default)]
pub struct Client {
    account: Account,
    contacts: HashMap<Ed25519Key, Contact>,
    /// Inbound sessions already built, oldest first: a fallback key stays in
    /// the account, so the same pre-key message could build one again and be
    /// read twice (ADR-019).
    seen: Vec<Seen>,
    /// How many fallback keys there have been; the current one's number.
    fallback_no: u64,
    /// Megolm sessions of our groups (ADR-027).
    groups: group::Groups,
}

#[derive(Clone, Serialize, Deserialize)]
struct Seen {
    id: String,
    /// The Curve25519 key that built the session (ADR-019); entries saved
    /// before 0.6.2 hold the relay-reported Ed25519 key instead.
    sender: Curve25519Key,
    /// The fallback key that was current when the session was built.
    fallback: u64,
}

struct Contact {
    curve25519: Curve25519Key,
    /// Wrote to us first; not accepted yet.
    is_request: bool,
    /// What they call themselves; from `ContactRequest` / `ContactAccept`.
    nickname: Option<String>,
    /// Newest (active) last.
    sessions: Vec<SessionState>,
    /// ADR-030: known only by someone else's word (a roster, a device list),
    /// not by a signed `Identity`. Never counts on an invite queue.
    member: bool,
}

struct SessionState {
    session: Session,
    sent: u64,
    received: SeqTracker,
}

impl Client {
    pub fn identity(&self) -> Identity {
        let curve25519 = self.account.curve25519_key().to_bytes();
        Identity {
            ed25519: self.ed25519(),
            curve25519,
            signature: self
                .account
                .sign([ID_CONTEXT, &curve25519].concat())
                .to_bytes(),
        }
    }

    fn ed25519(&self) -> Ed25519Key {
        *self.account.ed25519_key().as_bytes()
    }

    /// Tops up one-time keys when the relay holds fewer than half (ADR-002) and
    /// optionally rotates the fallback key. Returns everything not yet published.
    pub fn prekeys(&mut self, on_server: usize, rotate_fallback: bool) -> Prekeys {
        let max = self.account.max_number_of_one_time_keys();
        if on_server < max / 2 {
            self.account.generate_one_time_keys(max - on_server);
        }
        if rotate_fallback {
            self.account.generate_fallback_key();
            self.fallback_no += 1;
        }
        let prekeys = Prekeys {
            one_time: self.unpublished(),
            fallback: self
                .account
                .fallback_key()
                .into_values()
                .next()
                .map(|k| self.signed(k)),
        };
        self.account.mark_keys_as_published();
        prekeys
    }

    /// `n` new one-time keys for one contact's queue (ADR-023, amendment):
    /// never shared with another queue, so they cannot link queues.
    pub fn one_time_keys(&mut self, n: usize) -> Vec<SignedKey> {
        self.account.generate_one_time_keys(n);
        let keys = self.unpublished();
        self.account.mark_keys_as_published();
        keys
    }

    fn unpublished(&self) -> Vec<SignedKey> {
        self.account
            .one_time_keys()
            .into_values()
            .map(|k| self.signed(k))
            .collect()
    }

    fn signed(&self, k: Curve25519PublicKey) -> SignedKey {
        let key = k.to_bytes();
        SignedKey {
            key,
            signature: self.account.sign([OTK_CONTEXT, &key].concat()).to_bytes(),
        }
    }

    /// Adds a contact from an invite. Also accepts a pending contact request.
    pub fn add_contact(&mut self, identity: &Identity) -> Result<(), Error> {
        verify_identity(identity)?;
        let contact = self
            .contacts
            .entry(identity.ed25519)
            .or_insert_with(|| Contact {
                curve25519: identity.curve25519,
                is_request: false,
                nickname: None,
                sessions: Vec::new(),
                member: false,
            });
        // ADR-030: a signed identity replaces what someone else said of the key.
        if contact.member {
            if contact.curve25519 != identity.curve25519 {
                contact.curve25519 = identity.curve25519;
                contact.sessions.clear();
            }
            contact.member = false;
        }
        if contact.curve25519 != identity.curve25519 {
            return Err(Error::SenderMismatch);
        }
        contact.is_request = false;
        Ok(())
    }

    /// `None` for unknown keys.
    pub fn is_request(&self, contact: &Ed25519Key) -> Option<bool> {
        self.contacts.get(contact).map(|c| c.is_request)
    }

    pub fn nickname(&self, contact: &Ed25519Key) -> Option<&str> {
        self.contacts.get(contact)?.nickname.as_deref()
    }

    /// Starts an outbound session with a one-time (or fallback) key claimed from the relay.
    pub fn start_session(&mut self, contact: &Ed25519Key, key: &SignedKey) -> Result<(), Error> {
        let c = self.contacts.get_mut(contact).ok_or(Error::UnknownSender)?;
        verify(contact, OTK_CONTEXT, &key.key, &key.signature)?;
        let session = self
            .account
            .create_outbound_session(
                SessionConfig::version_1(),
                Curve25519PublicKey::from_bytes(c.curve25519),
                Curve25519PublicKey::from_bytes(key.key),
            )
            .map_err(|_| Error::Olm)?;
        push_session(
            c,
            SessionState {
                session,
                sent: 0,
                received: SeqTracker::default(),
            },
        );
        Ok(())
    }

    pub fn encrypt(&mut self, to: &Ed25519Key, body: Body) -> Result<Envelope, Error> {
        let sender = self.ed25519();
        let s = self
            .contacts
            .get_mut(to)
            .and_then(|c| c.sessions.last_mut())
            .ok_or(Error::NoSession)?;
        let seq = s.sent + 1;
        let padded = Inner {
            sender,
            recipient: *to,
            seq,
            body,
        }
        .to_padded()?;
        let (olm_type, body) = s
            .session
            .encrypt(&padded)
            .map_err(|_| Error::Olm)?
            .to_parts();
        s.sent = seq;
        Ok(Envelope {
            version: proto::VERSION,
            olm_type: u8::try_from(olm_type).map_err(|_| Error::Olm)?,
            body,
        })
    }

    /// An Olm envelope on a group reader (ADR-027): only from someone we know,
    /// member or peer, by the Curve25519 key Olm vouches for. Never a request
    /// from a stranger (ADR-030; review 2026-11c, finding 8).
    pub fn decrypt_on_reader(&mut self, env: &Envelope) -> Result<Received, Error> {
        let msg = OlmMessage::from_parts(env.olm_type.into(), &env.body)
            .map_err(|_| proto::Error::Malformed)?;
        let OlmMessage::PreKey(pre) = &msg else {
            return Err(Error::UnknownSender);
        };
        let curve = pre.identity_key().to_bytes();
        let sender = self
            .contacts
            .iter()
            .find(|(_, c)| c.curve25519 == curve)
            .map(|(k, _)| *k)
            .ok_or(Error::UnknownSender)?;
        self.decrypt(Some(&sender), env)
    }

    /// `from`: the contact whose queue the envelope came in (ADR-023). The
    /// relay picks the queue, so this is checked against the session and the
    /// plaintext like any claim of the relay.
    ///
    /// `None`: the invite queue, where anyone may write. Only pre-key messages
    /// are read there: everyone writes to it before their first answer. The
    /// sender is the contact with the Curve25519 key Olm vouches for, or a
    /// stranger whose contact request binds its key to that one.
    pub fn decrypt(
        &mut self,
        from: Option<&Ed25519Key>,
        env: &Envelope,
    ) -> Result<Received, Error> {
        let me = self.ed25519();
        let msg = OlmMessage::from_parts(env.olm_type.into(), &env.body)
            .map_err(|_| proto::Error::Malformed)?;
        let known = match (from, &msg) {
            (Some(k), _) => Some(*k),
            (None, OlmMessage::PreKey(pre)) => {
                let curve = pre.identity_key().to_bytes();
                // ADR-030: only peers; a member's key is someone else's word.
                self.contacts
                    .iter()
                    .find(|(_, c)| c.curve25519 == curve && !c.member)
                    .map(|(k, _)| *k)
            }
            (None, OlmMessage::Normal(_)) => return Err(Error::UnknownSender),
        };

        // Existing session, newest first.
        if let Some(sender) = known
            && let Some(c) = self.contacts.get_mut(&sender)
        {
            for i in (0..c.sessions.len()).rev() {
                let s = &mut c.sessions[i];
                let own_prekey = match &msg {
                    OlmMessage::PreKey(m) if m.session_id() != s.session.session_id() => continue,
                    OlmMessage::PreKey(_) => true,
                    OlmMessage::Normal(_) => false,
                };
                let Ok(plain) = s.session.decrypt(&msg) else {
                    // ADR-019: this session's own pre-key message, read
                    // before. It must not go on to build the session again.
                    if own_prekey {
                        return Err(Error::DecryptFailed);
                    }
                    continue;
                };
                let inner = open_inner(&plain, &sender, &me)?;
                s.received.accept(inner.seq)?;
                let missed = s.received.missed();
                remember_nickname(c, &inner.body);
                // The session the peer uses becomes our active one.
                let s = c.sessions.remove(i);
                c.sessions.push(s);
                return Ok(Received {
                    sender,
                    body: inner.body,
                    missed,
                    new_session: false,
                    request: c.is_request,
                });
            }
        }

        // New inbound session.
        let OlmMessage::PreKey(pre) = &msg else {
            return Err(
                match known.is_some_and(|k| self.contacts.contains_key(&k)) {
                    true => Error::DecryptFailed,
                    false => Error::UnknownSender,
                },
            );
        };
        let their_curve = pre.identity_key();
        if let Some(k) = known
            && let Some(c) = self.contacts.get(&k)
            && c.curve25519 != their_curve.to_bytes()
        {
            return Err(Error::SenderMismatch);
        }
        // ADR-019: a session that is gone (pushed out, or its contact removed)
        // is not built again either.
        let id = pre.session_id();
        if self.seen.iter().any(|s| s.id == id) {
            return Err(Error::DecryptFailed);
        }
        let created = self
            .account
            .create_inbound_session(SessionConfig::version_1(), their_curve, pre)
            .map_err(|_| Error::DecryptFailed)?;
        // At once, whatever the checks below say: refused once, refused again.
        // Counted under the key Olm vouches for, not the one the relay names.
        self.remember(id, their_curve.to_bytes());
        let sender = match known {
            Some(k) => k,
            None => {
                // From the invite queue, by someone with no contact under this
                // Curve25519 key: it may not name a contact we have.
                let named = Inner::from_padded(&created.plaintext)?.sender;
                if self.contacts.get(&named).is_some_and(|c| !c.member) {
                    return Err(Error::SenderMismatch);
                }
                named
            }
        };
        let inner = open_inner(&created.plaintext, &sender, &me)?;

        // Unknown, or on the invite queue known only by someone else's word.
        let stranger = match self.contacts.get(&sender) {
            None => true,
            Some(c) => c.member && known.is_none(),
        };
        if stranger {
            // A stranger may only send a contact request whose identity binds
            // its key to the Curve25519 key of this session.
            // Or a primary answering a new device's link code (ADR-028).
            let (Body::ContactRequest { identity, .. }
            | Body::ContactRequestV2 { identity, .. }
            | Body::DeviceLink { identity, .. }) = &inner.body
            else {
                return Err(Error::UnknownSender);
            };
            if identity.ed25519 != sender || identity.curve25519 != their_curve.to_bytes() {
                return Err(Error::SenderMismatch);
            }
            verify_identity(identity)?;
            // Making keys is cheap; every request is a stored session.
            if self.contacts.values().filter(|c| c.is_request).count() >= MAX_REQUESTS {
                return Err(Error::TooManyRequests);
            }
            self.contacts.insert(
                sender,
                Contact {
                    curve25519: identity.curve25519,
                    is_request: true,
                    nickname: None,
                    sessions: Vec::new(),
                    member: false,
                },
            );
        }

        let mut s = SessionState {
            session: created.session,
            sent: 0,
            received: SeqTracker::default(),
        };
        s.received.accept(inner.seq)?;
        let missed = s.received.missed();
        let mut request = false;
        if let Some(c) = self.contacts.get_mut(&sender) {
            remember_nickname(c, &inner.body);
            push_session(c, s);
            request = c.is_request;
        }
        Ok(Received {
            sender,
            body: inner.body,
            missed,
            new_session: true,
            request,
        })
    }

    fn remember(&mut self, id: String, sender: Curve25519Key) {
        let of_sender = |s: &&Seen| s.sender == sender;
        if self.seen.iter().filter(of_sender).count() >= MAX_SEEN_PER_SENDER
            && let Some(oldest) = self.seen.iter().position(|s| s.sender == sender)
        {
            self.seen.remove(oldest);
        }
        if self.seen.len() >= MAX_SEEN {
            self.seen.remove(0);
        }
        self.seen.push(Seen {
            id,
            sender,
            fallback: self.fallback_no,
        });
    }

    /// Encrypted state file bytes, with the app's own data (settings, history)
    /// stored alongside. The caller writes them to disk atomically.
    pub fn save(&self, key: &FileKey, app: &str) -> Result<Vec<u8>, Error> {
        store::seal(key, &self.state_json(app)?)
    }

    /// The crypto state alone, unencrypted: an extra identity kept inside the
    /// app data of the main one, which is sealed with it (ADR-014).
    pub fn export(&self) -> Result<Zeroizing<String>, Error> {
        json_string(self.state_json("")?)
    }

    pub fn import(json: &str) -> Result<Self, Error> {
        let state: State = serde_json::from_str(json).map_err(|_| Error::CorruptState)?;
        Ok(Self::from_state(state).0)
    }

    fn state_json(&self, app: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
        let state = StateRef {
            app,
            account: self.account.pickle(),
            contacts: self
                .contacts
                .iter()
                .map(|(key, c)| {
                    let sessions = c
                        .sessions
                        .iter()
                        .map(|s| (s.session.pickle(), s.sent, s.received.clone()))
                        .collect();
                    ContactState {
                        key: *key,
                        curve25519: c.curve25519,
                        is_request: c.is_request,
                        nickname: c.nickname.clone(),
                        sessions,
                        member: c.member,
                    }
                })
                .collect(),
            seen: &self.seen,
            fallback_no: self.fallback_no,
            groups: self.groups.pickle(),
        };
        to_json(&state)
    }

    /// Returns the client, the key to save it again, and the app data.
    pub fn load(password: &[u8], file: &[u8]) -> Result<(Self, FileKey, Zeroizing<String>), Error> {
        let (key, plain) = store::open(password, file)?;
        let state: State = serde_json::from_slice(&plain).map_err(|_| Error::CorruptState)?;
        let (client, app) = Self::from_state(state);
        Ok((client, key, app))
    }

    fn from_state(state: State) -> (Self, Zeroizing<String>) {
        let contacts = state
            .contacts
            .into_iter()
            .map(|c| {
                let sessions = c
                    .sessions
                    .into_iter()
                    .map(|(p, sent, received)| SessionState {
                        session: Session::from_pickle(p),
                        sent,
                        received,
                    })
                    .collect();
                (
                    c.key,
                    Contact {
                        curve25519: c.curve25519,
                        is_request: c.is_request,
                        nickname: c.nickname,
                        sessions,
                        member: c.member,
                    },
                )
            })
            .collect();
        let client = Self {
            account: Account::from_pickle(state.account),
            contacts,
            seen: state.seen,
            fallback_no: state.fallback_no,
            groups: group::Groups::from_pickle(state.groups),
        };
        (client, Zeroizing::new(state.app))
    }

    /// Accepts a pending contact request. `false` if there is none.
    pub fn accept(&mut self, contact: &Ed25519Key) -> bool {
        match self.contacts.get_mut(contact) {
            Some(c) if c.is_request => {
                c.is_request = false;
                true
            }
            _ => false,
        }
    }

    /// Identity and contacts without sessions, for a backup. Restoring it
    /// starts fresh sessions: peers see "restarted the conversation".
    pub fn without_sessions(&self) -> Self {
        let contacts = self
            .contacts
            .iter()
            .map(|(key, c)| {
                let contact = Contact {
                    curve25519: c.curve25519,
                    is_request: c.is_request,
                    nickname: c.nickname.clone(),
                    sessions: Vec::new(),
                    member: c.member,
                };
                (*key, contact)
            })
            .collect();
        Self {
            account: Account::from_pickle(self.account.pickle()),
            contacts,
            // The backup holds the fallback keys, so it holds what guards them.
            seen: self.seen.clone(),
            fallback_no: self.fallback_no,
            // Group sessions start again too: members share keys anew.
            groups: group::Groups::default(),
        }
    }

    /// Drops a contact's sessions, keeping the contact and its name: the
    /// next message starts a new one (ADR-023, contacts asked to add again).
    pub fn forget_sessions(&mut self, contact: &Ed25519Key) {
        if let Some(c) = self.contacts.get_mut(contact) {
            c.sessions.clear();
        }
    }

    /// Forgets a contact and its sessions.
    pub fn remove_contact(&mut self, contact: &Ed25519Key) {
        self.contacts.remove(contact);
    }

    /// Drops the fallback key that was rotated out (ADR-002). Call once envelopes
    /// sent to it can no longer be in the relay queue.
    pub fn forget_old_fallback(&mut self) -> bool {
        // Sessions from before the current key used keys that are gone now;
        // nothing can build them again (ADR-019).
        self.seen.retain(|s| s.fallback >= self.fallback_no);
        self.account.forget_fallback_key()
    }

    /// A new outbound Megolm session for `group`, for a new group or a
    /// rotation (ADR-027). Returns its key, to send each member over Olm.
    pub fn group_start(
        &mut self,
        group: proto::GroupId,
        now: u64,
    ) -> Result<Zeroizing<Vec<u8>>, Error> {
        let me = self.ed25519();
        self.groups.start(group, me, now)
    }

    /// Our current session key for `group`, for a member who just joined.
    pub fn group_key(&self, group: &proto::GroupId) -> Option<Zeroizing<Vec<u8>>> {
        self.groups.key(group)
    }

    /// The index of the next message we send in `group`: a member who joins
    /// now gets our key from here.
    pub fn group_index(&self, group: &proto::GroupId) -> Option<u32> {
        self.groups.index(group)
    }

    /// Our current session's key from message `index` on.
    pub fn group_key_at(
        &mut self,
        group: &proto::GroupId,
        index: u32,
    ) -> Option<Zeroizing<Vec<u8>>> {
        let me = self.ed25519();
        self.groups.key_at(group, &me, index)
    }

    /// Whether our session for `group` should be replaced (ADR-027).
    pub fn group_due(&self, group: &proto::GroupId, now: u64) -> bool {
        self.groups.due(group, now)
    }

    /// A member's session key, as it came over Olm from that member.
    pub fn group_member_key(
        &mut self,
        group: &proto::GroupId,
        member: Ed25519Key,
        key: &[u8],
    ) -> Result<(), Error> {
        self.groups.add_member_key(group, member, key)
    }

    pub fn group_forget_member(&mut self, group: &proto::GroupId, member: &Ed25519Key) {
        self.groups.forget_member(group, member);
    }

    pub fn group_forget(&mut self, group: &proto::GroupId) {
        self.groups.forget(group);
    }

    /// `body` for everyone in `group`: the bytes go into an envelope with
    /// `OLM_TYPE_MEGOLM`.
    pub fn group_encrypt(&mut self, group: proto::GroupId, body: Body) -> Result<Vec<u8>, Error> {
        let me = self.ed25519();
        self.groups.encrypt(group, me, body)
    }

    /// Who wrote it and what, for a group envelope's bytes.
    pub fn group_decrypt(
        &mut self,
        group: &proto::GroupId,
        bytes: &[u8],
    ) -> Result<(Ed25519Key, Body), Error> {
        self.groups.decrypt(group, bytes)
    }

    /// A group member, known by the keys the admin signed for (ADR-027), or
    /// a device of a contact or of our own, known by its owner's signed device
    /// list (ADR-028): not by an invite; kept apart from contacts by the app.
    pub fn add_member(
        &mut self,
        ed25519: Ed25519Key,
        curve25519: Curve25519Key,
    ) -> Result<(), Error> {
        let c = self.contacts.entry(ed25519).or_insert(Contact {
            curve25519,
            is_request: false,
            nickname: None,
            sessions: Vec::new(),
            member: true,
        });
        // ADR-030: what someone else says never changes a key we hold.
        match c.curve25519 == curve25519 {
            true => Ok(()),
            false => Err(Error::SenderMismatch),
        }
    }

    /// A contact our own primary handed over (ADR-028): its word, over our
    /// own Olm channel, stands for the contact's signature.
    pub fn add_from_primary(&mut self, ed25519: Ed25519Key, curve25519: Curve25519Key) {
        let c = self.contacts.entry(ed25519).or_insert(Contact {
            curve25519,
            is_request: false,
            nickname: None,
            sessions: Vec::new(),
            member: false,
        });
        if c.member {
            if c.curve25519 != curve25519 {
                c.curve25519 = curve25519;
                c.sessions.clear();
            }
            c.member = false;
        }
    }

    /// ADR-030, for states saved before it: every key but `peers` was known
    /// only by someone else's word.
    pub fn mark_members(&mut self, peers: impl Fn(&Ed25519Key) -> bool) {
        for (k, c) in &mut self.contacts {
            if !peers(k) {
                c.member = true;
            }
        }
    }

    /// A contact's nickname, as our primary knows it (ADR-028).
    pub fn set_nickname(&mut self, contact: &Ed25519Key, nickname: String) {
        if let Some(c) = self.contacts.get_mut(contact) {
            c.nickname = Some(nickname);
        }
    }

    /// A contact's Curve25519 key, for a roster.
    pub fn curve_of(&self, contact: &Ed25519Key) -> Option<Curve25519Key> {
        self.contacts.get(contact).map(|c| c.curve25519)
    }

    /// Our signature over a group's roster (ADR-027).
    pub fn sign_roster(&self, roster: &[u8]) -> Signature {
        self.account
            .sign([ROSTER_CONTEXT, roster].concat())
            .to_bytes()
    }

    /// Signs our device list (ADR-028). Only the primary can: its key is
    /// the identity the list is for.
    pub fn sign_devices(&self, list: &DeviceList) -> Result<Signature, Error> {
        if list.identity != self.ed25519() {
            return Err(Error::BadSignature);
        }
        list.check()?;
        Ok(self
            .account
            .sign([DEVICES_CONTEXT, &list.signed_bytes()].concat())
            .to_bytes())
    }

    pub fn has_session(&self, contact: &Ed25519Key) -> bool {
        self.contacts
            .get(contact)
            .is_some_and(|c| !c.sessions.is_empty())
    }
}

#[derive(Serialize, Deserialize)]
struct ContactState {
    key: Ed25519Key,
    curve25519: Curve25519Key,
    is_request: bool,
    nickname: Option<String>,
    sessions: Vec<(SessionPickle, u64, SeqTracker)>,
    #[serde(default)]
    member: bool,
}

/// Written form of [`State`]: borrows the app data so the plaintext history
/// is not copied once more on every save.
#[derive(Serialize)]
struct StateRef<'a> {
    app: &'a str,
    account: AccountPickle,
    contacts: Vec<ContactState>,
    seen: &'a [Seen],
    fallback_no: u64,
    groups: Vec<group::GroupState>,
}

#[derive(Deserialize)]
struct State {
    app: String,
    account: AccountPickle,
    contacts: Vec<ContactState>,
    /// Absent in files from before ADR-019.
    #[serde(default)]
    seen: Vec<Seen>,
    #[serde(default)]
    fallback_no: u64,
    /// Absent before ADR-027.
    #[serde(default)]
    groups: Vec<group::GroupState>,
}

/// JSON of `value` in a buffer that is wiped whenever it is outgrown, and when
/// dropped. `serde_json::to_vec` grows a plain `Vec`, which leaves shorter
/// copies of the keys and the history behind in freed memory.
pub fn to_json<T: Serialize>(value: &T) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut out = Wiped(Zeroizing::new(Vec::with_capacity(4096)));
    serde_json::to_writer(&mut out, value).map_err(|_| Error::CorruptState)?;
    Ok(out.0)
}

/// The same bytes as text, without another copy.
pub fn json_string(mut json: Zeroizing<Vec<u8>>) -> Result<Zeroizing<String>, Error> {
    String::from_utf8(std::mem::take(&mut *json))
        .map(Zeroizing::new)
        .map_err(|e| {
            Zeroizing::new(e.into_bytes());
            Error::CorruptState
        })
}

/// A byte buffer in memory, nothing more: no file, no network (rule 11).
struct Wiped(Zeroizing<Vec<u8>>);

impl std::io::Write for Wiped {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let need = self.0.len() + bytes.len();
        if need > self.0.capacity() {
            let mut bigger = Zeroizing::new(Vec::with_capacity(need.max(self.0.capacity() * 2)));
            bigger.extend_from_slice(&self.0);
            // The outgrown buffer is wiped here, as it is dropped.
            self.0 = bigger;
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Detects dropped and duplicated messages (protocol.md, section 7).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SeqTracker {
    last: u64,
    missing: BTreeSet<u64>,
    /// Missing numbers that no longer fit in `missing`.
    lost: u64,
}

impl SeqTracker {
    fn accept(&mut self, seq: u64) -> Result<(), Error> {
        if seq > self.last {
            let from = (self.last + 1).max(seq.saturating_sub(MAX_MISSING as u64));
            self.lost += from - (self.last + 1);
            self.missing.extend(from..seq);
            while self.missing.len() > MAX_MISSING {
                self.missing.pop_first();
                self.lost += 1;
            }
            self.last = seq;
            Ok(())
        } else if self.missing.remove(&seq) {
            Ok(())
        } else {
            Err(Error::Duplicate)
        }
    }

    fn missed(&self) -> u64 {
        self.missing.len() as u64 + self.lost
    }
}

/// ADR-007: a name arrives once. A pending request may correct it with a new
/// request; after that only the local petname changes, so a contact cannot
/// silently rename itself into someone else.
fn remember_nickname(c: &mut Contact, body: &Body) {
    let nickname = match body {
        Body::ContactRequest { nickname, .. } | Body::ContactRequestV2 { nickname, .. }
            if c.is_request || c.nickname.is_none() =>
        {
            nickname
        }
        Body::ContactAccept { nickname } if c.nickname.is_none() => nickname,
        _ => return,
    };
    c.nickname = Some(nickname.clone());
}

fn push_session(c: &mut Contact, s: SessionState) {
    c.sessions.push(s);
    if c.sessions.len() > MAX_SESSIONS {
        c.sessions.remove(0);
    }
}

fn open_inner(plain: &[u8], sender: &Ed25519Key, me: &Ed25519Key) -> Result<Inner, Error> {
    let inner = Inner::from_padded(plain)?;
    if inner.recipient != *me {
        return Err(Error::WrongRecipient);
    }
    if inner.sender != *sender {
        return Err(Error::SenderMismatch);
    }
    Ok(inner)
}

/// Whether `admin` signed `roster` (ADR-027).
pub fn verify_roster(admin: &Ed25519Key, roster: &[u8], sig: &Signature) -> Result<(), Error> {
    verify_message(admin, &[ROSTER_CONTEXT, roster].concat(), sig)
}

/// Whether a device list is well formed and signed by its identity (ADR-028).
pub fn verify_devices(list: &DeviceList) -> Result<(), Error> {
    list.check()?;
    verify_message(
        &list.identity,
        &[DEVICES_CONTEXT, &list.signed_bytes()].concat(),
        &list.signature,
    )
}

fn verify_identity(id: &Identity) -> Result<(), Error> {
    verify(&id.ed25519, ID_CONTEXT, &id.curve25519, &id.signature)
}

fn verify(
    signer: &Ed25519Key,
    context: &[u8],
    data: &[u8; 32],
    sig: &Signature,
) -> Result<(), Error> {
    verify_message(signer, &[context, data].concat(), sig)
}

fn verify_message(signer: &Ed25519Key, msg: &[u8], sig: &Signature) -> Result<(), Error> {
    let key = Ed25519PublicKey::from_slice(signer).map_err(|_| Error::BadSignature)?;
    let sig = Ed25519Signature::from_slice(sig).map_err(|_| Error::BadSignature)?;
    key.verify(msg, &sig).map_err(|_| Error::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiped_json_equals_serde_json_across_many_growths() {
        let value: Vec<String> = (0..5000).map(|n| format!("line {n} \"quoted\"")).collect();
        let json = to_json(&value).unwrap();
        assert!(
            json.len() > 16 * 4096,
            "outgrew the first buffer many times"
        );
        assert_eq!(*json, serde_json::to_vec(&value).unwrap());
        assert_eq!(
            json_string(json).unwrap().as_str(),
            serde_json::to_string(&value).unwrap()
        );
    }

    #[test]
    fn seen_sessions_go_with_their_fallback_key_and_are_capped() {
        let ids = |c: &Client| c.seen.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        let mut c = Client::default();
        c.prekeys(0, true);
        c.remember("a".into(), [1; 32]);
        c.prekeys(0, true);
        c.remember("b".into(), [1; 32]);
        // Key 1 is forgotten: "a" came before key 2 and could only have used
        // a key that is gone; "b" may have used key 2.
        c.forget_old_fallback();
        assert_eq!(ids(&c), ["b"]);

        // One sender's 201st session pushes out its own oldest, nobody else's.
        let mut c = Client::default();
        c.remember("other".into(), [2; 32]);
        for n in 0..=MAX_SEEN_PER_SENDER {
            c.remember(n.to_string(), [1; 32]);
        }
        assert_eq!(c.seen.len(), MAX_SEEN_PER_SENDER + 1);
        assert_eq!(ids(&c)[..2], ["other", "1"]);

        // The list as a whole is capped too, oldest first.
        for n in 0..MAX_SEEN {
            let mut sender = [3; 32];
            sender[..8].copy_from_slice(&(n as u64).to_le_bytes());
            c.remember(format!("s{n}"), sender);
        }
        assert_eq!(c.seen.len(), MAX_SEEN);
        assert_eq!(c.seen[0].id, "s0");
    }

    #[test]
    fn seq_tracker_gaps_duplicates_and_cap() {
        let mut t = SeqTracker::default();
        t.accept(1).unwrap();
        t.accept(4).unwrap();
        assert_eq!(t.missed(), 2);
        t.accept(3).unwrap();
        assert_eq!(t.missed(), 1);
        assert_eq!(t.accept(3), Err(Error::Duplicate));
        assert_eq!(t.accept(0), Err(Error::Duplicate));
        t.accept(u64::MAX).unwrap();
        assert_eq!(t.missing.len(), MAX_MISSING);
        assert_eq!(t.missed(), u64::MAX - 4);
    }
}
