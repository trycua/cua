// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! This phone's device key, as `cua-host::device` has it: a P-256 key, the id `dev_` and the first 24 hex characters
//! of the SHA-256 of the public key (uncompressed point), a fixed-size ECDSA signature of the SHA-256 of a message
//! (r then s), both base64url without padding. The key is PKCS#8; the native module keeps it in the Keychain and hands
//! it here per call, so it never passes through JavaScript.
//!
//! `tests/device_known_answers.rs` checks every answer against `cua_host::device`.

use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};

/// The methods this module answers.
pub const METHODS: &[&str] = &["device.generate", "device.info", "device.sign"];

fn b64url() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}
fn b64std() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn pair(pkcs8_b64: &str) -> Result<EcdsaKeyPair, String> {
    let der = b64std()
        .decode(pkcs8_b64.trim())
        .map_err(|_| "the device key is not base64".to_string())?;
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &SystemRandom::new())
        .map_err(|_| "the stored device key is invalid".to_string())
}

/// `dev_` and 24 hex characters of the SHA-256 of the uncompressed public key.
pub fn id_of(public_key: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, public_key);
    let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("dev_{}", &hex[..24])
}

pub fn generate() -> Result<String, String> {
    let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
        .map_err(|_| "cannot generate a device key".to_string())?;
    Ok(b64std().encode(der.as_ref()))
}

pub fn info(pkcs8_b64: &str) -> Result<(String, String), String> {
    let k = pair(pkcs8_b64)?;
    let public = k.public_key().as_ref();
    Ok((b64url().encode(public), id_of(public)))
}

pub fn sign(pkcs8_b64: &str, message: &str) -> Result<String, String> {
    let k = pair(pkcs8_b64)?;
    let sig = k
        .sign(&SystemRandom::new(), message.as_bytes())
        .map_err(|_| "cannot sign with the device key".to_string())?;
    Ok(b64url().encode(sig.as_ref()))
}

/// `device.generate {}` gives `{pkcs8}`; `device.info {pkcs8}` gives `{publicKey, id}`;
/// `device.sign {pkcs8, message}` gives `{signature}`.
pub fn call(method: &str, args: &str) -> Result<String, String> {
    let a: Value = serde_json::from_str(args).map_err(|e| format!("{method}: {e}"))?;
    let text = |k: &str| {
        a.get(k)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{method}: argument {k} is missing"))
    };
    let out = match method {
        "device.generate" => json!({ "pkcs8": generate()? }),
        "device.info" => {
            let (public_key, id) = info(text("pkcs8")?)?;
            json!({ "publicKey": public_key, "id": id })
        }
        "device.sign" => json!({ "signature": sign(text("pkcs8")?, text("message")?)? }),
        other => return Err(format!("invalid: unknown method {other}")),
    };
    Ok(out.to_string())
}
