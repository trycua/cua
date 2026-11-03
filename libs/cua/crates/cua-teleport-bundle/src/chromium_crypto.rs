// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Chromium "Safe Storage" value crypto.
//!
//! Chromium-family browsers (Chrome, Chromium, Brave, Microsoft Edge, Arc, …)
//! encrypt local secrets they don't want readable from disk alone -- saved
//! passwords (`Login Data`'s `password_value`) and, on macOS and Linux,
//! cookies (`Cookies`' `encrypted_value`) -- with a per-machine key:
//!
//! | platform | prefix | key |
//! |----------|--------|-----|
//! | macOS    | `v10`  | PBKDF2-HMAC-SHA1(`<Browser> Safe Storage` Keychain secret, `saltysalt`, 1003 rounds, 16 bytes) |
//! | Linux    | `v10`  | PBKDF2-HMAC-SHA1(`peanuts`, `saltysalt`, 1 round, 16 bytes) |
//! | Linux    | `v11`  | PBKDF2-HMAC-SHA1(the libsecret secret, `saltysalt`, 1 round, 16 bytes) |
//!
//! then AES-128-CBC with a fixed IV of 16 spaces and PKCS#7 padding.
//!
//! Windows is a different scheme, in the [`gcm`] section below: `v10` +
//! 12-byte nonce + AES-256-GCM ciphertext + 16-byte tag under a random
//! 32-byte key that Chrome keeps in `Local State` (`os_crypt.encrypted_key`,
//! base64 of `"DPAPI"` + a DPAPI blob). Unwrapping that blob is an OS call the
//! sender makes (`cua_teleport::safe_storage`); the cipher and the Local State
//! encoding live here, pure. `v20` (app-bound) cannot be decrypted by anyone
//! but Chrome.
//!
//! This module is **pure**: it derives keys and runs the cipher over bytes
//! it is given. It never reads a Keychain, a file, or the network -- every
//! Safe Storage secret is sourced by a caller (the sender's Keychain read on
//! macOS, or the receiver creating/reading the destination's own item) and
//! handed in. That split is what makes "decrypt on the source, re-encrypt on
//! the destination with the destination's own key" possible: both the
//! sender (`cua-teleport`) and the receiver (`cua-spacesd-teleport`) link
//! this one crate and never invent their own copy of the cipher.

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use zeroize::Zeroizing;

use crate::error::TeleportError;

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

/// The fixed PBKDF2 salt Chromium uses for every Safe Storage key.
const SALT: &[u8] = b"saltysalt";
/// The fixed AES-CBC IV Chromium uses (16 ASCII spaces): the key is unique
/// per machine and the value is never reused across keys, so a static IV
/// does not repeat under the same key/plaintext pair in practice the way it
/// would in a general-purpose protocol.
const IV: [u8; 16] = [b' '; 16];
/// AES-128 key length.
pub const KEY_LEN: usize = 16;

/// macOS PBKDF2-HMAC-SHA1 round count for the Keychain-sourced key.
pub const MACOS_PBKDF2_ROUNDS: u32 = 1003;
/// Linux `v10` PBKDF2-HMAC-SHA1 round count (the fixed `peanuts` password).
pub const LINUX_V10_PBKDF2_ROUNDS: u32 = 1;
/// Linux `v11` PBKDF2-HMAC-SHA1 round count (the libsecret-sourced password).
pub const LINUX_V11_PBKDF2_ROUNDS: u32 = 1;
/// Chrome's fixed Linux `v10` password when no key store secret exists.
pub const LINUX_V10_PASSWORD: &[u8] = b"peanuts";

/// The version-prefix byte length (`v10`, `v11`).
pub const PREFIX_LEN: usize = 3;

/// Derives the 128-bit AES key from a Safe Storage secret: PBKDF2-HMAC-SHA1
/// over `secret` with the fixed `saltysalt` salt, `rounds` iterations, 16
/// output bytes. `secret` is never logged or retained beyond this call.
pub fn derive_key(secret: &[u8], rounds: u32) -> Zeroizing<[u8; KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(secret, SALT, rounds, key.as_mut());
    key
}

/// Decrypts one value's ciphertext (the bytes AFTER the 3-byte `v10`/`v11`
/// prefix) with `key`. Fails on a malformed length or a wrong key/damaged
/// ciphertext; never panics on attacker- or disk-controlled input.
pub fn decrypt(
    key: &[u8; KEY_LEN],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, TeleportError> {
    if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) {
        return Err(TeleportError::Provider(
            "a Chromium Safe Storage value has a malformed ciphertext length".into(),
        ));
    }
    let mut buf = Zeroizing::new(ciphertext.to_vec());
    let plain = Aes128CbcDec::new(key.into(), &IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| {
            TeleportError::Provider(
                "a Chromium Safe Storage value did not decrypt (wrong key or damaged)".into(),
            )
        })?;
    Ok(Zeroizing::new(plain.to_vec()))
}

/// Decrypts a full value: a 3-byte version prefix (`v10` or `v11`) followed
/// by the AES-CBC ciphertext, as Chromium stores it in `Cookies` and
/// `Login Data`. Returns the prefix alongside the plaintext so a caller that
/// needs to distinguish `v10` from `v11` (different key derivations) can.
pub fn decrypt_prefixed<'a>(
    key: &[u8; KEY_LEN],
    value: &'a [u8],
) -> Result<(&'a [u8; PREFIX_LEN], Zeroizing<Vec<u8>>), TeleportError> {
    let (prefix, ct) = value.split_at_checked(PREFIX_LEN).ok_or_else(|| {
        TeleportError::Provider(
            "a Chromium Safe Storage value is too short to have a version prefix".into(),
        )
    })?;
    let prefix: &[u8; PREFIX_LEN] = prefix
        .try_into()
        .expect("split_at_checked(3) yields 3 bytes");
    Ok((prefix, decrypt(key, ct)?))
}

/// Encrypts `plaintext` under `key` the way Chromium does: AES-128-CBC with
/// the fixed space IV and PKCS#7 padding. Returns the ciphertext WITHOUT a
/// version prefix; callers that write a `Cookies`/`Login Data` value prepend
/// `v10` themselves ([`encrypt_v10`]).
pub fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Vec<u8> {
    Aes128CbcEnc::new(key.into(), &IV.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext)
}

/// Encrypts `plaintext` under `key` and prepends the `v10` version prefix:
/// the full value Chromium's SQLite stores expect on every OS this module
/// supports (macOS and Linux both write `v10` for a freshly created key; a
/// receiver re-encrypting for the destination always writes `v10`, never
/// `v11`, because `v11` only arises from the Linux libsecret path and the
/// destination gets its own freshly-created macOS Keychain item).
pub fn encrypt_v10(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(PREFIX_LEN + plaintext.len() + 16);
    out.extend_from_slice(b"v10");
    out.extend(encrypt(key, plaintext));
    out
}

/// The macOS login-Keychain "Safe Storage" service name for a Chromium-family
/// browser's catalog id, or `None` for a browser this module does not know
/// (Firefox's cookies are unencrypted and never reach this table).
pub fn macos_safe_storage_service(browser_id: &str) -> Option<&'static str> {
    match browser_id.trim().to_ascii_lowercase().as_str() {
        "chrome" => Some("Chrome Safe Storage"),
        "chromium" => Some("Chromium Safe Storage"),
        "brave" => Some("Brave Safe Storage"),
        "edge" | "microsoft-edge" => Some("Microsoft Edge Safe Storage"),
        "arc" => Some("Arc Safe Storage"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A known key and a known `v10` ciphertext, computed independently of
    /// this module (Python's `hashlib.pbkdf2_hmac` for the key, `openssl enc
    /// -aes-128-cbc` for the ciphertext -- neither touches this crate's
    /// code), must decrypt to the known plaintext. This is a real
    /// cross-check against Chromium's actual construction: the round-trip
    /// tests below only prove `encrypt` and `decrypt` agree with each other,
    /// and would still pass if both shared the same bug (wrong salt, wrong
    /// IV, wrong padding, ...).
    ///
    /// Vector: `secret = "mac-safe-storage"`, 1003 rounds (macOS), `plaintext
    /// = "s3cret-cookie-value"`.
    #[test]
    fn known_key_and_known_v10_blob_decrypt_to_the_known_plaintext() {
        let key = derive_key(b"mac-safe-storage", MACOS_PBKDF2_ROUNDS);
        assert_eq!(
            hex::encode(*key),
            "dbec0fea655540184a3201100fe01a93",
            "PBKDF2-HMAC-SHA1(mac-safe-storage, saltysalt, 1003) must match \
             the independently computed key"
        );
        let fixture =
            hex::decode("763130cd8b08c739b3a1a4a84d3c500c20e134dd9e4de671b1c1ec809f82b48ac36fe2")
                .unwrap();
        let (prefix, plain) = decrypt_prefixed(&key, &fixture).unwrap();
        assert_eq!(prefix, b"v10");
        assert_eq!(&*plain, b"s3cret-cookie-value");
    }

    #[test]
    fn linux_v10_fixed_password_round_trips() {
        let key = derive_key(LINUX_V10_PASSWORD, LINUX_V10_PBKDF2_ROUNDS);
        let ct = encrypt_v10(&key, b"hello-linux");
        let (prefix, plain) = decrypt_prefixed(&key, &ct).unwrap();
        assert_eq!(prefix, b"v10");
        assert_eq!(&*plain, b"hello-linux");
    }

    #[test]
    fn round_trips_arbitrary_plaintext_including_empty_and_block_sized() {
        let key = derive_key(b"any secret", 7);
        for plaintext in [
            &b""[..],
            b"a",
            b"exactly-sixteen!",
            b"this plaintext is deliberately longer than one AES block",
        ] {
            let ct = encrypt(&key, plaintext);
            let plain = decrypt(&key, &ct).unwrap();
            assert_eq!(&*plain, plaintext, "round trip for {plaintext:?}");
        }
    }

    #[test]
    fn wrong_key_fails_rather_than_returning_garbage() {
        let key_a = derive_key(b"secret-a", 1);
        let key_b = derive_key(b"secret-b", 1);
        let ct = encrypt(&key_a, b"top secret");
        assert!(decrypt(&key_b, &ct).is_err());
    }

    #[test]
    fn malformed_ciphertext_is_rejected_not_panicking() {
        let key = derive_key(b"k", 1);
        assert!(decrypt(&key, b"").is_err());
        assert!(decrypt(&key, b"not-block-sized").is_err());
        // A value too short to hold a prefix at all.
        assert!(decrypt_prefixed(&key, b"vv").is_err());
    }

    #[test]
    fn different_secrets_or_rounds_derive_different_keys() {
        assert_ne!(
            derive_key(b"a", 1).as_slice(),
            derive_key(b"b", 1).as_slice()
        );
        assert_ne!(
            derive_key(b"a", 1).as_slice(),
            derive_key(b"a", 2).as_slice()
        );
    }

    #[test]
    fn macos_service_names_cover_the_catalog_and_reject_unknown_browsers() {
        assert_eq!(
            macos_safe_storage_service("chrome"),
            Some("Chrome Safe Storage")
        );
        assert_eq!(
            macos_safe_storage_service("Chromium"),
            Some("Chromium Safe Storage")
        );
        assert_eq!(
            macos_safe_storage_service("brave"),
            Some("Brave Safe Storage")
        );
        assert_eq!(
            macos_safe_storage_service("edge"),
            Some("Microsoft Edge Safe Storage")
        );
        assert_eq!(macos_safe_storage_service("arc"), Some("Arc Safe Storage"));
        assert_eq!(macos_safe_storage_service("firefox"), None);
        assert_eq!(macos_safe_storage_service("safari"), None);
    }
}

/// Windows `v10`: AES-256-GCM under the 32-byte key from `Local State`.
pub mod gcm {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use base64::Engine as _;
    use zeroize::Zeroizing;

    use crate::error::TeleportError;

    /// AES-256 key length.
    pub const KEY_LEN: usize = 32;
    /// Nonce length.
    pub const NONCE_LEN: usize = 12;
    /// GCM tag length.
    pub const TAG_LEN: usize = 16;
    /// `os_crypt.encrypted_key` is `base64("DPAPI" + blob)`.
    pub const DPAPI_PREFIX: &[u8] = b"DPAPI";

    /// The DPAPI blob inside a Local State `os_crypt.encrypted_key` value
    /// (base64 decoded, `DPAPI` stripped).
    pub fn dpapi_blob(encrypted_key_b64: &str) -> Result<Vec<u8>, TeleportError> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(encrypted_key_b64.trim())
            .map_err(|_| {
                TeleportError::Provider("Local State encrypted_key is not base64".into())
            })?;
        raw.strip_prefix(DPAPI_PREFIX)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                TeleportError::Provider("Local State encrypted_key is not a DPAPI key".into())
            })
    }

    /// `os_crypt.encrypted_key` for a DPAPI blob (what a new Local State holds).
    pub fn encrypted_key_value(dpapi_blob: &[u8]) -> String {
        let mut raw = DPAPI_PREFIX.to_vec();
        raw.extend_from_slice(dpapi_blob);
        base64::engine::general_purpose::STANDARD.encode(raw)
    }

    /// The `os_crypt.encrypted_key` string of a Local State JSON document.
    pub fn encrypted_key_of(local_state_json: &[u8]) -> Option<String> {
        let v: serde_json::Value = serde_json::from_slice(local_state_json).ok()?;
        v.get("os_crypt")?
            .get("encrypted_key")?
            .as_str()
            .map(str::to_string)
    }

    /// Whether `value` is a `v10` GCM value (`v10` + nonce + tag at least).
    pub fn is_v10(value: &[u8]) -> bool {
        value.starts_with(b"v10") && value.len() >= 3 + NONCE_LEN + TAG_LEN
    }

    /// Decrypts a `v10` value (prefix included) with `key`.
    pub fn decrypt(key: &[u8; KEY_LEN], value: &[u8]) -> Result<Zeroizing<Vec<u8>>, TeleportError> {
        if !is_v10(value) {
            return Err(TeleportError::Provider(
                "a Windows Chromium value is too short to be v10 AES-GCM".into(),
            ));
        }
        let (nonce, ct) = value[3..].split_at(NONCE_LEN);
        Aes256Gcm::new(key.into())
            .decrypt(Nonce::from_slice(nonce), ct)
            .map(Zeroizing::new)
            .map_err(|_| {
                TeleportError::Provider(
                    "a Windows Chromium value did not decrypt (wrong key or damaged)".into(),
                )
            })
    }

    /// Encrypts `plaintext` under `key` the way Chromium does: `v10` + nonce +
    /// ciphertext + tag. The nonce is supplied by the caller (random per value).
    pub fn encrypt(key: &[u8; KEY_LEN], nonce: &[u8; NONCE_LEN], plaintext: &[u8]) -> Vec<u8> {
        let ct = Aes256Gcm::new(key.into())
            .encrypt(Nonce::from_slice(nonce), plaintext)
            .expect("AES-GCM encryption of an in-memory buffer cannot fail");
        let mut out = Vec::with_capacity(3 + NONCE_LEN + ct.len());
        out.extend_from_slice(b"v10");
        out.extend_from_slice(nonce);
        out.extend_from_slice(&ct);
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn v10_round_trips_and_a_wrong_key_or_damage_fails() {
            let key = [7u8; KEY_LEN];
            let nonce = [3u8; NONCE_LEN];
            let enc = encrypt(&key, &nonce, b"session-value");
            assert!(enc.starts_with(b"v10") && &enc[3..15] == nonce.as_slice());
            assert_eq!(enc.len(), 3 + 12 + 13 + 16);
            assert_eq!(&*decrypt(&key, &enc).unwrap(), b"session-value");
            assert!(decrypt(&[8u8; KEY_LEN], &enc).is_err());
            let mut bad = enc.clone();
            *bad.last_mut().unwrap() ^= 1;
            assert!(decrypt(&key, &bad).is_err());
            assert!(decrypt(&key, b"v10short").is_err());
            assert!(!is_v10(b"v20aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        }

        /// A vector computed independently (RFC-style, AES-256-GCM, nonce
        /// 000000000000000000000000, key of 32 zero bytes, empty plaintext):
        /// the tag of empty data under the zero key and zero nonce.
        #[test]
        fn matches_the_published_aes_256_gcm_zero_vector() {
            let key = [0u8; KEY_LEN];
            let enc = encrypt(&key, &[0u8; NONCE_LEN], b"");
            assert_eq!(hex::encode(&enc[15..]), "530f8afbc74536b9a963b4f1c4cb738b");
        }

        #[test]
        fn local_state_key_encoding_round_trips() {
            let v = encrypted_key_value(&[1, 2, 3]);
            assert_eq!(dpapi_blob(&v).unwrap(), [1, 2, 3]);
            assert!(dpapi_blob("AAAA").is_err());
            let ls = format!(r#"{{"os_crypt":{{"encrypted_key":"{v}"}}}}"#);
            assert_eq!(encrypted_key_of(ls.as_bytes()).as_deref(), Some(v.as_str()));
            assert_eq!(encrypted_key_of(b"{}"), None);
        }
    }
}
