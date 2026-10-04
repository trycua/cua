// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The crypto envelope. Thin wrappers over audited primitives only:
//!
//! - AEAD: ChaCha20-Poly1305 (ring), 256-bit keys, random 96-bit nonces.
//!   Every key encrypts few messages (one per item revision), so random
//!   nonces stay far below the birthday bound.
//! - KDF: HKDF-SHA256 (ring) for sub-keys of the vault master key;
//!   Argon2id (RustCrypto) for passphrases.
//! - MAC: HMAC-SHA256 (ring), verified in constant time.
//! - Randomness: ring's `SystemRandom` (the OS CSPRNG).
//!
//! Nothing here invents a construction: sealing is plain AEAD with the
//! caller's associated data, which binds a ciphertext to its vault, record
//! kind, id and revision so records cannot be swapped or replayed across
//! slots.

use base64::Engine as _;
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use ring::{hkdf, hmac};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::{Error, Result};

/// Symmetric key length in bytes.
pub const KEY_LEN: usize = 32;
/// AEAD nonce length in bytes.
pub const NONCE_LEN: usize = 12;
/// The only AEAD algorithm id written today.
pub const ALG_CHACHA20_POLY1305: &str = "chacha20poly1305";

/// A 256-bit secret key, zeroized on drop and never printed.
#[derive(Clone)]
pub struct SecretKey(Zeroizing<[u8; KEY_LEN]>);

impl SecretKey {
    /// A fresh random key.
    pub fn generate() -> Result<Self> {
        let mut k = Zeroizing::new([0u8; KEY_LEN]);
        SystemRandom::new()
            .fill(k.as_mut())
            .map_err(|_| Error::Crypto("the OS random generator failed".into()))?;
        Ok(Self(k))
    }

    /// Wraps existing key bytes (copied; the source should be zeroized by
    /// the caller).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != KEY_LEN {
            return Err(Error::Crypto(format!(
                "a key is {KEY_LEN} bytes, got {}",
                bytes.len()
            )));
        }
        let mut k = Zeroizing::new([0u8; KEY_LEN]);
        k.copy_from_slice(bytes);
        Ok(Self(k))
    }

    /// The raw bytes. Only for wrapping this key under another key.
    pub fn expose(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// HKDF-SHA256 sub-key: `info` separates purposes (for example
    /// `b"cua-keyvault/v1/meta"`).
    pub fn derive(&self, info: &[u8]) -> SecretKey {
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"cua-keyvault/v1/hkdf-salt");
        let prk = salt.extract(self.0.as_ref());
        let info_parts = [info];
        let okm = prk
            .expand(&info_parts, HkdfLen)
            .expect("HKDF-SHA256 can always expand 32 bytes");
        let mut out = Zeroizing::new([0u8; KEY_LEN]);
        okm.fill(out.as_mut())
            .expect("HKDF-SHA256 can always fill 32 bytes");
        SecretKey(out)
    }

    /// HMAC-SHA256 of `data`.
    pub fn mac(&self, data: &[u8]) -> [u8; 32] {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.0.as_ref());
        let tag = hmac::sign(&key, data);
        let mut out = [0u8; 32];
        out.copy_from_slice(tag.as_ref());
        out
    }

    /// Constant-time HMAC-SHA256 verification.
    pub fn verify_mac(&self, data: &[u8], tag: &[u8]) -> bool {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.0.as_ref());
        hmac::verify(&key, data, tag).is_ok()
    }
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

struct HkdfLen;
impl hkdf::KeyType for HkdfLen {
    fn len(&self) -> usize {
        KEY_LEN
    }
}

/// Fills a fresh random array.
pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| Error::Crypto("the OS random generator failed".into()))?;
    Ok(b)
}

/// A random identifier: 16 bytes, lowercase hex.
pub fn random_id() -> Result<String> {
    Ok(hex::encode(random_bytes::<16>()?))
}

/// SHA-256 digest.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, data);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

/// Constant-time equality.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// An AEAD ciphertext as stored on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    /// Algorithm id ([`ALG_CHACHA20_POLY1305`]).
    pub alg: String,
    /// Base64 nonce.
    pub nonce: String,
    /// Base64 ciphertext with the 16-byte tag appended.
    pub ct: String,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Encrypts `plaintext` under `key`, authenticating `aad`.
pub fn seal(key: &SecretKey, aad: &[u8], plaintext: &[u8]) -> Result<Sealed> {
    let nonce_bytes = random_bytes::<NONCE_LEN>()?;
    let less_safe = aead_key(key)?;
    let mut in_out = Zeroizing::new(plaintext.to_vec());
    less_safe
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(aad),
            &mut *in_out,
        )
        .map_err(|_| Error::Crypto("seal failed".into()))?;
    Ok(Sealed {
        alg: ALG_CHACHA20_POLY1305.into(),
        nonce: b64().encode(nonce_bytes),
        ct: b64().encode(&*in_out),
    })
}

/// Decrypts and authenticates. Any mismatch (key, `aad`, tampered bytes,
/// unknown algorithm) is [`Error::Crypto`] without detail.
pub fn open(key: &SecretKey, aad: &[u8], sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>> {
    if sealed.alg != ALG_CHACHA20_POLY1305 {
        return Err(Error::Crypto(format!("unknown algorithm {:?}", sealed.alg)));
    }
    let nonce = b64()
        .decode(&sealed.nonce)
        .map_err(|_| Error::Crypto("bad nonce".into()))?;
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| Error::Crypto("bad nonce length".into()))?;
    let mut buf = Zeroizing::new(
        b64()
            .decode(&sealed.ct)
            .map_err(|_| Error::Crypto("bad ciphertext".into()))?,
    );
    let less_safe = aead_key(key)?;
    let plain_len = less_safe
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut buf,
        )
        .map_err(|_| Error::Crypto("authentication failed".into()))?
        .len();
    buf.truncate(plain_len);
    Ok(buf)
}

fn aead_key(key: &SecretKey) -> Result<LessSafeKey> {
    let unbound = UnboundKey::new(&aead::CHACHA20_POLY1305, key.expose())
        .map_err(|_| Error::Crypto("bad key".into()))?;
    Ok(LessSafeKey::new(unbound))
}

/// Argon2id cost parameters, stored with each passphrase protector so they
/// can be raised later without breaking old vaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Iterations.
    pub t: u32,
    /// Lanes.
    pub p: u32,
    /// Base64 salt (16 bytes).
    pub salt: String,
}

impl KdfParams {
    /// OWASP-recommended Argon2id defaults (64 MiB, 3 passes, 1 lane).
    pub fn recommended() -> Result<Self> {
        Ok(Self {
            m_kib: 64 * 1024,
            t: 3,
            p: 1,
            salt: b64().encode(random_bytes::<16>()?),
        })
    }

    /// Cheap parameters for tests only (8 MiB, 1 pass).
    pub fn for_tests() -> Result<Self> {
        Ok(Self {
            m_kib: 8 * 1024,
            t: 1,
            p: 1,
            salt: b64().encode(random_bytes::<16>()?),
        })
    }
}

/// Stretches a passphrase into a key-encryption key with Argon2id.
pub fn passphrase_key(passphrase: &[u8], params: &KdfParams) -> Result<SecretKey> {
    if params.m_kib < 8 * 1024 || params.t < 1 || params.p < 1 {
        return Err(Error::Crypto("Argon2id parameters below the floor".into()));
    }
    let salt = b64()
        .decode(&params.salt)
        .map_err(|_| Error::Crypto("bad salt".into()))?;
    let p = argon2::Params::new(params.m_kib, params.t, params.p, Some(KEY_LEN))
        .map_err(|e| Error::Crypto(format!("Argon2id parameters: {e}")))?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, p);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    a.hash_password_into(passphrase, &salt, out.as_mut())
        .map_err(|e| Error::Crypto(format!("Argon2id: {e}")))?;
    let key = SecretKey::from_bytes(out.as_ref());
    out.zeroize();
    key
}

/// Base64url without padding (tokens).
pub fn b64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Base64url decode.
pub fn b64url_decode(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| Error::Crypto("bad base64url".into()))
}

/// Standard base64 encode (payload bytes on disk).
pub fn b64_encode(bytes: &[u8]) -> String {
    b64().encode(bytes)
}

/// Standard base64 decode.
pub fn b64_decode(s: &str) -> Result<Vec<u8>> {
    b64()
        .decode(s)
        .map_err(|_| Error::Crypto("bad base64".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn seal_open_round_trip_and_aad_binding() {
        let k = SecretKey::generate().unwrap();
        let s = seal(&k, b"slot-a", b"hello").unwrap();
        assert_eq!(&*open(&k, b"slot-a", &s).unwrap(), b"hello");
        // Wrong slot (associated data), wrong key, and tampering all fail.
        assert!(open(&k, b"slot-b", &s).is_err());
        assert!(open(&SecretKey::generate().unwrap(), b"slot-a", &s).is_err());
        let mut bad = s.clone();
        let mut ct = b64_decode(&bad.ct).unwrap();
        ct[0] ^= 1;
        bad.ct = b64_encode(&ct);
        assert!(open(&k, b"slot-a", &bad).is_err());
        let mut alg = s.clone();
        alg.alg = "none".into();
        assert!(open(&k, b"slot-a", &alg).is_err());
    }

    #[test]
    fn nonces_are_fresh() {
        let k = SecretKey::generate().unwrap();
        let a = seal(&k, b"", b"x").unwrap();
        let b = seal(&k, b"", b"x").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ct, b.ct);
    }

    #[test]
    fn derived_keys_are_separated_and_deterministic() {
        let k = SecretKey::generate().unwrap();
        assert_eq!(k.derive(b"a").expose(), k.derive(b"a").expose());
        assert_ne!(k.derive(b"a").expose(), k.derive(b"b").expose());
        assert_ne!(k.derive(b"a").expose(), k.expose());
    }

    #[test]
    fn debug_never_prints_key_bytes() {
        let k = SecretKey::from_bytes(&[7u8; 32]).unwrap();
        assert_eq!(format!("{k:?}"), "SecretKey(<redacted>)");
    }

    #[test]
    fn passphrase_kdf_is_salted_and_floored() {
        let p = KdfParams::for_tests().unwrap();
        let a = passphrase_key(b"correct horse", &p).unwrap();
        let b = passphrase_key(b"correct horse", &p).unwrap();
        assert_eq!(a.expose(), b.expose());
        assert_ne!(
            a.expose(),
            passphrase_key(b"correct horse", &KdfParams::for_tests().unwrap())
                .unwrap()
                .expose()
        );
        let mut weak = p.clone();
        weak.m_kib = 1024;
        assert!(passphrase_key(b"x", &weak).is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn any_plaintext_round_trips(data in proptest::collection::vec(any::<u8>(), 0..4096),
                                     aad in proptest::collection::vec(any::<u8>(), 0..64)) {
            let k = SecretKey::generate().unwrap();
            let s = seal(&k, &aad, &data).unwrap();
            prop_assert_eq!(&*open(&k, &aad, &s).unwrap(), &data[..]);
        }

        #[test]
        fn any_single_bit_flip_is_rejected(data in proptest::collection::vec(any::<u8>(), 1..512),
                                           pos in any::<usize>(), bit in 0u8..8) {
            let k = SecretKey::generate().unwrap();
            let s = seal(&k, b"aad", &data).unwrap();
            let mut ct = b64_decode(&s.ct).unwrap();
            let i = pos % ct.len();
            ct[i] ^= 1 << bit;
            let bad = Sealed { ct: b64_encode(&ct), ..s };
            prop_assert!(open(&k, b"aad", &bad).is_err());
        }
    }
}
