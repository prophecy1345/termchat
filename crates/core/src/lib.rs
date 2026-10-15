//! Sans-IO core: E2EE sessions (vodozemac), protocol state, encrypted storage format.
//!
//! No networking, no files, no clock: callers pass bytes in and get bytes out.
//! After every call that takes `&mut self`, the caller must persist [`Client::save`]:
//! the Olm ratchet has moved and an old state breaks sessions.

mod fingerprint;
pub mod store;

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use vodozemac::olm::{Account, AccountPickle, OlmMessage, Session, SessionConfig, SessionPickle};
use vodozemac::{Curve25519PublicKey, Ed25519PublicKey, Ed25519Signature};
use zeroize::Zeroizing;

pub use fingerprint::{fingerprint, safety_number};
pub use store::{FileKey, KdfParams, create as create_file_key, set_duress};
pub use termchat_proto as proto;

use proto::{Body, Curve25519Key, Ed25519Key, Envelope, Identity, Inner, Signature, SignedKey};

/// ADR-002: old sessions kept per contact.
const MAX_SESSIONS: usize = 5;
/// protocol.md, section 7.
const MAX_MISSING: usize = 1000;
const ID_CONTEXT: &[u8] = b"termchat-id-v1";
const OTK_CONTEXT: &[u8] = b"termchat-otk-v1";
const AUTH_CONTEXT: &[u8] = b"termchat-auth-v1";

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
    pub body: Body,
    /// Messages known to be missing in this session (dropped or not yet arrived).
    pub missed: u64,
    /// This message started a new session (first contact or the peer reset its state).
    pub new_session: bool,
}

/// No `Debug`: holds secret keys.
#[derive(Default)]
pub struct Client {
    account: Account,
    contacts: HashMap<Ed25519Key, Contact>,
}

struct Contact {
    curve25519: Curve25519Key,
    /// Wrote to us first; not accepted yet.
    is_request: bool,
    /// What they call themselves; from `ContactRequest` / `ContactAccept`.
    nickname: Option<String>,
    /// Newest (active) last.
    sessions: Vec<SessionState>,
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

    /// Answer to the relay's `Challenge` (protocol.md, section 3).
    pub fn auth_signature(&self, relay_host: &str, nonce: &[u8; 32]) -> Signature {
        self.account
            .sign(auth_message(relay_host, nonce))
            .to_bytes()
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
        }
        let sign = |k: Curve25519PublicKey| {
            let key = k.to_bytes();
            SignedKey {
                key,
                signature: self.account.sign([OTK_CONTEXT, &key].concat()).to_bytes(),
            }
        };
        let prekeys = Prekeys {
            one_time: self
                .account
                .one_time_keys()
                .into_values()
                .map(sign)
                .collect(),
            fallback: self.account.fallback_key().into_values().next().map(sign),
        };
        self.account.mark_keys_as_published();
        prekeys
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
            });
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
            recipient: *to,
            olm_type: u8::try_from(olm_type).map_err(|_| Error::Olm)?,
            body,
        })
    }

    /// `sender` is what the relay says; it is checked against the session and the plaintext.
    pub fn decrypt(&mut self, sender: &Ed25519Key, env: &Envelope) -> Result<Received, Error> {
        let me = self.ed25519();
        if env.recipient != me {
            return Err(Error::WrongRecipient);
        }
        let msg = OlmMessage::from_parts(env.olm_type.into(), &env.body)
            .map_err(|_| proto::Error::Malformed)?;

        // Existing session, newest first.
        if let Some(c) = self.contacts.get_mut(sender) {
            for i in (0..c.sessions.len()).rev() {
                let s = &mut c.sessions[i];
                if let OlmMessage::PreKey(m) = &msg
                    && m.session_id() != s.session.session_id()
                {
                    continue;
                }
                let Ok(plain) = s.session.decrypt(&msg) else {
                    continue;
                };
                let inner = open_inner(&plain, sender, &me)?;
                s.received.accept(inner.seq)?;
                let missed = s.received.missed();
                remember_nickname(c, &inner.body);
                // The session the peer uses becomes our active one.
                let s = c.sessions.remove(i);
                c.sessions.push(s);
                return Ok(Received {
                    body: inner.body,
                    missed,
                    new_session: false,
                });
            }
        }

        // New inbound session.
        let OlmMessage::PreKey(pre) = &msg else {
            return Err(match self.contacts.contains_key(sender) {
                true => Error::DecryptFailed,
                false => Error::UnknownSender,
            });
        };
        let their_curve = pre.identity_key();
        if let Some(c) = self.contacts.get(sender)
            && c.curve25519 != their_curve.to_bytes()
        {
            return Err(Error::SenderMismatch);
        }
        let created = self
            .account
            .create_inbound_session(SessionConfig::version_1(), their_curve, pre)
            .map_err(|_| Error::DecryptFailed)?;
        let inner = open_inner(&created.plaintext, sender, &me)?;

        if !self.contacts.contains_key(sender) {
            // A stranger may only send a contact request whose identity binds
            // the relay-reported key to the Curve25519 key of this session.
            let Body::ContactRequest { identity, .. } = &inner.body else {
                return Err(Error::UnknownSender);
            };
            if identity.ed25519 != *sender || identity.curve25519 != their_curve.to_bytes() {
                return Err(Error::SenderMismatch);
            }
            verify_identity(identity)?;
            self.contacts.insert(
                *sender,
                Contact {
                    curve25519: identity.curve25519,
                    is_request: true,
                    nickname: None,
                    sessions: Vec::new(),
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
        if let Some(c) = self.contacts.get_mut(sender) {
            remember_nickname(c, &inner.body);
            push_session(c, s);
        }
        Ok(Received {
            body: inner.body,
            missed,
            new_session: true,
        })
    }

    /// Encrypted state file bytes, with the app's own data (settings, history)
    /// stored alongside. The caller writes them to disk atomically.
    pub fn save(&self, key: &FileKey, app: &str) -> Result<Vec<u8>, Error> {
        let state = State {
            app: app.to_owned(),
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
                    }
                })
                .collect(),
        };
        let plain = Zeroizing::new(serde_json::to_vec(&state).map_err(|_| Error::CorruptState)?);
        store::seal(key, &plain)
    }

    /// Returns the client, the key to save it again, and the app data.
    pub fn load(password: &[u8], file: &[u8]) -> Result<(Self, FileKey, Zeroizing<String>), Error> {
        let (key, plain) = store::open(password, file)?;
        let state: State = serde_json::from_slice(&plain).map_err(|_| Error::CorruptState)?;
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
                    },
                )
            })
            .collect();
        let client = Self {
            account: Account::from_pickle(state.account),
            contacts,
        };
        Ok((client, key, Zeroizing::new(state.app)))
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
                };
                (*key, contact)
            })
            .collect();
        Self {
            account: Account::from_pickle(self.account.pickle()),
            contacts,
        }
    }

    /// Forgets a contact and its sessions.
    pub fn remove_contact(&mut self, contact: &Ed25519Key) {
        self.contacts.remove(contact);
    }

    /// Drops the fallback key that was rotated out (ADR-002). Call once envelopes
    /// sent to it can no longer be in the relay queue.
    pub fn forget_old_fallback(&mut self) -> bool {
        self.account.forget_fallback_key()
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
}

#[derive(Serialize, Deserialize)]
struct State {
    app: String,
    account: AccountPickle,
    contacts: Vec<ContactState>,
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

fn remember_nickname(c: &mut Contact, body: &Body) {
    if let Body::ContactRequest { nickname, .. } | Body::ContactAccept { nickname } = body {
        c.nickname = Some(nickname.clone());
    }
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

/// Relay side of the challenge-response: identity binding and nonce signature.
pub fn verify_auth(
    identity: &Identity,
    relay_host: &str,
    nonce: &[u8; 32],
    sig: &Signature,
) -> Result<(), Error> {
    verify_identity(identity)?;
    verify_message(&identity.ed25519, &auth_message(relay_host, nonce), sig)
}

/// `nonce` has a fixed length at the end, so the concatenation is unambiguous.
fn auth_message(relay_host: &str, nonce: &[u8; 32]) -> Vec<u8> {
    [AUTH_CONTEXT, relay_host.as_bytes(), nonce].concat()
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
