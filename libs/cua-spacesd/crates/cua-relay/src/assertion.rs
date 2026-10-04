// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Relay-asserted identity.
//!
//! In account mode the relay authenticates the client (cua.ai account
//! token), strips its credentials and forwards a short-lived, relay-signed
//! principal assertion in [`ASSERTION_HEADER`]: an EdDSA JWT whose audience
//! is the machine id. The machine verifies it with the relay's public key,
//! which it receives on the join handshake ([`JWKS_HEADER`]) or from a pinned
//! file, so env tokens never leave the host.

use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use ring::signature::KeyPair as _;
use serde::{Deserialize, Serialize};

/// Header carrying the relay-signed principal assertion to the machine.
pub const ASSERTION_HEADER: &str = "x-cua-relay-assertion";
/// Header on the machine's 101 response: base64url(JSON JWKS) of the relay.
pub const JWKS_HEADER: &str = "x-cua-relay-jwks";
/// Header on the machine's 101 response: the owning account id.
pub const OWNER_HEADER: &str = "x-cua-relay-owner";
/// Longest assertion lifetime the relay mints and machines accept.
pub const MAX_ASSERTION_TTL_SECS: u64 = 60;
/// Clock skew tolerated when checking `exp` / `iat`.
pub const LEEWAY_SECS: u64 = 5;

/// Claims of a principal assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionClaims {
    /// The relay (its public URL).
    pub iss: String,
    /// The machine id the assertion is for.
    pub aud: String,
    /// User id (OIDC `sub`).
    pub sub: String,
    /// Account id.
    pub acct: String,
    /// User email, when the identity provider supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Machine id (same as `aud`).
    pub mid: String,
    /// `owner` or `shared`.
    pub role: String,
    /// Granted scope (`env`).
    pub scope: String,
    /// Issued at (Unix seconds).
    pub iat: u64,
    /// Expiry (Unix seconds), at most [`MAX_ASSERTION_TTL_SECS`] after `iat`.
    pub exp: u64,
    /// Unique id.
    pub jti: String,
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The relay's Ed25519 signing key.
pub struct RelayKey {
    kid: String,
    encoding: EncodingKey,
    public: Vec<u8>,
}

impl std::fmt::Debug for RelayKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayKey").field("kid", &self.kid).finish()
    }
}

impl RelayKey {
    /// A fresh random key.
    pub fn generate() -> Self {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("ed25519 keygen");
        Self::from_pkcs8(pkcs8.as_ref()).expect("fresh key parses")
    }

    /// Parses a PKCS#8 (DER) Ed25519 key.
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, String> {
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(der)
            .map_err(|e| format!("invalid Ed25519 PKCS#8 key: {e}"))?;
        let public = pair.public_key().as_ref().to_vec();
        let digest = ring::digest::digest(&ring::digest::SHA256, &public);
        Ok(Self {
            kid: hex::encode(&digest.as_ref()[..8]),
            encoding: EncodingKey::from_ed_der(der),
            public,
        })
    }

    /// Loads the key at `path`, creating it (0600) on first use, so a relay
    /// restart keeps the key machines pinned.
    pub fn load_or_create(path: &std::path::Path) -> std::io::Result<Self> {
        if let Ok(der) = std::fs::read(path) {
            return Self::from_pkcs8(&der)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
        }
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_private(path, pkcs8.as_ref())?;
        Self::from_pkcs8(pkcs8.as_ref()).map_err(std::io::Error::other)
    }

    /// Key id.
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The public JWKS (`/.well-known/jwks.json`).
    pub fn jwks(&self) -> serde_json::Value {
        serde_json::json!({
            "keys": [{
                "kty": "OKP",
                "crv": "Ed25519",
                "x": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.public),
                "kid": self.kid,
                "alg": "EdDSA",
                "use": "sig",
            }]
        })
    }

    /// Signs an assertion.
    pub fn sign(&self, claims: &AssertionClaims) -> String {
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.kid.clone());
        jsonwebtoken::encode(&header, claims, &self.encoding).expect("EdDSA signing")
    }
}

/// Writes a secret file readable only by its owner. The bytes go to a
/// fresh, exclusively created 0600 temp file (never an existing file or
/// symlink) that is renamed over `path`, so `path` holds either the old or
/// the new content, never a mix.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => std::path::Path::new("."),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let mut last = None;
    for _ in 0..16 {
        let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = match options.open(&tmp) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        return Ok(());
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free temp name")))
}

/// The Ed25519 public keys (`x`, base64url) in a JWKS document.
pub fn ed25519_keys(jwks: &str) -> Result<Vec<String>, String> {
    let set: JwkSet = serde_json::from_str(jwks).map_err(|e| format!("relay JWKS: {e}"))?;
    Ok(set
        .keys
        .iter()
        .filter_map(|k| match &k.algorithm {
            jsonwebtoken::jwk::AlgorithmParameters::OctetKeyPair(p) => Some(p.x.clone()),
            _ => None,
        })
        .collect())
}

/// Decodes a [`JWKS_HEADER`] value to its JSON text.
pub fn decode_jwks_header(value: &str) -> Result<String, String> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim().trim_end_matches('='))
        .map_err(|e| format!("relay JWKS header: {e}"))?;
    String::from_utf8(raw).map_err(|e| format!("relay JWKS header: {e}"))
}

/// The relay public keys a machine trusts.
#[derive(Default)]
pub struct TrustedKeys {
    keys: RwLock<Vec<(Option<String>, DecodingKey)>>,
}

impl std::fmt::Debug for TrustedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustedKeys")
            .field("keys", &self.len())
            .finish()
    }
}

impl TrustedKeys {
    /// Replaces the trusted set with the Ed25519 keys of `jwks` (JSON).
    /// Returns how many keys were installed; keys of other types are ignored.
    pub fn set_jwks_json(&self, jwks: &str) -> Result<usize, String> {
        let set: JwkSet = serde_json::from_str(jwks).map_err(|e| format!("relay JWKS: {e}"))?;
        let keys: Vec<_> = set
            .keys
            .iter()
            .filter(|k| {
                matches!(
                    k.algorithm,
                    jsonwebtoken::jwk::AlgorithmParameters::OctetKeyPair(_)
                )
            })
            .filter_map(|k| {
                DecodingKey::from_jwk(k)
                    .ok()
                    .map(|d| (k.common.key_id.clone(), d))
            })
            .collect();
        if keys.is_empty() {
            return Err("relay JWKS has no Ed25519 key".into());
        }
        let n = keys.len();
        *self.keys.write().expect("keys") = keys;
        Ok(n)
    }

    /// Decodes a [`JWKS_HEADER`] value and installs it.
    pub fn set_from_header(&self, value: &str) -> Result<usize, String> {
        self.set_jwks_json(&decode_jwks_header(value)?)
    }

    /// Number of trusted keys.
    pub fn len(&self) -> usize {
        self.keys.read().expect("keys").len()
    }

    /// True with no trusted key (account mode inactive).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Verifies an assertion for `machine_id`: EdDSA signature by a trusted
    /// key, audience, expiry and a lifetime of at most
    /// [`MAX_ASSERTION_TTL_SECS`].
    pub fn verify(&self, token: &str, machine_id: &str) -> Result<AssertionClaims, String> {
        let header = jsonwebtoken::decode_header(token).map_err(|e| format!("assertion: {e}"))?;
        if header.alg != Algorithm::EdDSA {
            return Err("assertion: only EdDSA is accepted".into());
        }
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_audience(&[machine_id]);
        validation.leeway = LEEWAY_SECS;
        validation.set_required_spec_claims(&["exp", "aud", "sub", "iat"]);
        let keys = self.keys.read().expect("keys");
        if keys.is_empty() {
            return Err("no relay key is trusted".into());
        }
        let mut last = String::from("assertion: no matching relay key");
        for (kid, key) in keys.iter() {
            if header.kid.is_some() && kid.is_some() && header.kid != *kid {
                continue;
            }
            match jsonwebtoken::decode::<AssertionClaims>(token, key, &validation) {
                Ok(data) => {
                    let c = data.claims;
                    if c.mid != machine_id {
                        return Err("assertion: machine id mismatch".into());
                    }
                    if c.exp < c.iat || c.exp - c.iat > MAX_ASSERTION_TTL_SECS {
                        return Err("assertion: lifetime too long".into());
                    }
                    if c.iat > now_secs() + LEEWAY_SECS {
                        return Err("assertion: issued in the future".into());
                    }
                    return Ok(c);
                }
                Err(e) => last = format!("assertion: {e}"),
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {

    #[cfg(unix)]
    #[test]
    fn private_writes_are_atomic_and_never_follow_a_planted_temp() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("directory.json");
        let outside = dir.path().join("outside");
        std::fs::write(&outside, "untouched").unwrap();
        // The old fixed temp name, planted as a symlink, is never used.
        std::os::unix::fs::symlink(&outside, path.with_extension("tmp")).unwrap();
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with('.') && n.ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
    }
    use super::*;

    fn claims(machine: &str, ttl: u64) -> AssertionClaims {
        let now = now_secs();
        AssertionClaims {
            iss: "http://relay".into(),
            aud: machine.into(),
            sub: "user-1".into(),
            acct: "acct-1".into(),
            email: Some("ada@example.com".into()),
            name: None,
            mid: machine.into(),
            role: "owner".into(),
            scope: "env".into(),
            iat: now,
            exp: now + ttl,
            jti: "j".into(),
        }
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let key = RelayKey::generate();
        let trusted = TrustedKeys::default();
        assert!(trusted.verify("x.y.z", "m").is_err());
        let header =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.jwks().to_string());
        assert_eq!(trusted.set_from_header(&header).unwrap(), 1);
        let token = key.sign(&claims("machine-01", 60));
        let c = trusted.verify(&token, "machine-01").unwrap();
        assert_eq!(c.acct, "acct-1");
        // Wrong audience, too long a lifetime, another relay's key.
        assert!(trusted.verify(&token, "machine-02").is_err());
        assert!(trusted
            .verify(&key.sign(&claims("machine-01", 3600)), "machine-01")
            .unwrap_err()
            .contains("lifetime"));
        let other = RelayKey::generate();
        assert!(trusted
            .verify(&other.sign(&claims("machine-01", 60)), "machine-01")
            .is_err());
        // Expired.
        let mut old = claims("machine-01", 60);
        old.iat -= 600;
        old.exp -= 600;
        assert!(trusted.verify(&key.sign(&old), "machine-01").is_err());
    }

    #[test]
    fn key_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay/signing.key");
        let a = RelayKey::load_or_create(&path).unwrap();
        let b = RelayKey::load_or_create(&path).unwrap();
        assert_eq!(a.kid(), b.kid());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
