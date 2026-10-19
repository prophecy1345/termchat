//! Encrypted state file (ADR-002, ADR-003).
//!
//! Layout, all integers little-endian:
//! `version u16 | m_kib u32 | t u32 | p u32 | slot | slot | nonce 24 | body`
//! where `slot = salt 16 | nonce 24 | wrapped DEK 32 + tag 16`.
//! The body is encrypted with a random data key (DEK) and the whole header as AAD.
//! One slot wraps the DEK under the password. The other is random bytes, or
//! wraps `DURESS` under the duress password (ADR-003). Order of slots is random,
//! and both look the same: an AEAD ciphertext is indistinguishable from noise.

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
const SLOTS_AT: usize = HEADER - 2 * SLOT;
/// What the duress slot wraps instead of a DEK. A real DEK is random, so it never matches.
const DURESS: [u8; 32] = *b"termchat-duress-marker-v1\0\0\0\0\0\0\0";

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
    /// Slot holding the DEK; the other one is free for a duress password.
    real: usize,
    params: KdfParams,
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
    Ok(FileKey {
        header,
        dek,
        real,
        params,
    })
}

/// Sets (or with `None` clears) the duress password in the free slot.
/// The caller must save afterwards: the header is part of the body's AAD.
pub fn set_duress(key: &mut FileKey, duress: Option<&[u8]>) -> Result<(), Error> {
    let mut slot = random::<SLOT>()?;
    if let Some(password) = duress {
        let (salt, rest) = slot.split_at_mut(SALT);
        let (nonce, wrapped) = rest.split_at_mut(NONCE);
        let kek = derive(password, salt, key.params)?;
        wrapped.copy_from_slice(
            &aead(kek.as_slice())?
                .encrypt(&XNonce::from(arr(nonce)), DURESS.as_slice())
                .map_err(|_| Error::CorruptState)?,
        );
    }
    let at = SLOTS_AT + (1 - key.real) * SLOT;
    key.header[at..at + SLOT].copy_from_slice(&slot);
    Ok(())
}

/// Changes the main password (ADR-007): rewraps the same DEK under a new salt
/// and password. The duress slot stays. The caller must save afterwards.
pub fn rekey(key: &mut FileKey, password: &[u8]) -> Result<(), Error> {
    let mut slot = random::<SLOT>()?;
    {
        let (salt, rest) = slot.split_at_mut(SALT);
        let (nonce, wrapped) = rest.split_at_mut(NONCE);
        let kek = derive(password, salt, key.params)?;
        wrapped.copy_from_slice(
            &aead(kek.as_slice())?
                .encrypt(&XNonce::from(arr(nonce)), key.dek.as_slice())
                .map_err(|_| Error::CorruptState)?,
        );
    }
    let at = SLOTS_AT + key.real * SLOT;
    key.header[at..at + SLOT].copy_from_slice(&slot);
    Ok(())
}

/// Whether `password` is the duress password: a new main password must not be.
pub fn opens_duress(key: &FileKey, password: &[u8]) -> Result<bool, Error> {
    let at = SLOTS_AT + (1 - key.real) * SLOT;
    let slot = &key.header[at..at + SLOT];
    let (salt, rest) = slot.split_at(SALT);
    let (nonce, wrapped) = rest.split_at(NONCE);
    let kek = derive(password, salt, key.params)?;
    let inner = aead(kek.as_slice())?.decrypt(&XNonce::from(arr(nonce)), wrapped);
    Ok(inner.is_ok_and(|k| k.as_slice() == DURESS))
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

/// The first slot's salt: random per file, so it tells one backup from another
/// without opening it (ADR-003). `None` if the file is too short.
pub fn file_id(file: &[u8]) -> Option<[u8; SALT]> {
    file.get(SLOTS_AT..SLOTS_AT + SALT)?.try_into().ok()
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
    // Untrusted file (a backup from elsewhere): do not let it make us
    // allocate gigabytes or spin for minutes, on a phone least of all. Four
    // times the default memory leaves room to raise it later.
    if params.m_kib > 4 * KdfParams::DEFAULT.m_kib || params.t > 8 || params.p > 4 {
        return Err(Error::CorruptState);
    }

    // Derive for both slots and decrypt the body exactly once on every path
    // (with a dummy key if needed): either password, or a wrong one, takes the
    // same time (ADR-003).
    let mut opened = Vec::with_capacity(2);
    for (i, slot) in header[SLOTS_AT..].as_chunks::<SLOT>().0.iter().enumerate() {
        let (salt, rest) = slot.split_at(SALT);
        let (slot_nonce, wrapped) = rest.split_at(NONCE);
        let kek = derive(password, salt, params)?;
        if let Ok(inner) = aead(kek.as_slice())?.decrypt(&XNonce::from(arr(slot_nonce)), wrapped) {
            opened.push((i, Zeroizing::new(inner)));
        }
    }
    let duress = opened.iter().any(|(_, k)| k.as_slice() == DURESS);
    let real = opened.into_iter().find(|(_, k)| k.as_slice() != DURESS);
    let dek = real.as_ref().map_or(&[0u8; 32][..], |(_, k)| k.as_slice());
    let plain = aead(dek)?
        .decrypt(
            &XNonce::from(arr(nonce)),
            Payload {
                msg: body,
                aad: header,
            },
        )
        .ok();

    match (real, plain) {
        (Some((real, dek)), Some(plain)) => {
            let key = FileKey {
                header: header.to_vec(),
                dek,
                real,
                params,
            };
            Ok((key, Zeroizing::new(plain)))
        }
        // The real password, but the body is damaged: never treat this as duress.
        (Some(_), None) => Err(Error::CorruptState),
        (None, _) if duress => Err(Error::Duress),
        (None, _) => Err(Error::WrongPassword),
    }
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
    fn duress_password_is_recognised_and_main_still_works() {
        let mut key = create(b"main password", FAST).unwrap();
        let plain_len = seal(&key, b"state").unwrap().len();
        set_duress(&mut key, Some(b"duress password")).unwrap();
        let file = seal(&key, b"state").unwrap();
        assert_eq!(
            file.len(),
            plain_len,
            "duress must not change the file shape"
        );

        assert!(matches!(
            open(b"duress password", &file),
            Err(Error::Duress)
        ));
        assert!(matches!(open(b"wrong", &file), Err(Error::WrongPassword)));
        let (mut key, plain) = open(b"main password", &file).unwrap();
        assert_eq!(plain.as_slice(), b"state");

        // A damaged body with the real password is corruption, not duress.
        let mut bad = file.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(matches!(
            open(b"main password", &bad),
            Err(Error::CorruptState)
        ));

        set_duress(&mut key, None).unwrap();
        let file = seal(&key, b"state").unwrap();
        assert!(matches!(
            open(b"duress password", &file),
            Err(Error::WrongPassword)
        ));
    }

    #[test]
    fn rekey_changes_the_main_password_and_keeps_duress() {
        let mut key = create(b"old main", FAST).unwrap();
        set_duress(&mut key, Some(b"the duress one")).unwrap();
        assert!(opens_duress(&key, b"the duress one").unwrap());
        assert!(!opens_duress(&key, b"old main").unwrap());
        rekey(&mut key, b"new main").unwrap();
        let file = seal(&key, b"state").unwrap();
        assert!(matches!(
            open(b"old main", &file),
            Err(Error::WrongPassword)
        ));
        assert_eq!(open(b"new main", &file).unwrap().1.as_slice(), b"state");
        assert!(matches!(open(b"the duress one", &file), Err(Error::Duress)));
    }

    #[test]
    fn both_passwords_take_the_same_time() {
        use std::time::Instant;
        let params = KdfParams {
            m_kib: 8 * 1024,
            t: 1,
            p: 1,
        };
        let mut key = create(b"main password", params).unwrap();
        set_duress(&mut key, Some(b"duress password")).unwrap();
        // Argon2 dominates. Only a successful open also runs the ChaCha20 keystream
        // over the body, so huge bodies add a size-dependent difference (ADR-003).
        let file = seal(&key, &vec![0; 16 << 10]).unwrap();
        let time = |pw: &[u8]| {
            let t = Instant::now();
            let _ = open(pw, &file);
            t.elapsed().as_secs_f64()
        };
        let _ = time(b"warm up");
        let (main, duress, wrong) = (
            time(b"main password"),
            time(b"duress password"),
            time(b"wrong"),
        );
        for other in [duress, wrong] {
            let ratio = main / other;
            assert!((0.5..2.0).contains(&ratio), "main {main} vs {other}");
        }
    }

    #[test]
    fn costly_kdf_params_from_a_file_are_refused() {
        let file = seal(&create(b"pw", FAST).unwrap(), b"state").unwrap();
        // m_kib at 2, t at 6, p at 10: each past its cap, refused before any work.
        for (at, value) in [(2, 4 * 64 * 1024 + 1), (6, 9), (10, 5)] {
            let mut bad = file.clone();
            bad[at..at + 4].copy_from_slice(&u32::to_le_bytes(value));
            assert!(
                matches!(open(b"pw", &bad), Err(Error::CorruptState)),
                "offset {at}"
            );
        }
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
