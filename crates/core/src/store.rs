//! Encrypted state file (ADR-002, ADR-003).
//!
//! Layout, all integers little-endian:
//! `version u16 | m_kib u32 | t u32 | p u32 | slot | slot | nonce 24 | body`
//! where `slot = salt 16 | nonce 24 | wrapped DEK 32 + tag 16`.
//! The body is encrypted with a random data key (DEK) and the whole header as AAD.
//! One slot wraps the DEK under the password; the other is random bytes
//! (M4: wraps a duress marker instead). Order of slots is random.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::Error;

const FORMAT_VERSION: u16 = 1;
const SALT: usize = 16;
const NONCE: usize = 24;
const WRAPPED: usize = 32 + 16;
const SLOT: usize = SALT + NONCE + WRAPPED;
const HEADER: usize = 2 + 4 * 3 + 2 * SLOT;

/// Argon2id cost, stored in the header so it can be raised later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// ADR-002: 64 MiB, 3 passes, 1 lane.
    pub const DEFAULT: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };
}

/// An unlocked state file: its header and data key. Kept in memory while the
/// app is unlocked so saving does not rerun Argon2. No `Debug`: holds the key.
pub struct FileKey {
    header: Vec<u8>,
    dek: Zeroizing<Vec<u8>>,
}

/// New file key: random DEK wrapped under `password` in one of two slots.
pub fn create(password: &[u8], params: KdfParams) -> Result<FileKey, Error> {
    let dek = Zeroizing::new(random::<32>()?.to_vec());
    let mut slots = [random::<SLOT>()?, random::<SLOT>()?];
    let real = usize::from(random::<1>()?[0] & 1);
    {
        let (salt, rest) = slots[real].split_at_mut(SALT);
        let (nonce, wrapped) = rest.split_at_mut(NONCE);
        let kek = derive(password, salt, params)?;
        wrapped.copy_from_slice(
            &aead(kek.as_slice())?
                .encrypt(&XNonce::from(arr(nonce)), dek.as_slice())
                .map_err(|_| Error::CorruptState)?,
        );
    }
    let mut header = Vec::with_capacity(HEADER);
    header.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    for v in [params.m_kib, params.t, params.p] {
        header.extend_from_slice(&v.to_le_bytes());
    }
    header.extend_from_slice(&slots[0]);
    header.extend_from_slice(&slots[1]);
    Ok(FileKey { header, dek })
}

/// Whole file bytes: header, fresh nonce, body encrypted with the header as AAD.
pub fn seal(key: &FileKey, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    let nonce = random::<NONCE>()?;
    let body = aead(key.dek.as_slice())?
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &key.header,
            },
        )
        .map_err(|_| Error::CorruptState)?;
    Ok([key.header.as_slice(), &nonce, &body].concat())
}

pub fn open(password: &[u8], file: &[u8]) -> Result<(FileKey, Zeroizing<Vec<u8>>), Error> {
    if file.len() < HEADER + NONCE + 16 {
        return Err(Error::CorruptState);
    }
    let (header, rest) = file.split_at(HEADER);
    let (nonce, body) = rest.split_at(NONCE);
    if header[..2] != FORMAT_VERSION.to_le_bytes() {
        return Err(Error::CorruptState);
    }
    let le = |i: usize| u32::from_le_bytes(arr(&header[i..i + 4]));
    let params = KdfParams {
        m_kib: le(2),
        t: le(6),
        p: le(10),
    };
    // Untrusted file: do not let it make us allocate gigabytes or spin for minutes.
    if params.m_kib > 1024 * 1024 || params.t > 16 || params.p > 8 {
        return Err(Error::CorruptState);
    }

    // Derive for both slots before deciding: equal time for either password (ADR-003).
    let slots = &header[HEADER - 2 * SLOT..];
    let mut deks = Vec::with_capacity(2);
    for slot in slots.as_chunks::<SLOT>().0 {
        let (salt, rest) = slot.split_at(SALT);
        let (slot_nonce, wrapped) = rest.split_at(NONCE);
        let kek = derive(password, salt, params)?;
        deks.push(
            aead(kek.as_slice())?
                .decrypt(&XNonce::from(arr(slot_nonce)), wrapped)
                .ok()
                .map(Zeroizing::new),
        );
    }
    for dek in deks.into_iter().flatten() {
        if let Ok(plain) = aead(dek.as_slice())?.decrypt(
            &XNonce::from(arr(nonce)),
            Payload {
                msg: body,
                aad: header,
            },
        ) {
            let key = FileKey {
                header: header.to_vec(),
                dek,
            };
            return Ok((key, Zeroizing::new(plain)));
        }
    }
    Err(Error::WrongPassword)
}

fn derive(password: &[u8], salt: &[u8], p: KdfParams) -> Result<Zeroizing<[u8; 32]>, Error> {
    let params = Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|_| Error::CorruptState)?;
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, key.as_mut_slice())
        .map_err(|_| Error::CorruptState)?;
    Ok(key)
}

fn aead(key: &[u8]) -> Result<XChaCha20Poly1305, Error> {
    XChaCha20Poly1305::new_from_slice(key).map_err(|_| Error::CorruptState)
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut buf = [0; N];
    getrandom::fill(&mut buf).map_err(|_| Error::Rng)?;
    Ok(buf)
}

/// Slices here always have the length the layout guarantees.
fn arr<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut a = [0; N];
    a.copy_from_slice(s);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams {
        m_kib: 64,
        t: 1,
        p: 1,
    };

    #[test]
    fn roundtrip_and_wrong_password() {
        let key = create(b"correct horse", FAST).unwrap();
        let file = seal(&key, b"state").unwrap();
        let (key, plain) = open(b"correct horse", &file).unwrap();
        assert_eq!(plain.as_slice(), b"state");
        assert!(matches!(open(b"wrong", &file), Err(Error::WrongPassword)));
        // Resealing with the kept key needs no password and opens the same way.
        let file = seal(&key, b"state 2").unwrap();
        assert_eq!(
            open(b"correct horse", &file).unwrap().1.as_slice(),
            b"state 2"
        );
    }

    #[test]
    fn any_bit_flip_is_rejected() {
        let file = seal(&create(b"pw", FAST).unwrap(), b"state").unwrap();
        // Skip the KDF params: changing them only changes the derived key, also rejected,
        // but huge values would make the test slow.
        for i in (14..file.len()).step_by(7) {
            let mut bad = file.clone();
            bad[i] ^= 1;
            assert!(open(b"pw", &bad).is_err(), "byte {i}");
        }
        assert!(open(b"pw", &file[..file.len() - 1]).is_err());
    }
}
