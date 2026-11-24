//! Wire format: envelopes, inner messages, padding, size limits.
//!
//! See `docs/protocol.md`. Everything here parses untrusted input, so every
//! decoder checks sizes and rejects trailing bytes.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// Anonymous queues (ADR-023). Version 1 had accounts and is not supported.
pub const VERSION: u16 = 2;
/// Padded plaintext sizes (protocol.md, section 8). The last one carries
/// `Body::FilePart` only (ADR-043).
pub const BUCKETS: [usize; 4] = [512, 4096, 16384, LARGE_BUCKET];
/// The largest bucket for everything but `Body::FilePart`.
const SMALL_BUCKET: usize = 16384;
const LARGE_BUCKET: usize = 65536;
pub const MAX_TEXT_BYTES: usize = 16_000;
/// Self-chosen display name, only ever sent inside E2EE.
pub const MAX_NICKNAME_BYTES: usize = 64;
/// Free text attached to a contact request.
pub const MAX_NOTE_BYTES: usize = 256;
/// A document sent in chunks (ADR-017).
pub const MAX_FILE_BYTES: u32 = 1 << 20;
/// Data per `Body::File` chunk: fits the largest bucket with a full header.
pub const FILE_CHUNK: usize = 16_000;
pub const MAX_FILE_NAME_BYTES: usize = 255;
/// A document offered and sent on request (ADR-043).
pub const MAX_LARGE_FILE_BYTES: u32 = 25 << 20;
/// Data per `Body::FilePart`: fits the large bucket with its header.
pub const LARGE_CHUNK: usize = 64_000;
/// Parts the sender may have out before the receiver's `FileAck` (ADR-043).
pub const LARGE_WINDOW: u16 = 64;
/// Olm ciphertext of the 16384 bucket plus pre-key overhead: a body above
/// this can only be a `FilePart` (ADR-043, amendment 1).
pub const SMALL_BODY_BYTES: usize = 20 * 1024;
/// Olm ciphertext of the largest bucket plus pre-key overhead.
pub const MAX_BODY_BYTES: usize = 68 * 1024;
/// One WebSocket frame (protocol.md, section 11).
pub const MAX_FRAME_BYTES: usize = 72 * 1024;
/// A UnifiedPush endpoint URL (ADR-012).
pub const MAX_PUSH_URL_BYTES: usize = 512;
/// HTTP header on the relay's upgrade response: its version (ADR-010).
pub const VERSION_HEADER: &str = "x-termchat-version";
/// One-time keys per `PublishKeys`: vodozemac keeps 50 on the server.
pub const MAX_KEYS_PER_PUBLISH: usize = 100;
/// How long the relay keeps an undelivered envelope (ADR-005). Clients keep an
/// old fallback key one day longer (ADR-002).
pub const QUEUE_TTL_DAYS: u64 = 14;

pub type Ed25519Key = [u8; 32];
pub type Curve25519Key = [u8; 32];
pub type Signature = [u8; 64];
/// A queue address (ADR-023): `recv_id` to read it, `send_id` to write to it.
pub type QueueId = [u8; 16];
/// The queue owner's secret; the relay keeps only its SHA-256.
pub type QueueToken = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("malformed message")]
    Malformed,
    #[error("message too large")]
    TooLarge,
    #[error("unsupported protocol version")]
    UnsupportedVersion,
}

/// What the relay forwards from client to client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u16,
    /// 0 = pre-key, 1 = normal (vodozemac `OlmMessage::to_parts`).
    pub olm_type: u8,
    pub body: Vec<u8>,
}

impl Envelope {
    pub fn encode(&self) -> Vec<u8> {
        // Serializing an owned struct into a Vec cannot fail.
        postcard::to_allocvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        // Bound the input before postcard allocates for `body`.
        if bytes.len() > MAX_BODY_BYTES + 64 {
            return Err(Error::TooLarge);
        }
        let env: Self = decode_exact(bytes)?;
        env.check()?;
        Ok(env)
    }

    fn check(&self) -> Result<(), Error> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if self.body.len() > MAX_BODY_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}

/// Client to relay (protocol.md, sections 3-5). Nothing here names a person:
/// the relay knows queues only (ADR-023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientFrame {
    /// First frame on every connection. Stays variant 0, so a version 1
    /// client still gets a readable `UnsupportedVersion`.
    Hello {
        version: u16,
    },
    CreateQueue,
    DeleteQueue {
        recv_id: QueueId,
        token: QueueToken,
    },
    Subscribe {
        recv_id: QueueId,
        token: QueueToken,
    },
    /// Prekeys of the queue; a contact queue carries one-time keys only.
    PublishKeys {
        recv_id: QueueId,
        token: QueueToken,
        one_time: Vec<SignedKey>,
        fallback: Option<SignedKey>,
    },
    ClaimKey {
        send_id: QueueId,
    },
    /// No authentication: knowing `send_id` is the right to write.
    Send {
        client_id: u64,
        send_id: QueueId,
        envelope: Envelope,
    },
    Ack {
        recv_id: QueueId,
        relay_id: u64,
    },
    /// The queue owner chose to disclose `text` to the relay operator (ADR-006).
    Report {
        recv_id: QueueId,
        token: QueueToken,
        text: String,
    },
    /// Where to wake the owner when a message waits in the queue; `None` forgets it (ADR-012).
    SetPush {
        recv_id: QueueId,
        token: QueueToken,
        endpoint: Option<String>,
    },
    /// A group queue (ADR-027): what is sent to its `send_id` reaches every reader.
    CreateGroup,
    /// The group's admin adds a reader: a queue of its own for one member.
    AddReader {
        group_id: QueueId,
        token: QueueToken,
    },
    RemoveReader {
        group_id: QueueId,
        token: QueueToken,
        recv_id: QueueId,
    },
    DeleteGroup {
        group_id: QueueId,
        token: QueueToken,
    },
    /// ADR-042: `RemoveReader` for a reader named by its `send_id`, which the
    /// roster gives for every reader, those another admin made included.
    RemoveReaderAt {
        group_id: QueueId,
        token: QueueToken,
        send_id: QueueId,
    },
}

/// Relay to client. `Error` stays variant 7, as in version 1, so an old
/// client can read why it was turned away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerFrame {
    QueueCreated {
        recv_id: QueueId,
        send_id: QueueId,
        token: QueueToken,
    },
    Subscribed {
        recv_id: QueueId,
    },
    ClaimedKey {
        send_id: QueueId,
        key: SignedKey,
    },
    /// Sent to the queue's subscriber. `fallback_used`: the relay handed out
    /// the fallback key; rotate it (ADR-002).
    KeyCount {
        recv_id: QueueId,
        one_time_left: u32,
        fallback_used: bool,
    },
    Accepted {
        client_id: u64,
    },
    /// Same answer whatever the relay did.
    Reported,
    Deliver {
        recv_id: QueueId,
        relay_id: u64,
        envelope: Envelope,
    },
    Error {
        client_id: Option<u64>,
        code: ErrorCode,
    },
    QueueDeleted {
        recv_id: QueueId,
    },
    /// `SetPush` was stored (ADR-012).
    PushSet,
    /// ADR-027. `group_id` and `token` are the admin's; `send_id` is for members.
    GroupCreated {
        group_id: QueueId,
        send_id: QueueId,
        token: QueueToken,
    },
    /// A reader's queue, for the admin to hand to one member: `recv_id` and
    /// `token` to read, `send_id` to write to that member alone.
    ReaderAdded {
        group_id: QueueId,
        recv_id: QueueId,
        send_id: QueueId,
        token: QueueToken,
    },
    ReaderRemoved {
        recv_id: QueueId,
    },
    GroupDeleted {
        group_id: QueueId,
    },
}

/// protocol.md, section 12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// Variant 0, as in version 1 (see `ServerFrame`).
    UnsupportedVersion,
    Malformed,
    TooLarge,
    RateLimited,
    NoKeys,
    /// No such queue, or a wrong token: one answer, so a token cannot probe
    /// which queues exist.
    UnknownQueue,
    QueueFull,
    Internal,
}

impl From<Error> for ErrorCode {
    fn from(e: Error) -> Self {
        match e {
            Error::Malformed => Self::Malformed,
            Error::TooLarge => Self::TooLarge,
            Error::UnsupportedVersion => Self::UnsupportedVersion,
        }
    }
}

impl ClientFrame {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(Error::TooLarge);
        }
        let frame: Self = decode_exact(bytes)?;
        match &frame {
            Self::Send { envelope, .. } => envelope.check()?,
            Self::PublishKeys { one_time, .. } if one_time.len() > MAX_KEYS_PER_PUBLISH => {
                return Err(Error::TooLarge);
            }
            Self::Report { text, .. } if text.len() > MAX_TEXT_BYTES => {
                return Err(Error::TooLarge);
            }
            Self::SetPush {
                endpoint: Some(url),
                ..
            } if url.len() > MAX_PUSH_URL_BYTES => return Err(Error::TooLarge),
            _ => {}
        }
        Ok(frame)
    }
}

impl ServerFrame {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(Error::TooLarge);
        }
        let frame: Self = decode_exact(bytes)?;
        if let Self::Deliver { envelope, .. } = &frame {
            envelope.check()?;
        }
        Ok(frame)
    }
}

/// Self-certifying identity: the Ed25519 key signs the Curve25519 key
/// (`"termchat-id-v1" || curve25519`). Sent in invites and contact requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub ed25519: Ed25519Key,
    pub curve25519: Curve25519Key,
    #[serde(with = "serde_big_array")]
    pub signature: Signature,
}

/// What a user hands out to be added: `termchat:<base64url(postcard)>` (protocol.md, section 9).
/// Versions 1 and 2 named only the identity; they are refused (ADR-023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub version: u16,
    pub relay_url: String,
    /// The same relay as a Tor onion service, if it has one (ADR-004).
    pub onion_url: Option<String>,
    pub identity: Identity,
    /// `send_id` of the invite queue, where strangers write.
    pub queue: QueueId,
    /// Echoed in `ContactRequestV2` (ADR-013).
    pub code: InviteCode,
}

pub type InviteCode = [u8; 16];

pub const INVITE_VERSION: u16 = 3;
const INVITE_PREFIX: &str = "termchat:";
/// A new device's link code (ADR-028): the same fields as an invite, under
/// a prefix of its own, so neither can be taken for the other.
const LINK_PREFIX: &str = "termchat-link:";
const MAX_RELAY_URL: usize = 256;

fn check_urls(relay_url: &str, onion_url: Option<&str>) -> Result<(), Error> {
    if relay_url.len() > MAX_RELAY_URL || onion_url.is_some_and(|u| u.len() > MAX_RELAY_URL) {
        return Err(Error::TooLarge);
    }
    Ok(())
}

impl Invite {
    pub fn encode(&self) -> String {
        self.encode_with(INVITE_PREFIX)
    }

    /// Tolerates surrounding whitespace from copy-paste.
    pub fn decode(s: &str) -> Result<Self, Error> {
        Self::decode_with(s, INVITE_PREFIX)
    }

    /// A new device's link code (ADR-028).
    pub fn encode_link(&self) -> String {
        self.encode_with(LINK_PREFIX)
    }

    pub fn decode_link(s: &str) -> Result<Self, Error> {
        Self::decode_with(s, LINK_PREFIX)
    }

    fn encode_with(&self, prefix: &str) -> String {
        format!(
            "{prefix}{}",
            URL_SAFE_NO_PAD.encode(postcard::to_allocvec(self).unwrap_or_default())
        )
    }

    fn decode_with(s: &str, prefix: &str) -> Result<Self, Error> {
        let b64 = s.trim().strip_prefix(prefix).ok_or(Error::Malformed)?;
        if b64.len() > 8 * MAX_RELAY_URL {
            return Err(Error::TooLarge);
        }
        let bytes = URL_SAFE_NO_PAD.decode(b64).map_err(|_| Error::Malformed)?;
        let (version, _): (u16, _) =
            postcard::take_from_bytes(&bytes).map_err(|_| Error::Malformed)?;
        if version != INVITE_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let invite: Self = decode_exact(&bytes)?;
        invite.check()?;
        Ok(invite)
    }

    /// What an invite inside a message must pass too (ADR-045).
    pub fn check(&self) -> Result<(), Error> {
        if self.version != INVITE_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        check_urls(&self.relay_url, self.onion_url.as_deref())
    }
}

/// One-time or fallback key, signed over `"termchat-otk-v1" || key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedKey {
    pub key: Curve25519Key,
    #[serde(with = "serde_big_array")]
    pub signature: Signature,
}

/// Plaintext inside the Olm ciphertext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inner {
    pub sender: Ed25519Key,
    pub recipient: Ed25519Key,
    /// Per session and direction, starts at 1.
    pub seq: u64,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    /// First message to someone who does not know us yet.
    /// The nickname is the only thing we learn about a person; no real names,
    /// phones or emails exist anywhere in the protocol.
    ContactRequest {
        identity: Identity,
        nickname: String,
        note: String,
    },
    ContactAccept {
        nickname: String,
    },
    Text {
        sent_at: u64,
        expires_in: Option<u32>,
        text: String,
    },
    /// The sender wiped their account and will not read anything (ADR-009).
    Goodbye,
    /// `ContactRequest` sent from a version 2 invite, echoing its code (ADR-013).
    ContactRequestV2 {
        identity: Identity,
        nickname: String,
        note: String,
        code: InviteCode,
    },
    /// One chunk of a document, opaque bytes (ADR-017). Name and size repeat
    /// in every chunk; chunk `index` holds bytes from `index * FILE_CHUNK`.
    File {
        id: u32,
        name: String,
        size: u32,
        index: u16,
        expires_in: Option<u32>,
        data: Vec<u8>,
    },
    /// Write to me here from now on (ADR-023). Sent only inside Olm, so only
    /// the contact learns the address.
    QueueOffer {
        relay_url: String,
        onion_url: Option<String>,
        send_id: QueueId,
    },
    /// A text that can be replied to, edited, deleted and reacted to
    /// (ADR-025). `id` is random and names the message among the sender's own.
    Message {
        id: u64,
        sent_at: u64,
        expires_in: Option<u32>,
        reply_to: Option<Ref>,
        text: String,
    },
    /// New text for the sender's own `Message` `id` (ADR-025).
    Edit {
        id: u64,
        text: String,
    },
    /// The sender's own `Message` `id` is to be wiped (ADR-025).
    Delete {
        id: u64,
    },
    /// One of `REACTIONS`, or `None` to take one's reaction back (ADR-025).
    React {
        target: Ref,
        reaction: Option<u8>,
    },
    /// The kinds of message the sender reads: `UNDERSTANDS` (ADR-025).
    Understands {
        level: u16,
    },
    /// The admin asks a contact into a group (ADR-027), one to one: the roster
    /// and the contact's own reader of the group queue.
    GroupInvite {
        roster: Roster,
        relay_url: String,
        onion_url: Option<String>,
        send_id: QueueId,
        reader: ReaderGrant,
    },
    /// The contact took the invite; the admin puts them on the roster.
    GroupJoin {
        group: GroupId,
    },
    /// A member's Megolm session key, sent to one member over Olm.
    GroupKey {
        group: GroupId,
        key: SecretBytes,
    },
    /// A new roster, from the admin, inside the group.
    GroupRoster {
        roster: Roster,
    },
    /// The sender leaves the group, inside the group; from the admin over a
    /// chat, the receiver was removed.
    GroupLeave {
        group: GroupId,
    },
    /// The admin moved the group to a new queue (someone was removed), over a
    /// chat: the new roster and the receiver's reader of the new queue.
    GroupMove {
        roster: Roster,
        send_id: QueueId,
        reader: ReaderGrant,
    },
    /// ADR-028. The primary answers a new device's link code, before anything
    /// secret: the code, and the identity whose fingerprint both screens show.
    DeviceLink {
        code: InviteCode,
        identity: Identity,
    },
    /// The new device: the person saw the same fingerprint on both screens,
    /// and its proof for the list (ADR-030).
    DeviceLinked {
        #[serde(with = "serde_big_array")]
        proof: Signature,
    },
    /// The primary to a new device: one contact, to talk to from there.
    LinkedContact {
        contact: LinkedContact,
    },
    /// To one's own devices only: the device list, each device's label, and
    /// the nickname contacts know.
    OwnDevices {
        list: DeviceList,
        labels: Vec<(Ed25519Key, String)>,
        nickname: String,
    },
    /// To a contact: the device list, and where each device reads this contact.
    Devices {
        list: DeviceList,
        queues: Vec<DeviceQueue>,
    },
    /// To one's own other devices: our queue for `contact`, where copies of
    /// what we all send them go (ADR-028).
    OwnQueue {
        contact: Ed25519Key,
        queue: DeviceQueue,
    },
    /// Between linked devices, through the primary (ADR-028): "`queue` is
    /// where its device reads `device`". The primary passes it on to `device`.
    SiblingQueue {
        device: Ed25519Key,
        queue: DeviceQueue,
    },
    /// From the admin, inside the group (ADR-028): a member's device list,
    /// as they signed it, and the reader made for each linked device.
    GroupDevices {
        group: GroupId,
        member: Ed25519Key,
        list: DeviceList,
        readers: Vec<(Ed25519Key, QueueId)>,
    },
    /// To one's own devices: we joined `group`; take your own invite to it.
    OwnGroupJoin {
        group: GroupId,
    },
    /// ADR-030: to one's own devices, what we sent to `contact`. Filed under
    /// that chat only if it came in our queue for that contact.
    Copy {
        contact: Ed25519Key,
        body: Box<Body>,
    },
    /// ADR-031: a liveness pulse for a contact's direct queue.
    Heartbeat {
        sent_at: u64,
    },
    /// ADR-041: our `Message` ids and whole `File` ids the contact got
    /// (`read`: also saw on screen). At most `MAX_RECEIPT_IDS` of each.
    Receipt {
        messages: Vec<u64>,
        files: Vec<u32>,
        read: bool,
    },
    /// ADR-042: the group's admin lists, oldest first, each signed by an
    /// admin of the list before it; the last one is current. Sent just before
    /// the roster it came with, in the same channel.
    GroupAdmins {
        group: GroupId,
        chain: Vec<AdminList>,
    },
    /// ADR-042: the main admin leaves the role, inside the group, before
    /// their `GroupLeave`; the next admin puts it into their list.
    GroupResign {
        group: GroupId,
        resign: Resign,
    },
    /// ADR-042: the group queue's admin token, over Olm to one admin's
    /// primary device only; never inside the group, never in a `Copy`.
    GroupAdminKeys {
        group: GroupId,
        roster_no: u64,
        grant: AdminGrant,
    },
    /// ADR-043: a document of more than 1 MiB, sent only after `FileWant`.
    FileOffer {
        id: u32,
        name: String,
        size: u32,
        expires_in: Option<u32>,
    },
    /// ADR-043: the receiver asks for offer `id`.
    FileWant {
        id: u32,
    },
    /// ADR-043: every part of `id` below `next` arrived.
    FileAck {
        id: u32,
        next: u16,
    },
    /// ADR-043: either side drops transfer `id`.
    FileStop {
        id: u32,
    },
    /// ADR-043: part `index` of offer `id`, bytes from `index * LARGE_CHUNK`.
    FilePart {
        id: u32,
        index: u16,
        data: Vec<u8>,
    },
    /// ADR-045: the introducer asks the receiver to meet contact `key`.
    IntroAsk {
        intro: u64,
        nickname: String,
        key: Ed25519Key,
    },
    /// ADR-045: yes, with a one-time invite made for this introduction.
    IntroGrant {
        intro: u64,
        invite: Invite,
    },
    /// ADR-045: no.
    IntroDecline {
        intro: u64,
    },
    /// ADR-045: the introducer passes the invite on to the other contact.
    IntroOffer {
        intro: u64,
        nickname: String,
        invite: Invite,
    },
    /// ADR-046: a note to self, from one of our devices to another: a
    /// `Message`, `Edit`, `Delete` or `File` chunk.
    Note {
        body: Box<Body>,
    },
    /// A kind this version does not know, from a newer client (ADR-025): its
    /// `seq` counts, nothing is shown. Never on the wire; keep it last, and
    /// put new kinds above it.
    #[serde(skip)]
    Unknown,
}

/// Readers of a group queue, the admin included (ADR-027).
pub const MAX_GROUP_MEMBERS: usize = 32;

/// Kinds of `Body` on the wire: every variant but `Unknown`.
const KNOWN_KINDS: u32 = 43;

/// What `Understands` says this version reads: 1 = ADR-025, 2 = groups
/// (ADR-027), 3 = devices (ADR-028), 4 = devices that prove themselves and
/// copies that name their chat (ADR-030), 5 = receipts (ADR-041), 6 = several
/// group admins (ADR-042), 7 = documents on request (ADR-043), 8 =
/// introductions (ADR-045), 9 = notes to self (ADR-046).
pub const UNDERSTANDS: u16 = 9;

/// The level from which our own device takes notes (ADR-046).
pub const NOTES_LEVEL: u16 = 9;

/// The level from which a contact takes introductions (ADR-045).
pub const INTRO_LEVEL: u16 = 8;

/// The level from which a contact takes `FileOffer` (ADR-043).
pub const LARGE_FILES_LEVEL: u16 = 7;

/// The level from which a member takes rosters from several admins (ADR-042).
pub const ADMINS_LEVEL: u16 = 6;

/// The level from which a contact reads proven device lists (ADR-030):
/// what linked devices need, whatever later levels add.
pub const DEVICES_LEVEL: u16 = 4;

/// Ids of each kind in one `Receipt` (ADR-041).
pub const MAX_RECEIPT_IDS: usize = 64;

/// Devices of one identity, the primary included (ADR-028).
pub const MAX_DEVICES: usize = 4;

/// A person's devices, numbered and signed by their identity key (ADR-028).
/// The first device is the primary: its key is the identity key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    pub identity: Ed25519Key,
    pub no: u64,
    pub devices: Vec<Device>,
    #[serde(with = "serde_big_array")]
    pub signature: Signature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub ed25519: Ed25519Key,
    pub curve25519: Curve25519Key,
    /// ADR-030: the device's own signature that it belongs to this identity
    /// with this Curve25519 key (`DEVICE_PROOF_CONTEXT`). Zero in lists
    /// saved before it, which no longer verify.
    #[serde(with = "serde_big_array", default = "no_signature")]
    pub proof: Signature,
}

/// What a device signs to join an identity (ADR-030): this, the identity
/// key, then its own Curve25519 key.
pub const DEVICE_PROOF_CONTEXT: &[u8] = b"termchat-device-v1";

fn no_signature() -> Signature {
    [0; 64]
}

impl DeviceList {
    /// What the identity key signs: everything but the signature.
    pub fn signed_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(&(self.identity, self.no, &self.devices)).unwrap_or_default()
    }

    pub fn check(&self) -> Result<(), Error> {
        if self.devices.len() > MAX_DEVICES {
            return Err(Error::TooLarge);
        }
        let first = self.devices.first().ok_or(Error::Malformed)?;
        let doubled = self
            .devices
            .iter()
            .enumerate()
            .any(|(i, d)| self.devices[..i].iter().any(|e| e.ed25519 == d.ed25519));
        if first.ed25519 != self.identity || doubled {
            return Err(Error::Malformed);
        }
        Ok(())
    }
}

/// Where one device reads one contact: write to it at `send_id` (ADR-028).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceQueue {
    pub device: Ed25519Key,
    pub relay_url: String,
    pub onion_url: Option<String>,
    pub send_id: QueueId,
}

/// A contact, as the primary hands it to a new device (ADR-028).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedContact {
    pub identity: Identity,
    pub nickname: String,
    /// Our own name for them, if we gave one.
    pub petname: Option<String>,
    pub verified: bool,
    pub understands: u16,
    /// Their device list, if they have more than one device.
    pub devices: Option<DeviceList>,
    /// Where each of their devices reads us.
    pub queues: Vec<DeviceQueue>,
}

fn check_queues(queues: &[DeviceQueue]) -> Result<(), Error> {
    if queues.len() > MAX_DEVICES {
        return Err(Error::TooLarge);
    }
    queues
        .iter()
        .try_for_each(|q| check_urls(&q.relay_url, q.onion_url.as_deref()))
}

/// A device's label, "phone" or "laptop": like a nickname, but may be empty.
fn check_label(label: &str) -> Result<(), Error> {
    if label.len() > MAX_NICKNAME_BYTES {
        return Err(Error::TooLarge);
    }
    if label.chars().any(char::is_control) {
        return Err(Error::Malformed);
    }
    Ok(())
}

/// A group's name, like a nickname.
pub const MAX_GROUP_NAME_BYTES: usize = 64;
/// A Megolm session key: 229 bytes; room to spare, no more.
const MAX_GROUP_KEY_BYTES: usize = 512;

/// Bytes that must not show in logs: `Debug` prints their length only.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretBytes(pub Vec<u8>);

impl std::fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretBytes({} bytes)", self.0.len())
    }
}

/// One member's reader of a group queue (ADR-027): what the relay's
/// `ReaderAdded` gave the admin. `Debug` hides the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderGrant {
    pub recv_id: QueueId,
    pub send_id: QueueId,
    pub token: QueueToken,
}

impl std::fmt::Debug for ReaderGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReaderGrant").finish_non_exhaustive()
    }
}

/// Admins of one group, the main admin included (ADR-042).
pub const MAX_GROUP_ADMINS: usize = 4;

/// How far ahead of ours a roster or admin list may number itself (ADR-042,
/// amendment 4): a member far behind still catches up, and nobody takes the
/// last number.
pub const MAX_ROSTER_STEP: u64 = 1024;

/// Admin lists in one `GroupAdmins` (ADR-042): enough to catch up after a gap.
pub const MAX_ADMIN_CHAIN: usize = 8;

/// Who may change a group (ADR-042): the main admin first, then the helpers
/// in the order they were made. `no` is the roster it came with; `signer`
/// is an admin of the list before, and the receiver checks their right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminList {
    pub group: GroupId,
    pub no: u64,
    pub signer: Ed25519Key,
    pub admins: Vec<Ed25519Key>,
    /// The main admin's word that they left, when their successor signs.
    pub resign: Option<Resign>,
    #[serde(with = "serde_big_array")]
    pub signature: Signature,
}

impl AdminList {
    /// What the signer signs: everything but the signature.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let fields = (self.group, self.no, self.signer, &self.admins, &self.resign);
        postcard::to_allocvec(&fields).unwrap_or_default()
    }

    pub fn check(&self) -> Result<(), Error> {
        if self.admins.len() > MAX_GROUP_ADMINS {
            return Err(Error::TooLarge);
        }
        let doubled = self
            .admins
            .iter()
            .enumerate()
            .any(|(i, a)| self.admins[..i].contains(a));
        if self.admins.is_empty() || doubled {
            return Err(Error::Malformed);
        }
        Ok(())
    }
}

/// The main admin's word that they leave the role at roster `roster_no`
/// (ADR-042), signed by their identity key. The group is the list's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resign {
    pub roster_no: u64,
    #[serde(with = "serde_big_array")]
    pub signature: Signature,
}

impl Resign {
    /// What the main admin signs: the group and the roster number.
    pub fn signed_bytes(&self, group: &GroupId) -> Vec<u8> {
        postcard::to_allocvec(&(group, self.roster_no)).unwrap_or_default()
    }
}

fn check_admin_chain(group: &GroupId, chain: &[AdminList]) -> Result<(), Error> {
    if chain.len() > MAX_ADMIN_CHAIN {
        return Err(Error::TooLarge);
    }
    chain.iter().try_for_each(AdminList::check)?;
    let in_order = chain.windows(2).all(|w| w[0].no < w[1].no);
    if chain.is_empty() || !in_order || chain.iter().any(|l| l.group != *group) {
        return Err(Error::Malformed);
    }
    Ok(())
}

/// The admin token of a group queue (ADR-042), for another admin.
/// `Debug` hides the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGrant {
    pub queue: QueueId,
    pub token: QueueToken,
}

impl std::fmt::Debug for AdminGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminGrant").finish_non_exhaustive()
    }
}

/// Who is in a group, numbered and signed by its admin (ADR-027).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster {
    pub group: GroupId,
    pub no: u64,
    pub admin: Ed25519Key,
    pub name: String,
    pub members: Vec<RosterMember>,
    /// The group's disappearing timer, seconds; the admin sets it.
    pub expires_in: Option<u32>,
    #[serde(with = "serde_big_array")]
    pub signature: Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterMember {
    pub ed25519: Ed25519Key,
    pub curve25519: Curve25519Key,
    pub nickname: String,
    /// Their reader's `send_id`: to share a session key with them alone.
    pub reader: QueueId,
}

impl Roster {
    /// What the admin signs: everything but the signature.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let fields = (
            self.group,
            self.no,
            self.admin,
            &self.name,
            &self.members,
            self.expires_in,
        );
        postcard::to_allocvec(&fields).unwrap_or_default()
    }

    fn check(&self) -> Result<(), Error> {
        check_group_name(&self.name)?;
        if self.members.len() > MAX_GROUP_MEMBERS {
            return Err(Error::TooLarge);
        }
        // Review 2026-11d, finding 2: one entry per key, as in admin and
        // device lists; a double could never be removed.
        let doubled = self
            .members
            .iter()
            .enumerate()
            .any(|(i, m)| self.members[..i].iter().any(|e| e.ed25519 == m.ed25519));
        if doubled {
            return Err(Error::Malformed);
        }
        self.members
            .iter()
            .try_for_each(|m| check_nickname(&m.nickname))
    }
}

pub fn check_group_name(name: &str) -> Result<(), Error> {
    if name.len() > MAX_GROUP_NAME_BYTES {
        return Err(Error::TooLarge);
    }
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(Error::Malformed);
    }
    Ok(())
}

/// Reactions are indices into a fixed set the apps draw (ADR-025):
/// like, love, laugh, wow, sad, thanks, dislike.
pub const REACTIONS: u8 = 7;

/// A message in a chat, seen from whoever sends the `Ref`: `theirs` is
/// false for their own message, true for the recipient's (ADR-025).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    pub id: u64,
    pub theirs: bool,
}

/// The part of `Inner` before the body, to read past a body of unknown kind.
#[derive(Deserialize)]
struct InnerHead {
    sender: Ed25519Key,
    recipient: Ed25519Key,
    seq: u64,
}

/// Pads to the smallest bucket (ISO/IEC 7816-4: 0x80, then zeros).
fn pad(mut buf: Vec<u8>, largest: usize) -> Result<Vec<u8>, Error> {
    let bucket = *BUCKETS
        .iter()
        .find(|&&b| b > buf.len() && b <= largest)
        .ok_or(Error::TooLarge)?;
    buf.push(0x80);
    buf.resize(bucket, 0);
    Ok(buf)
}

/// The bytes before the padding.
fn unpad(bytes: &[u8]) -> Result<&[u8], Error> {
    if !BUCKETS.contains(&bytes.len()) {
        return Err(Error::Malformed);
    }
    let end = bytes
        .iter()
        .rposition(|&b| b != 0)
        .ok_or(Error::Malformed)?;
    if bytes[end] != 0x80 {
        return Err(Error::Malformed);
    }
    Ok(&bytes[..end])
}

impl Inner {
    /// Serialize and pad to the smallest bucket (ISO/IEC 7816-4: 0x80, then zeros).
    pub fn to_padded(&self) -> Result<Vec<u8>, Error> {
        self.check_limits()?;
        let largest = match self.body {
            Body::FilePart { .. } => LARGE_BUCKET,
            _ => SMALL_BUCKET,
        };
        pad(
            postcard::to_allocvec(self).map_err(|_| Error::Malformed)?,
            largest,
        )
    }

    pub fn from_padded(bytes: &[u8]) -> Result<Self, Error> {
        let large = bytes.len() == LARGE_BUCKET;
        let bytes = unpad(bytes)?;
        let inner: Self = match decode_exact(bytes) {
            Ok(inner) => inner,
            Err(_) if large => return Err(Error::Malformed),
            Err(e) => return Self::unknown_kind(bytes).ok_or(e),
        };
        // ADR-043: the large bucket carries parts and nothing else.
        if large && !matches!(inner.body, Body::FilePart { .. }) {
            return Err(Error::Malformed);
        }
        inner.check_limits()?;
        Ok(inner)
    }

    /// A body of a kind newer than this version, after a valid head.
    fn unknown_kind(bytes: &[u8]) -> Option<Self> {
        let (head, rest) = postcard::take_from_bytes::<InnerHead>(bytes).ok()?;
        let (kind, _) = postcard::take_from_bytes::<u32>(rest).ok()?;
        (kind >= KNOWN_KINDS).then_some(Self {
            sender: head.sender,
            recipient: head.recipient,
            seq: head.seq,
            body: Body::Unknown,
        })
    }

    fn check_limits(&self) -> Result<(), Error> {
        check_body(&self.body)
    }
}

/// A group's own handle, chosen by its admin (ADR-027).
pub type GroupId = [u8; 16];

/// `Envelope::olm_type` of a group message: Megolm, not Olm (ADR-027).
pub const OLM_TYPE_MEGOLM: u8 = 2;

/// Plaintext inside a group's Megolm message (ADR-027). `sender` must be the
/// member whose session it is; the receiver checks that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInner {
    pub sender: Ed25519Key,
    pub group: GroupId,
    pub body: Body,
}

impl GroupInner {
    pub fn to_padded(&self) -> Result<Vec<u8>, Error> {
        self.check_body()?;
        pad(
            postcard::to_allocvec(self).map_err(|_| Error::Malformed)?,
            SMALL_BUCKET,
        )
    }

    pub fn from_padded(bytes: &[u8]) -> Result<Self, Error> {
        let inner: Self = decode_exact(unpad(bytes)?)?;
        inner.check_body()?;
        Ok(inner)
    }

    fn check_body(&self) -> Result<(), Error> {
        match &self.body {
            // ADR-041: receipts are between two people, never in a group.
            // ADR-042: every member would read the admin token.
            // ADR-043: offers are between two people for now.
            Body::Heartbeat { .. }
            | Body::Receipt { .. }
            | Body::GroupAdminKeys { .. }
            | Body::FileOffer { .. }
            | Body::FileWant { .. }
            | Body::FileAck { .. }
            | Body::FileStop { .. }
            | Body::FilePart { .. }
            | Body::IntroAsk { .. }
            | Body::IntroGrant { .. }
            | Body::IntroDecline { .. }
            | Body::IntroOffer { .. }
            | Body::Note { .. } => Err(Error::Malformed),
            body => check_body(body),
        }
    }
}

/// A group message on the wire, in `Envelope::body`: which Megolm session,
/// and the message. Megolm messages do not name their session themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCiphertext {
    pub session_id: String,
    pub message: Vec<u8>,
}

/// A Megolm session id: the base64 of an Ed25519 key, 43 characters.
const MAX_SESSION_ID: usize = 64;

impl GroupCiphertext {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let c: Self = decode_exact(bytes)?;
        if c.session_id.len() > MAX_SESSION_ID {
            return Err(Error::Malformed);
        }
        Ok(c)
    }
}

fn check_body(body: &Body) -> Result<(), Error> {
    match body {
        Body::Text { text, .. } | Body::Message { text, .. } | Body::Edit { text, .. }
            if text.len() > MAX_TEXT_BYTES =>
        {
            Err(Error::TooLarge)
        }
        Body::Unknown => Err(Error::Malformed),
        // One level: a copy of a copy is no message anyone sent.
        // ADR-042: the admin token goes to the admin's primary device alone.
        Body::Copy { body, .. } => match **body {
            Body::Copy { .. }
            | Body::Heartbeat { .. }
            | Body::Receipt { .. }
            | Body::GroupAdminKeys { .. }
            | Body::FileOffer { .. }
            | Body::FileWant { .. }
            | Body::FileAck { .. }
            | Body::FileStop { .. }
            | Body::FilePart { .. }
            | Body::IntroAsk { .. }
            | Body::IntroGrant { .. }
            | Body::IntroDecline { .. }
            | Body::IntroOffer { .. }
            | Body::Note { .. } => Err(Error::Malformed),
            ref inner => check_body(inner),
        },
        Body::Receipt {
            messages, files, ..
        } if messages.len() > MAX_RECEIPT_IDS || files.len() > MAX_RECEIPT_IDS => {
            Err(Error::TooLarge)
        }
        Body::GroupInvite {
            roster,
            relay_url,
            onion_url,
            ..
        } => {
            roster.check()?;
            check_urls(relay_url, onion_url.as_deref())
        }
        Body::GroupRoster { roster } | Body::GroupMove { roster, .. } => roster.check(),
        Body::GroupAdmins { group, chain } => check_admin_chain(group, chain),
        Body::GroupKey { key, .. } if key.0.len() > MAX_GROUP_KEY_BYTES => Err(Error::TooLarge),
        Body::LinkedContact { contact } => {
            check_nickname(&contact.nickname)?;
            contact.petname.as_deref().map_or(Ok(()), check_nickname)?;
            contact.devices.as_ref().map_or(Ok(()), DeviceList::check)?;
            check_queues(&contact.queues)
        }
        Body::OwnDevices {
            list,
            labels,
            nickname,
        } => {
            list.check()?;
            check_nickname(nickname)?;
            if labels.len() > MAX_DEVICES {
                return Err(Error::TooLarge);
            }
            labels.iter().try_for_each(|(_, l)| check_label(l))
        }
        Body::Devices { list, queues } => {
            list.check()?;
            check_queues(queues)
        }
        Body::GroupDevices { list, readers, .. } => {
            list.check()?;
            if readers.len() > MAX_DEVICES {
                return Err(Error::TooLarge);
            }
            Ok(())
        }
        Body::OwnQueue { queue, .. } | Body::SiblingQueue { queue, .. } => {
            check_queues(std::slice::from_ref(queue))
        }
        Body::ContactRequest { note, .. } | Body::ContactRequestV2 { note, .. }
            if note.len() > MAX_NOTE_BYTES =>
        {
            Err(Error::TooLarge)
        }
        Body::ContactRequest { nickname, .. }
        | Body::ContactRequestV2 { nickname, .. }
        | Body::ContactAccept { nickname } => check_nickname(nickname),
        Body::QueueOffer {
            relay_url,
            onion_url,
            ..
        } => check_urls(relay_url, onion_url.as_deref()),
        Body::File {
            name,
            size,
            index,
            data,
            ..
        } => {
            check_file_name(name)?;
            if *size == 0 || *size > MAX_FILE_BYTES {
                return Err(Error::TooLarge);
            }
            if data.len() != chunk_len(*size, *index).ok_or(Error::Malformed)? {
                return Err(Error::Malformed);
            }
            Ok(())
        }
        // ADR-046: what a note can be, one level deep.
        Body::Note { body } => match **body {
            Body::Message { .. } | Body::Edit { .. } | Body::Delete { .. } | Body::File { .. } => {
                check_body(body)
            }
            _ => Err(Error::Malformed),
        },
        Body::IntroAsk { nickname, .. } => check_nickname(nickname),
        Body::IntroGrant { invite, .. } => invite.check(),
        Body::IntroOffer {
            nickname, invite, ..
        } => {
            check_nickname(nickname)?;
            invite.check()
        }
        Body::FileOffer { name, size, .. } => {
            check_file_name(name)?;
            if *size == 0 || *size > MAX_LARGE_FILE_BYTES {
                return Err(Error::TooLarge);
            }
            Ok(())
        }
        // The exact length needs the offer's size; the receiver checks it.
        Body::FilePart { index, data, .. }
            if data.is_empty()
                || data.len() > LARGE_CHUNK
                || usize::from(*index) >= large_chunks(MAX_LARGE_FILE_BYTES) =>
        {
            Err(Error::Malformed)
        }
        Body::FileAck { next, .. } if usize::from(*next) > large_chunks(MAX_LARGE_FILE_BYTES) => {
            Err(Error::Malformed)
        }
        _ => Ok(()),
    }
}

/// Number of chunks a file of `size` bytes takes.
pub fn file_chunks(size: u32) -> usize {
    (size as usize).div_ceil(FILE_CHUNK)
}

/// Exact data length of chunk `index`, or `None` past the last chunk.
pub fn chunk_len(size: u32, index: u16) -> Option<usize> {
    let start = usize::from(index) * FILE_CHUNK;
    (start < size as usize).then(|| (size as usize - start).min(FILE_CHUNK))
}

/// Number of parts a document of `size` bytes takes (ADR-043).
pub fn large_chunks(size: u32) -> usize {
    (size as usize).div_ceil(LARGE_CHUNK)
}

/// Exact data length of part `index`, or `None` past the last part.
pub fn large_chunk_len(size: u32, index: u16) -> Option<usize> {
    let start = usize::from(index) * LARGE_CHUNK;
    (start < size as usize).then(|| (size as usize - start).min(LARGE_CHUNK))
}

/// Control characters drive a terminal; bidi overrides reorder what is
/// around them. Neither belongs in anything a peer gets shown.
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A name only, never a path: the receiver joins it to a folder it picked.
pub fn check_file_name(name: &str) -> Result<(), Error> {
    if name.len() > MAX_FILE_NAME_BYTES {
        return Err(Error::TooLarge);
    }
    if name.trim().is_empty()
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| is_unsafe_char(c) || matches!(c, '/' | '\\'))
    {
        return Err(Error::Malformed);
    }
    Ok(())
}

/// Accepts only the canonical encoding: postcard tolerates overlong varints,
/// so the value must re-encode to exactly the input (found by fuzzing).
/// Control characters would let a peer drive the recipient's terminal.
pub fn check_nickname(nickname: &str) -> Result<(), Error> {
    if nickname.len() > MAX_NICKNAME_BYTES {
        return Err(Error::TooLarge);
    }
    if nickname.trim().is_empty() || nickname.chars().any(char::is_control) {
        return Err(Error::Malformed);
    }
    Ok(())
}

fn decode_exact<'a, T: Serialize + Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, Error> {
    let value: T = postcard::from_bytes(bytes).map_err(|_| Error::Malformed)?;
    match postcard::to_allocvec(&value) {
        Ok(canonical) if canonical == bytes => Ok(value),
        _ => Err(Error::Malformed),
    }
}

/// serde supports arrays only up to 32 elements; signatures are 64 bytes.
mod serde_big_array {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        let mut halves = ([0u8; 32], [0u8; 32]);
        halves.0.copy_from_slice(&v[..32]);
        halves.1.copy_from_slice(&v[32..]);
        halves.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let (a, b) = <([u8; 32], [u8; 32])>::deserialize(d)?;
        let mut out = [0; 64];
        out[..32].copy_from_slice(&a);
        out[32..].copy_from_slice(&b);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(len: usize) -> Inner {
        Inner {
            sender: [1; 32],
            recipient: [2; 32],
            seq: 1,
            body: Body::Text {
                sent_at: 0,
                expires_in: None,
                text: "a".repeat(len),
            },
        }
    }

    fn with(body: Body) -> Inner {
        Inner {
            sender: [1; 32],
            recipient: [2; 32],
            seq: 7,
            body,
        }
    }

    /// Pads raw `Inner` bytes the way `to_padded` does.
    fn pad(mut buf: Vec<u8>) -> Vec<u8> {
        let bucket = *BUCKETS.iter().find(|&&b| b > buf.len()).unwrap();
        buf.push(0x80);
        buf.resize(bucket, 0);
        buf
    }

    #[test]
    fn adr_025_kinds_roundtrip() {
        let target = Ref {
            id: u64::MAX,
            theirs: true,
        };
        for body in [
            Body::Message {
                id: u64::MAX,
                sent_at: u64::MAX,
                expires_in: Some(u32::MAX),
                reply_to: Some(target),
                text: "a".repeat(MAX_TEXT_BYTES),
            },
            Body::Edit {
                id: 3,
                text: "fixed".into(),
            },
            Body::Delete { id: 3 },
            Body::React {
                target,
                reaction: Some(REACTIONS - 1),
            },
            Body::React {
                target,
                reaction: None,
            },
            Body::Understands { level: UNDERSTANDS },
        ] {
            let inner = with(body);
            assert_eq!(
                Inner::from_padded(&inner.to_padded().unwrap()).unwrap(),
                inner
            );
        }
    }

    #[test]
    fn adr_025_texts_keep_the_limit() {
        for body in [
            Body::Message {
                id: 1,
                sent_at: 0,
                expires_in: None,
                reply_to: None,
                text: "a".repeat(MAX_TEXT_BYTES + 1),
            },
            Body::Edit {
                id: 1,
                text: "a".repeat(MAX_TEXT_BYTES + 1),
            },
        ] {
            assert!(matches!(with(body).to_padded(), Err(Error::TooLarge)));
        }
    }

    #[test]
    fn known_kinds_counts_every_kind_on_the_wire() {
        // The last kind on the wire is `KNOWN_KINDS - 1`, right after the head.
        let last = Body::IntroDecline { intro: 1 };
        let raw = postcard::to_allocvec(&with(last)).unwrap();
        let head = postcard::to_allocvec(&(&[1u8; 32], &[2u8; 32], 7u64)).unwrap();
        assert_eq!(u32::from(raw[head.len()]), KNOWN_KINDS - 3);
        let raw = postcard::to_allocvec(&with(Body::IntroOffer {
            intro: 1,
            nickname: "c".into(),
            invite: Invite {
                version: INVITE_VERSION,
                relay_url: "wss://r/".into(),
                onion_url: None,
                identity: Identity {
                    ed25519: [1; 32],
                    curve25519: [2; 32],
                    signature: [3; 64],
                },
                queue: [4; 16],
                code: [5; 16],
            },
        }))
        .unwrap();
        assert_eq!(u32::from(raw[head.len()]), KNOWN_KINDS - 2);
        let note = Body::Note {
            body: Box::new(Body::Delete { id: 1 }),
        };
        let raw = postcard::to_allocvec(&with(note)).unwrap();
        assert_eq!(u32::from(raw[head.len()]), KNOWN_KINDS - 1);
        assert_eq!(raw[head.len()], 42);
    }

    #[test]
    fn adr_043_parts_alone_take_the_large_bucket() {
        let part = |len: usize| {
            with(Body::FilePart {
                id: 1,
                index: 2,
                data: vec![7; len],
            })
        };
        let full = part(LARGE_CHUNK).to_padded().unwrap();
        assert_eq!(full.len(), LARGE_BUCKET);
        assert_eq!(Inner::from_padded(&full).unwrap(), part(LARGE_CHUNK));
        assert_eq!(part(10).to_padded().unwrap().len(), 512);
        // Anything else in the large bucket, or too big for the small ones.
        let big_text = pad(postcard::to_allocvec(&text(MAX_TEXT_BYTES)).unwrap());
        assert_eq!(big_text.len(), 16384);
        let mut raw = postcard::to_allocvec(&text(20_000)).unwrap();
        raw = pad(raw);
        assert_eq!(raw.len(), LARGE_BUCKET);
        assert!(Inner::from_padded(&raw).is_err());
        let mut unknown = postcard::to_allocvec(&(&[1u8; 32], &[2u8; 32], 7u64)).unwrap();
        unknown.extend(postcard::to_allocvec(&KNOWN_KINDS).unwrap());
        unknown.extend([0; 20_000]);
        assert!(Inner::from_padded(&pad(unknown)).is_err());
        let offer = with(Body::FileOffer {
            id: 1,
            name: "a".repeat(MAX_TEXT_BYTES),
            size: 5,
            expires_in: None,
        });
        assert!(offer.to_padded().is_err());
    }

    #[test]
    fn adr_043_bounds() {
        let offer = |name: &str, size: u32| {
            with(Body::FileOffer {
                id: 1,
                name: name.into(),
                size,
                expires_in: Some(60),
            })
        };
        let ok = offer("scan.pdf", MAX_LARGE_FILE_BYTES);
        assert_eq!(Inner::from_padded(&ok.to_padded().unwrap()).unwrap(), ok);
        for bad in [
            offer("scan.pdf", 0),
            offer("scan.pdf", MAX_LARGE_FILE_BYTES + 1),
            offer("../x", 5),
            offer("a\u{202E}b", 5),
            with(Body::FilePart {
                id: 1,
                index: 0,
                data: vec![],
            }),
            with(Body::FilePart {
                id: 1,
                index: 0,
                data: vec![1; LARGE_CHUNK + 1],
            }),
            with(Body::FilePart {
                id: 1,
                index: large_chunks(MAX_LARGE_FILE_BYTES) as u16,
                data: vec![1],
            }),
            with(Body::FileAck {
                id: 1,
                next: large_chunks(MAX_LARGE_FILE_BYTES) as u16 + 1,
            }),
        ] {
            assert!(bad.to_padded().is_err());
            let raw = pad(postcard::to_allocvec(&bad).unwrap());
            assert!(Inner::from_padded(&raw).is_err());
        }
        assert_eq!(large_chunks(MAX_LARGE_FILE_BYTES), 410);
        assert_eq!(large_chunk_len(LARGE_CHUNK as u32 + 1, 1), Some(1));
        assert_eq!(large_chunk_len(LARGE_CHUNK as u32, 1), None);
    }

    #[test]
    fn adr_046_notes_carry_four_kinds_one_level_deep() {
        let msg = Body::Message {
            id: 1,
            sent_at: 2,
            expires_in: None,
            reply_to: None,
            text: "buy milk".into(),
        };
        for inner in [
            msg.clone(),
            Body::Edit {
                id: 1,
                text: "x".into(),
            },
            Body::Delete { id: 1 },
        ] {
            let note = with(Body::Note {
                body: Box::new(inner),
            });
            assert_eq!(
                Inner::from_padded(&note.to_padded().unwrap()).unwrap(),
                note
            );
        }
        let long = Body::Message {
            id: 1,
            sent_at: 2,
            expires_in: None,
            reply_to: None,
            text: "a".repeat(MAX_TEXT_BYTES + 1),
        };
        for bad in [
            Body::Note {
                body: Box::new(Body::Note {
                    body: Box::new(msg.clone()),
                }),
            },
            Body::Note {
                body: Box::new(Body::Goodbye),
            },
            Body::Note {
                body: Box::new(long),
            },
        ] {
            assert!(with(bad.clone()).to_padded().is_err());
            let raw = pad(postcard::to_allocvec(&with(bad)).unwrap());
            assert!(Inner::from_padded(&raw).is_err());
        }
        let group = GroupInner {
            sender: [1; 32],
            group: [3; 16],
            body: Body::Note {
                body: Box::new(msg.clone()),
            },
        };
        assert!(group.to_padded().is_err());
        let copy = Body::Copy {
            contact: [4; 32],
            body: Box::new(Body::Note {
                body: Box::new(msg),
            }),
        };
        assert!(with(copy).to_padded().is_err());
    }

    #[test]
    fn adr_045_introductions_are_checked_and_direct() {
        let invite = Invite {
            version: INVITE_VERSION,
            relay_url: "wss://relay.example/".into(),
            onion_url: None,
            identity: Identity {
                ed25519: [1; 32],
                curve25519: [2; 32],
                signature: [3; 64],
            },
            queue: [4; 16],
            code: [5; 16],
        };
        let ok = [
            Body::IntroAsk {
                intro: 9,
                nickname: "bob".into(),
                key: [7; 32],
            },
            Body::IntroGrant {
                intro: 9,
                invite: invite.clone(),
            },
            Body::IntroDecline { intro: 9 },
            Body::IntroOffer {
                intro: 9,
                nickname: "carol".into(),
                invite: invite.clone(),
            },
        ];
        for body in ok.clone() {
            let inner = with(body.clone());
            assert_eq!(
                Inner::from_padded(&inner.to_padded().unwrap()).unwrap(),
                inner
            );
            let group = GroupInner {
                sender: [1; 32],
                group: [3; 16],
                body: body.clone(),
            };
            assert!(group.to_padded().is_err());
            let copy = Body::Copy {
                contact: [4; 32],
                body: Box::new(body),
            };
            assert!(with(copy).to_padded().is_err());
        }
        let mut http = invite.clone();
        http.relay_url = format!("wss://{}/", "a".repeat(MAX_RELAY_URL));
        let mut old = invite.clone();
        old.version = INVITE_VERSION - 1;
        for bad in [
            Body::IntroAsk {
                intro: 1,
                nickname: "".into(),
                key: [7; 32],
            },
            Body::IntroAsk {
                intro: 1,
                nickname: "a\nb".into(),
                key: [7; 32],
            },
            Body::IntroGrant {
                intro: 1,
                invite: http,
            },
            Body::IntroOffer {
                intro: 1,
                nickname: "carol".into(),
                invite: old,
            },
        ] {
            assert!(with(bad.clone()).to_padded().is_err());
            let raw = pad(postcard::to_allocvec(&with(bad)).unwrap());
            assert!(Inner::from_padded(&raw).is_err());
        }
    }

    #[test]
    fn adr_043_never_in_groups_or_copies() {
        for body in [
            Body::FileOffer {
                id: 1,
                name: "a".into(),
                size: 5,
                expires_in: None,
            },
            Body::FileWant { id: 1 },
            Body::FileAck { id: 1, next: 1 },
            Body::FileStop { id: 1 },
            Body::FilePart {
                id: 1,
                index: 0,
                data: vec![1],
            },
        ] {
            let group = GroupInner {
                sender: [1; 32],
                group: [3; 16],
                body: body.clone(),
            };
            assert!(group.to_padded().is_err());
            let copy = Body::Copy {
                contact: [4; 32],
                body: Box::new(body),
            };
            assert!(with(copy).to_padded().is_err());
        }
    }

    #[test]
    fn receipts_are_direct_and_bounded() {
        let receipt = |n: usize, m: usize| Body::Receipt {
            messages: vec![7; n],
            files: vec![9; m],
            read: true,
        };
        let ok = with(receipt(MAX_RECEIPT_IDS, MAX_RECEIPT_IDS));
        assert_eq!(Inner::from_padded(&ok.to_padded().unwrap()).unwrap(), ok);
        for body in [
            receipt(MAX_RECEIPT_IDS + 1, 0),
            receipt(0, MAX_RECEIPT_IDS + 1),
        ] {
            assert!(matches!(
                with(body.clone()).to_padded(),
                Err(Error::TooLarge)
            ));
            let raw = pad(postcard::to_allocvec(&with(body)).unwrap());
            assert!(Inner::from_padded(&raw).is_err());
        }
        let group = GroupInner {
            sender: [1; 32],
            group: [2; 16],
            body: receipt(1, 0),
        };
        let copy = with(Body::Copy {
            contact: [5; 32],
            body: Box::new(receipt(1, 0)),
        });
        assert!(group.to_padded().is_err());
        assert!(copy.to_padded().is_err());
        assert!(GroupInner::from_padded(&pad(postcard::to_allocvec(&group).unwrap())).is_err());
    }

    #[test]
    fn heartbeats_are_only_direct() {
        let pulse = Body::Heartbeat { sent_at: 1 };
        let direct = with(pulse.clone());
        assert_eq!(
            Inner::from_padded(&direct.to_padded().unwrap()).unwrap(),
            direct
        );
        let group = GroupInner {
            sender: [1; 32],
            group: [2; 16],
            body: pulse.clone(),
        };
        let copy = with(Body::Copy {
            contact: [5; 32],
            body: Box::new(pulse),
        });
        assert!(group.to_padded().is_err());
        assert!(copy.to_padded().is_err());
        assert!(GroupInner::from_padded(&pad(postcard::to_allocvec(&group).unwrap())).is_err());
        assert!(Inner::from_padded(&pad(postcard::to_allocvec(&copy).unwrap())).is_err());
    }

    #[test]
    fn a_newer_kind_is_unknown_with_its_seq() {
        let mut raw = postcard::to_allocvec(&(&[1u8; 32], &[2u8; 32], 7u64)).unwrap();
        raw.extend(postcard::to_allocvec(&KNOWN_KINDS).unwrap());
        raw.extend(b"whatever a newer version puts here");
        assert_eq!(
            Inner::from_padded(&pad(raw.clone())).unwrap(),
            with(Body::Unknown)
        );
        // A kind number past one byte as well.
        raw.truncate(73);
        raw.extend(postcard::to_allocvec(&100_000u32).unwrap());
        assert_eq!(Inner::from_padded(&pad(raw)).unwrap(), with(Body::Unknown));
    }

    #[test]
    fn a_broken_known_kind_stays_malformed() {
        let mut raw = postcard::to_allocvec(&with(Body::Delete { id: 1 })).unwrap();
        raw.extend(b"trailing");
        assert!(matches!(
            Inner::from_padded(&pad(raw)),
            Err(Error::Malformed)
        ));
        // A head cut short is no unknown kind either.
        assert!(matches!(
            Inner::from_padded(&pad(vec![1; 40])),
            Err(Error::Malformed)
        ));
    }

    #[test]
    fn unknown_never_goes_on_the_wire() {
        assert!(with(Body::Unknown).to_padded().is_err());
    }

    #[test]
    fn padding_roundtrip_hits_every_bucket() {
        for (len, bucket) in [(0, 512), (1000, 4096), (MAX_TEXT_BYTES, 16384)] {
            let padded = text(len).to_padded().unwrap();
            assert_eq!(padded.len(), bucket);
            assert_eq!(Inner::from_padded(&padded).unwrap(), text(len));
        }
    }

    #[test]
    fn contact_request_roundtrip() {
        let identity = Identity {
            ed25519: [1; 32],
            curve25519: [2; 32],
            signature: [3; 64],
        };
        let mut signature = [0; 64];
        signature[63] = 9;
        let inner = Inner {
            body: Body::ContactRequest {
                identity: Identity {
                    signature,
                    ..identity
                },
                nickname: "Ёжик 🦔".into(),
                note: "hi".into(),
            },
            ..text(0)
        };
        assert_eq!(
            Inner::from_padded(&inner.to_padded().unwrap()).unwrap(),
            inner
        );
        for (bad, err) in [
            ("evil\u{1b}[2J", Error::Malformed),
            ("  ", Error::Malformed),
            (&*"я".repeat(33), Error::TooLarge),
        ] {
            let inner = Inner {
                body: Body::ContactAccept {
                    nickname: bad.into(),
                },
                ..text(0)
            };
            assert_eq!(inner.to_padded(), Err(err));
        }
        let long_note = Inner {
            body: Body::ContactRequest {
                identity: Identity {
                    signature,
                    ..identity
                },
                nickname: "x".into(),
                note: "n".repeat(MAX_NOTE_BYTES + 1),
            },
            ..text(0)
        };
        assert_eq!(long_note.to_padded(), Err(Error::TooLarge));
        let mut v2 = Inner {
            body: Body::ContactRequestV2 {
                identity: identity.clone(),
                nickname: "x".into(),
                note: "hi".into(),
                code: [5; 16],
            },
            ..text(0)
        };
        assert_eq!(Inner::from_padded(&v2.to_padded().unwrap()).unwrap(), v2);
        if let Body::ContactRequestV2 { note, .. } = &mut v2.body {
            *note = "n".repeat(MAX_NOTE_BYTES + 1);
        }
        assert_eq!(v2.to_padded(), Err(Error::TooLarge));
        let bye = Inner {
            body: Body::Goodbye,
            ..text(0)
        };
        let padded = bye.to_padded().unwrap();
        assert_eq!(padded.len(), 512);
        assert_eq!(Inner::from_padded(&padded).unwrap(), bye);
        let key = SignedKey {
            key: [4; 32],
            signature,
        };
        let bytes = postcard::to_allocvec(&key).unwrap();
        assert_eq!(bytes.len(), 96);
        assert_eq!(postcard::from_bytes::<SignedKey>(&bytes).unwrap(), key);
    }

    fn chunk(name: &str, size: u32, index: u16, len: usize) -> Inner {
        Inner {
            seq: u64::MAX,
            body: Body::File {
                id: u32::MAX,
                name: name.into(),
                size,
                index,
                expires_in: Some(u32::MAX),
                data: vec![7; len],
            },
            ..text(0)
        }
    }

    #[test]
    fn file_chunks_roundtrip_and_check_limits() {
        // The largest header still fits the largest bucket.
        let full = chunk(
            &"n".repeat(MAX_FILE_NAME_BYTES),
            MAX_FILE_BYTES,
            0,
            FILE_CHUNK,
        );
        let padded = full.to_padded().unwrap();
        assert_eq!(padded.len(), 16384);
        assert_eq!(Inner::from_padded(&padded).unwrap(), full);
        let last = file_chunks(MAX_FILE_BYTES) - 1;
        assert_eq!(file_chunks(MAX_FILE_BYTES), 66);
        let tail = chunk(
            "a.pdf",
            MAX_FILE_BYTES,
            last as u16,
            (1 << 20) - last * FILE_CHUNK,
        );
        assert_eq!(
            Inner::from_padded(&tail.to_padded().unwrap()).unwrap(),
            tail
        );
        assert_eq!(file_chunks(1), 1);
        assert_eq!(chunk_len(FILE_CHUNK as u32, 1), None);

        for (bad, err) in [
            (chunk("a", 0, 0, 0), Error::TooLarge),
            (
                chunk("a", MAX_FILE_BYTES + 1, 0, FILE_CHUNK),
                Error::TooLarge,
            ),
            (chunk("a", 10, 1, 0), Error::Malformed),
            (chunk("a", 10, 0, 9), Error::Malformed),
            (chunk("a", 10, 0, 11), Error::Malformed),
            (chunk("a", 20_000, 0, 4000), Error::Malformed),
            (chunk("a", 20_000, 1, FILE_CHUNK), Error::Malformed),
            (
                chunk(&"n".repeat(MAX_FILE_NAME_BYTES + 1), 1, 0, 1),
                Error::TooLarge,
            ),
        ] {
            assert_eq!(bad.to_padded(), Err(err));
        }
        for name in [
            "",
            " ",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "a\0b",
            "evil\u{1b}[2J",
            "x\u{202E}fdp.exe",
        ] {
            assert_eq!(
                chunk(name, 1, 0, 1).to_padded(),
                Err(Error::Malformed),
                "{name:?}"
            );
        }
        assert!(chunk("отчёт v2.final.pdf", 1, 0, 1).to_padded().is_ok());
    }

    #[test]
    fn file_chunk_rejected_on_the_wire() {
        // Same check on decode: a peer that skips `to_padded` gets nowhere.
        let bad = chunk("../x", 1, 0, 1);
        let mut buf = postcard::to_allocvec(&bad).unwrap();
        buf.push(0x80);
        buf.resize(512, 0);
        assert_eq!(Inner::from_padded(&buf), Err(Error::Malformed));
    }

    #[test]
    fn frames_roundtrip_and_check_limits() {
        let env = Envelope {
            version: VERSION,
            olm_type: 0,
            body: vec![1; 10],
        };
        let send = ClientFrame::Send {
            client_id: 7,
            send_id: [5; 16],
            envelope: env.clone(),
        };
        assert_eq!(ClientFrame::decode(&send.encode()).unwrap(), send);
        let deliver = ServerFrame::Deliver {
            recv_id: [4; 16],
            relay_id: 1,
            envelope: env.clone(),
        };
        assert_eq!(ServerFrame::decode(&deliver.encode()).unwrap(), deliver);
        let create = ClientFrame::CreateQueue;
        assert_eq!(ClientFrame::decode(&create.encode()).unwrap(), create);
        let mut trailing = create.encode();
        trailing.push(0);
        assert_eq!(ClientFrame::decode(&trailing), Err(Error::Malformed));
        let created = ServerFrame::QueueCreated {
            recv_id: [1; 16],
            send_id: [2; 16],
            token: [3; 32],
        };
        assert_eq!(ServerFrame::decode(&created.encode()).unwrap(), created);
        // ADR-027: group frames.
        for f in [
            ClientFrame::CreateGroup,
            ClientFrame::AddReader {
                group_id: [1; 16],
                token: [2; 32],
            },
            ClientFrame::RemoveReader {
                group_id: [1; 16],
                token: [2; 32],
                recv_id: [3; 16],
            },
            ClientFrame::DeleteGroup {
                group_id: [1; 16],
                token: [2; 32],
            },
            ClientFrame::RemoveReaderAt {
                group_id: [1; 16],
                token: [2; 32],
                send_id: [4; 16],
            },
        ] {
            assert_eq!(ClientFrame::decode(&f.encode()).unwrap(), f);
        }
        for f in [
            ServerFrame::GroupCreated {
                group_id: [1; 16],
                send_id: [2; 16],
                token: [3; 32],
            },
            ServerFrame::ReaderAdded {
                group_id: [1; 16],
                recv_id: [2; 16],
                send_id: [3; 16],
                token: [4; 32],
            },
            ServerFrame::ReaderRemoved { recv_id: [2; 16] },
            ServerFrame::GroupDeleted { group_id: [1; 16] },
        ] {
            assert_eq!(ServerFrame::decode(&f.encode()).unwrap(), f);
        }
        let push = ClientFrame::SetPush {
            recv_id: [1; 16],
            token: [3; 32],
            endpoint: Some("https://ntfy.sh/upAbc".into()),
        };
        assert_eq!(ClientFrame::decode(&push.encode()).unwrap(), push);
        let long = ClientFrame::SetPush {
            recv_id: [1; 16],
            token: [3; 32],
            endpoint: Some("x".repeat(MAX_PUSH_URL_BYTES + 1)),
        };
        assert_eq!(ClientFrame::decode(&long.encode()), Err(Error::TooLarge));
        let report = ClientFrame::Report {
            recv_id: [1; 16],
            token: [3; 32],
            text: "x".repeat(MAX_TEXT_BYTES + 1),
        };
        assert_eq!(ClientFrame::decode(&report.encode()), Err(Error::TooLarge));
        assert_eq!(
            ServerFrame::decode(&ServerFrame::PushSet.encode()).unwrap(),
            ServerFrame::PushSet
        );

        let old = ClientFrame::Send {
            client_id: 7,
            send_id: [5; 16],
            envelope: Envelope { version: 9, ..env },
        };
        assert_eq!(
            ClientFrame::decode(&old.encode()),
            Err(Error::UnsupportedVersion)
        );
        let key = SignedKey {
            key: [0; 32],
            signature: [0; 64],
        };
        let many = ClientFrame::PublishKeys {
            recv_id: [1; 16],
            token: [3; 32],
            one_time: vec![key; MAX_KEYS_PER_PUBLISH + 1],
            fallback: None,
        };
        assert_eq!(ClientFrame::decode(&many.encode()), Err(Error::TooLarge));
        assert_eq!(
            ClientFrame::decode(&vec![0; MAX_FRAME_BYTES + 1]),
            Err(Error::TooLarge)
        );
    }

    #[test]
    fn version_1_client_reads_the_refusal() {
        // A version 1 client opens with `Hello { version: 1 }` and can decode
        // only its own `ServerFrame`: `Error` must keep variant 7 and
        // `UnsupportedVersion` variant 0, or it sees garbage instead of
        // "please update".
        assert_eq!(
            ClientFrame::Hello { version: 1 }.encode(),
            [0, 1],
            "Hello is variant 0"
        );
        let refusal = ServerFrame::Error {
            client_id: None,
            code: ErrorCode::UnsupportedVersion,
        };
        assert_eq!(refusal.encode(), [7, 0, 0]);
    }

    fn devices(n: u8) -> DeviceList {
        DeviceList {
            identity: [1; 32],
            no: 1,
            devices: (1..=n)
                .map(|i| Device {
                    ed25519: [i; 32],
                    curve25519: [i; 32],
                    proof: [0; 64],
                })
                .collect(),
            signature: [0; 64],
        }
    }

    fn queue(device: u8) -> DeviceQueue {
        DeviceQueue {
            device: [device; 32],
            relay_url: "wss://relay.example.org/".into(),
            onion_url: None,
            send_id: [device; 16],
        }
    }

    #[test]
    fn adr_028_kinds_roundtrip() {
        let identity = Identity {
            ed25519: [1; 32],
            curve25519: [2; 32],
            signature: [3; 64],
        };
        for body in [
            Body::DeviceLink {
                code: [9; 16],
                identity: identity.clone(),
            },
            Body::DeviceLinked { proof: [4; 64] },
            Body::LinkedContact {
                contact: LinkedContact {
                    identity,
                    nickname: "bob".into(),
                    petname: Some("Bob from work".into()),
                    verified: true,
                    understands: UNDERSTANDS,
                    devices: Some(devices(MAX_DEVICES as u8)),
                    queues: (1..=MAX_DEVICES as u8).map(queue).collect(),
                },
            },
            Body::OwnDevices {
                list: devices(2),
                labels: vec![([1; 32], "phone".into()), ([2; 32], String::new())],
                nickname: "alice".into(),
            },
            Body::Devices {
                list: devices(3),
                queues: vec![queue(1), queue(2), queue(3)],
            },
            Body::OwnQueue {
                contact: [5; 32],
                queue: queue(2),
            },
            Body::SiblingQueue {
                device: [6; 32],
                queue: queue(3),
            },
            Body::GroupDevices {
                group: [7; 16],
                member: [1; 32],
                list: devices(MAX_DEVICES as u8),
                readers: (2..=MAX_DEVICES as u8)
                    .map(|d| ([d; 32], [d; 16]))
                    .collect(),
            },
            Body::OwnGroupJoin { group: [7; 16] },
            Body::Copy {
                contact: [8; 32],
                body: Box::new(Body::OwnGroupJoin { group: [7; 16] }),
            },
        ] {
            let inner = with(body);
            assert_eq!(
                Inner::from_padded(&inner.to_padded().unwrap()).unwrap(),
                inner
            );
        }
    }

    #[test]
    fn adr_028_a_bad_device_list_is_refused() {
        let bad = |list: DeviceList| {
            with(Body::Devices {
                list,
                queues: Vec::new(),
            })
            .to_padded()
            .unwrap_err()
        };
        assert_eq!(bad(devices(0)), Error::Malformed);
        assert_eq!(bad(devices(MAX_DEVICES as u8 + 1)), Error::TooLarge);
        // The first device is the identity itself.
        let mut not_first = devices(2);
        not_first.devices.swap(0, 1);
        assert_eq!(bad(not_first), Error::Malformed);
        let mut twice = devices(2);
        twice.devices[1] = twice.devices[0];
        assert_eq!(bad(twice), Error::Malformed);
    }

    #[test]
    fn adr_028_labels_queues_and_names_keep_their_limits() {
        let own = |labels: Vec<(Ed25519Key, String)>| {
            with(Body::OwnDevices {
                list: devices(1),
                labels,
                nickname: "alice".into(),
            })
            .to_padded()
        };
        assert_eq!(
            own(vec![([1; 32], "a\u{1b}[2J".into())]),
            Err(Error::Malformed)
        );
        assert_eq!(
            own(vec![([1; 32], "a".repeat(MAX_NICKNAME_BYTES + 1))]),
            Err(Error::TooLarge)
        );
        assert_eq!(
            own(vec![([1; 32], String::new()); MAX_DEVICES + 1]),
            Err(Error::TooLarge)
        );
        let too_many = with(Body::Devices {
            list: devices(1),
            queues: (0..=MAX_DEVICES as u8).map(queue).collect(),
        });
        assert_eq!(too_many.to_padded(), Err(Error::TooLarge));
        let many_readers = with(Body::GroupDevices {
            group: [7; 16],
            member: [1; 32],
            list: devices(1),
            readers: vec![([2; 32], [2; 16]); MAX_DEVICES + 1],
        });
        assert_eq!(many_readers.to_padded(), Err(Error::TooLarge));
        let bad_list = with(Body::GroupDevices {
            group: [7; 16],
            member: [1; 32],
            list: devices(0),
            readers: Vec::new(),
        });
        assert_eq!(bad_list.to_padded(), Err(Error::Malformed));
        let long_url = with(Body::Devices {
            list: devices(1),
            queues: vec![DeviceQueue {
                relay_url: "x".repeat(MAX_RELAY_URL + 1),
                ..queue(1)
            }],
        });
        assert_eq!(long_url.to_padded(), Err(Error::TooLarge));
        let contact = |petname: Option<String>| {
            with(Body::LinkedContact {
                contact: LinkedContact {
                    identity: Identity {
                        ed25519: [1; 32],
                        curve25519: [2; 32],
                        signature: [3; 64],
                    },
                    nickname: "bob".into(),
                    petname,
                    verified: false,
                    understands: 2,
                    devices: None,
                    queues: vec![queue(1)],
                },
            })
            .to_padded()
        };
        assert!(contact(None).is_ok());
        assert_eq!(contact(Some("\n".into())), Err(Error::Malformed));
    }

    #[test]
    fn adr_028_a_link_code_and_an_invite_are_never_taken_for_each_other() {
        let invite = Invite {
            version: INVITE_VERSION,
            relay_url: "wss://relay.example.org/".into(),
            onion_url: None,
            identity: Identity {
                ed25519: [1; 32],
                curve25519: [2; 32],
                signature: [3; 64],
            },
            queue: [4; 16],
            code: [9; 16],
        };
        let link = invite.encode_link();
        assert!(link.starts_with("termchat-link:"));
        assert_eq!(Invite::decode_link(&format!(" {link}\n")).unwrap(), invite);
        assert_eq!(Invite::decode(&link), Err(Error::Malformed));
        assert_eq!(Invite::decode_link(&invite.encode()), Err(Error::Malformed));
    }

    #[test]
    fn invite_roundtrip_and_old_versions_refused() {
        let invite = Invite {
            version: INVITE_VERSION,
            relay_url: "wss://relay.example.org/".into(),
            onion_url: Some(
                "ws://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuv.onion/".into(),
            ),
            identity: Identity {
                ed25519: [1; 32],
                curve25519: [2; 32],
                signature: [3; 64],
            },
            queue: [4; 16],
            code: [9; 16],
        };
        let s = invite.encode();
        assert!(s.starts_with("termchat:") && s.len() < 400, "{s}");
        assert_eq!(Invite::decode(&format!("  {s}\n")).unwrap(), invite);
        assert_eq!(Invite::decode("termchat:!!!"), Err(Error::Malformed));
        assert_eq!(Invite::decode(&s[1..]), Err(Error::Malformed));

        let raw = URL_SAFE_NO_PAD.decode(&s[INVITE_PREFIX.len()..]).unwrap();
        let with = |b: &[u8]| format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(b));
        for v in [1, 2, 4] {
            let mut old = raw.clone();
            old[0] = v;
            assert_eq!(
                Invite::decode(&with(&old)),
                Err(Error::UnsupportedVersion),
                "version {v}"
            );
        }
        let mut trailing = raw.clone();
        trailing.push(0);
        assert_eq!(Invite::decode(&with(&trailing)), Err(Error::Malformed));
        let long = Invite {
            relay_url: "x".repeat(MAX_RELAY_URL + 1),
            ..invite
        };
        assert_eq!(Invite::decode(&long.encode()), Err(Error::TooLarge));
    }

    #[test]
    fn queue_offer_roundtrip_and_url_limits() {
        let offer = Inner {
            body: Body::QueueOffer {
                relay_url: "wss://relay.example.org/".into(),
                onion_url: None,
                send_id: [8; 16],
            },
            ..text(0)
        };
        let padded = offer.to_padded().unwrap();
        assert_eq!(padded.len(), 512);
        assert_eq!(Inner::from_padded(&padded).unwrap(), offer);
        for (relay_url, onion_url) in [
            ("x".repeat(MAX_RELAY_URL + 1), None),
            ("x".into(), Some("y".repeat(MAX_RELAY_URL + 1))),
        ] {
            let bad = Inner {
                body: Body::QueueOffer {
                    relay_url,
                    onion_url,
                    send_id: [8; 16],
                },
                ..text(0)
            };
            assert_eq!(bad.to_padded(), Err(Error::TooLarge));
            // The same check on decode, for a peer that skips `to_padded`.
            let mut buf = postcard::to_allocvec(&bad).unwrap();
            buf.push(0x80);
            buf.resize(4096, 0);
            assert_eq!(Inner::from_padded(&buf), Err(Error::TooLarge));
        }
    }

    #[test]
    fn text_over_limit_is_rejected() {
        assert_eq!(text(MAX_TEXT_BYTES + 1).to_padded(), Err(Error::TooLarge));
    }

    #[test]
    fn bad_padding_is_rejected() {
        let mut padded = text(5).to_padded().unwrap();
        assert_eq!(Inner::from_padded(&padded[..511]), Err(Error::Malformed));
        let last = padded.len() - 1;
        padded[last] = 1;
        assert_eq!(Inner::from_padded(&padded), Err(Error::Malformed));
        assert_eq!(Inner::from_padded(&[0; 512]), Err(Error::Malformed));
    }

    #[test]
    fn envelope_checks_version_size_and_trailing_bytes() {
        let env = Envelope {
            version: VERSION,
            olm_type: 1,
            body: vec![9; 100],
        };
        assert_eq!(Envelope::decode(&env.encode()).unwrap(), env);

        let mut bytes = env.encode();
        bytes.push(0);
        assert_eq!(Envelope::decode(&bytes), Err(Error::Malformed));

        // Overlong varint for the version.
        let mut overlong = vec![0x82, 0x00];
        overlong.extend_from_slice(&env.encode()[1..]);
        assert_eq!(Envelope::decode(&overlong), Err(Error::Malformed));

        let old = Envelope {
            version: 0,
            ..env.clone()
        };
        assert_eq!(
            Envelope::decode(&old.encode()),
            Err(Error::UnsupportedVersion)
        );

        let big = Envelope {
            body: vec![0; MAX_BODY_BYTES + 1],
            ..env
        };
        assert_eq!(Envelope::decode(&big.encode()), Err(Error::TooLarge));
    }

    fn admin_keys() -> Body {
        Body::GroupAdminKeys {
            group: [7; 16],
            roster_no: 3,
            grant: AdminGrant {
                queue: [8; 16],
                token: [9; 32],
            },
        }
    }

    fn admin_list(no: u64, admins: &[u8]) -> AdminList {
        AdminList {
            group: [7; 16],
            no,
            signer: [1; 32],
            admins: admins.iter().map(|&a| [a; 32]).collect(),
            resign: None,
            signature: [3; 64],
        }
    }

    fn admins(chain: Vec<AdminList>) -> Result<Vec<u8>, Error> {
        with(Body::GroupAdmins {
            group: [7; 16],
            chain,
        })
        .to_padded()
    }

    #[test]
    fn adr_042_admin_kinds_round_trip() {
        let mut handed_over = admin_list(5, &[2, 3]);
        handed_over.resign = Some(Resign {
            roster_no: 4,
            signature: [4; 64],
        });
        let full: Vec<u8> = (1..=MAX_GROUP_ADMINS as u8).collect();
        let chain: Vec<AdminList> = (1..=MAX_ADMIN_CHAIN as u64)
            .map(|no| admin_list(no, &full))
            .collect();
        for body in [
            Body::GroupAdmins {
                group: [7; 16],
                chain: vec![admin_list(4, &[1, 2]), handed_over],
            },
            Body::GroupAdmins {
                group: [7; 16],
                chain,
            },
            Body::GroupResign {
                group: [7; 16],
                resign: Resign {
                    roster_no: 4,
                    signature: [4; 64],
                },
            },
            admin_keys(),
        ] {
            let inner = with(body);
            assert_eq!(
                Inner::from_padded(&inner.to_padded().unwrap()).unwrap(),
                inner
            );
        }
        let group = GroupInner {
            sender: [1; 32],
            group: [7; 16],
            body: Body::GroupResign {
                group: [7; 16],
                resign: Resign {
                    roster_no: 4,
                    signature: [4; 64],
                },
            },
        };
        assert_eq!(
            GroupInner::from_padded(&group.to_padded().unwrap()).unwrap(),
            group
        );
    }

    #[test]
    fn review_2026_11d_a_roster_names_each_key_once() {
        let member = |k: u8| RosterMember {
            ed25519: [k; 32],
            curve25519: [k; 32],
            nickname: "m".into(),
            reader: [k; 16],
        };
        let roster = |members: Vec<RosterMember>| {
            with(Body::GroupRoster {
                roster: Roster {
                    group: [7; 16],
                    no: 2,
                    admin: [1; 32],
                    name: "g".into(),
                    members,
                    expires_in: None,
                    signature: [0; 64],
                },
            })
            .to_padded()
        };
        assert!(roster(vec![member(1), member(2)]).is_ok());
        let mut twice = member(1);
        twice.reader = [9; 16];
        assert_eq!(
            roster(vec![member(1), member(2), twice]),
            Err(Error::Malformed)
        );
    }

    #[test]
    fn adr_042_a_bad_admin_chain_is_refused() {
        assert_eq!(admins(vec![]), Err(Error::Malformed));
        assert_eq!(admins(vec![admin_list(1, &[])]), Err(Error::Malformed));
        assert_eq!(
            admins(vec![admin_list(1, &[1, 2, 1])]),
            Err(Error::Malformed)
        );
        let five: Vec<u8> = (1..=MAX_GROUP_ADMINS as u8 + 1).collect();
        assert_eq!(admins(vec![admin_list(1, &five)]), Err(Error::TooLarge));
        let long = (1..=MAX_ADMIN_CHAIN as u64 + 1)
            .map(|no| admin_list(no, &[1]))
            .collect();
        assert_eq!(admins(long), Err(Error::TooLarge));
        // Oldest first, each list newer than the one before.
        let back = vec![admin_list(2, &[1]), admin_list(1, &[1])];
        assert_eq!(admins(back), Err(Error::Malformed));
        let same = vec![admin_list(2, &[1]), admin_list(2, &[1, 2])];
        assert_eq!(admins(same), Err(Error::Malformed));
        // Every list is of the group the message names.
        let mut other = admin_list(2, &[1]);
        other.group = [6; 16];
        assert_eq!(
            admins(vec![admin_list(1, &[1]), other]),
            Err(Error::Malformed)
        );
    }

    #[test]
    fn adr_042_the_admin_token_stays_between_two_devices() {
        // Inside the group every member would read it.
        let group = GroupInner {
            sender: [1; 32],
            group: [7; 16],
            body: admin_keys(),
        };
        assert_eq!(group.to_padded(), Err(Error::Malformed));
        let raw = pad(postcard::to_allocvec(&group).unwrap());
        assert_eq!(GroupInner::from_padded(&raw), Err(Error::Malformed));
        // Nor to one's own linked devices.
        let copy = with(Body::Copy {
            contact: [5; 32],
            body: Box::new(admin_keys()),
        });
        assert_eq!(copy.to_padded(), Err(Error::Malformed));
        // And it does not show in logs.
        let shown = format!("{:?}", admin_keys());
        assert!(shown.ends_with("grant: AdminGrant { .. } }"), "{shown}");
    }
}
