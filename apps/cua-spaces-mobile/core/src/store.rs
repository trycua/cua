// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The phone's sealed store: what is kept on disk between launches (the
//! outbox, the cached threads and Spaces list) is encrypted, so a message
//! never sits there as plain text.
//!
//! AES-256-GCM through `ring`. The 32-byte key is made here and kept in the
//! Keychain on the native side (this phone only, readable while unlocked); the
//! native module hands it to these calls and JavaScript only ever holds sealed
//! text. A sealed value is `v1.` and the base64url of nonce (12 bytes), then
//! the ciphertext and its tag. The nonce is random per seal. `aad` names what
//! the value is (`outbox`, `thread:relay:abc`), so a sealed value cannot be
//! moved to another slot and still open.

use base64::Engine as _;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};

/// The methods this module answers.
pub const METHODS: &[&str] = &["store.generate", "store.seal", "store.open"];

const PREFIX: &str = "v1.";

fn b64url() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}
fn b64std() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn key_of(key_b64: &str) -> Result<LessSafeKey, String> {
    let raw = b64std()
        .decode(key_b64.trim())
        .map_err(|_| "the store key is not base64".to_string())?;
    let unbound = UnboundKey::new(&AES_256_GCM, &raw)
        .map_err(|_| "the store key is the wrong size".to_string())?;
    Ok(LessSafeKey::new(unbound))
}

/// A new random store key, base64.
pub fn generate() -> Result<String, String> {
    let mut k = [0u8; 32];
    SystemRandom::new()
        .fill(&mut k)
        .map_err(|_| "cannot make a store key".to_string())?;
    Ok(b64std().encode(k))
}

fn seal_with_nonce(
    key: &LessSafeKey,
    nonce: [u8; NONCE_LEN],
    plaintext: &[u8],
    aad: &str,
) -> Result<String, String> {
    let mut buf = plaintext.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad.as_bytes()),
        &mut buf,
    )
    .map_err(|_| "cannot seal".to_string())?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&buf);
    Ok(format!("{PREFIX}{}", b64url().encode(out)))
}

/// Seals `plaintext` under `key_b64`, bound to `aad`.
pub fn seal(key_b64: &str, plaintext: &str, aad: &str) -> Result<String, String> {
    let key = key_of(key_b64)?;
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "cannot make a nonce".to_string())?;
    seal_with_nonce(&key, nonce, plaintext.as_bytes(), aad)
}

/// Opens a sealed value; an error for a wrong key, a changed value or another `aad`.
pub fn open(key_b64: &str, sealed: &str, aad: &str) -> Result<String, String> {
    let key = key_of(key_b64)?;
    let body = sealed
        .strip_prefix(PREFIX)
        .ok_or_else(|| "this is not a sealed value".to_string())?;
    let mut data = b64url()
        .decode(body)
        .map_err(|_| "this is not a sealed value".to_string())?;
    if data.len() < NONCE_LEN + AES_256_GCM.tag_len() {
        return Err("this is not a sealed value".into());
    }
    let (nonce, rest) = data.split_at_mut(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| "this is not a sealed value".to_string())?;
    let plain = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad.as_bytes()),
            rest,
        )
        .map_err(|_| "cannot open: wrong key, wrong slot or a changed value".to_string())?;
    String::from_utf8(plain.to_vec()).map_err(|_| "the sealed value is not text".to_string())
}

/// `store.generate {}` gives `{key}`; `store.seal {key, plaintext, aad}` gives `{sealed}`;
/// `store.open {key, sealed, aad}` gives `{plaintext}`.
pub fn call(method: &str, args: &str) -> Result<String, String> {
    let a: Value = serde_json::from_str(args).map_err(|e| format!("{method}: {e}"))?;
    let text = |k: &str| {
        a.get(k)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{method}: argument {k} is missing"))
    };
    let aad = a.get("aad").and_then(Value::as_str).unwrap_or("");
    let out = match method {
        "store.generate" => json!({ "key": generate()? }),
        "store.seal" => json!({ "sealed": seal(text("key")?, text("plaintext")?, aad)? }),
        "store.open" => json!({ "plaintext": open(text("key")?, text("sealed")?, aad)? }),
        other => return Err(format!("invalid: unknown method {other}")),
    };
    Ok(out.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    /// NIST SP 800-38D / the GCM spec, test case 15 (AES-256, 96-bit IV, no AAD): the sealed bytes are the standard's.
    #[test]
    fn the_cipher_is_aes_256_gcm_by_the_published_known_answer() {
        let key = LessSafeKey::new(
            UnboundKey::new(
                &AES_256_GCM,
                &hex("feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308"),
            )
            .unwrap(),
        );
        let iv: [u8; 12] = hex("cafebabefacedbaddecaf888").try_into().unwrap();
        let plain = hex(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255",
        );
        let mut buf = plain.clone();
        key.seal_in_place_append_tag(Nonce::assume_unique_for_key(iv), Aad::empty(), &mut buf)
            .unwrap();
        let want = "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662898015adb094dac5d93471bdec1a502270e3cc6c";
        assert_eq!(buf, hex(want));
        // And the store's own framing carries exactly those bytes after the nonce.
        let key_b64 = b64std().encode(hex(
            "feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308",
        ));
        let k = key_of(&key_b64).unwrap();
        let sealed = seal_with_nonce(&k, iv, &plain, "").unwrap();
        let raw = b64url()
            .decode(sealed.strip_prefix("v1.").unwrap())
            .unwrap();
        assert_eq!(&raw[..12], &iv);
        assert_eq!(&raw[12..], &hex(want)[..]);
    }

    #[test]
    fn a_value_round_trips_and_every_seal_is_different() {
        let key = generate().unwrap();
        let a = seal(&key, "hello \u{1F600}", "outbox").unwrap();
        let b = seal(&key, "hello \u{1F600}", "outbox").unwrap();
        assert_ne!(a, b, "a fresh nonce each time");
        assert!(a.starts_with("v1."));
        assert!(!a.contains("hello"));
        assert_eq!(open(&key, &a, "outbox").unwrap(), "hello \u{1F600}");
        assert_eq!(open(&key, &seal(&key, "", "x").unwrap(), "x").unwrap(), "");
    }

    #[test]
    fn a_wrong_key_a_wrong_slot_or_a_changed_value_does_not_open() {
        let key = generate().unwrap();
        let other = generate().unwrap();
        let sealed = seal(&key, "secret", "thread:relay:a").unwrap();
        assert!(open(&other, &sealed, "thread:relay:a").is_err());
        assert!(open(&key, &sealed, "thread:relay:b").is_err());
        let mut tampered = sealed.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == 'A' { 'B' } else { 'A' });
        assert!(open(&key, &tampered, "thread:relay:a").is_err());
        for bad in ["", "v1.", "v1.AAAA", "v2.AAAA", "plain text"] {
            assert!(open(&key, bad, "").is_err(), "{bad}");
        }
        assert!(key_of("not base64!!").is_err());
        assert!(key_of(&b64std().encode([1u8; 16])).is_err());
    }

    #[test]
    fn the_json_entry_point_matches() {
        let key: Value = serde_json::from_str(&call("store.generate", "{}").unwrap()).unwrap();
        let key = key["key"].as_str().unwrap();
        let args = json!({ "key": key, "plaintext": "hi", "aad": "outbox" }).to_string();
        let sealed: Value = serde_json::from_str(&call("store.seal", &args).unwrap()).unwrap();
        let open_args =
            json!({ "key": key, "sealed": sealed["sealed"], "aad": "outbox" }).to_string();
        let plain: Value = serde_json::from_str(&call("store.open", &open_args).unwrap()).unwrap();
        assert_eq!(plain["plaintext"], "hi");
        assert!(call("store.nope", "{}").is_err());
        assert!(
            call("store.seal", "{}")
                .unwrap_err()
                .contains("argument key is missing")
        );
    }
}
