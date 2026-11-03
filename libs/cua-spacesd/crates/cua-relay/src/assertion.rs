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
    /// Teams (or organizations) the account reached this machine through,
    /// empty for the owner and for a direct share. Additive: verifiers that
    /// predate it ignore the field, and an assertion without it parses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<String>,
    /// A thread grant ([`THREAD_ROLE`]): the one Space (relay machine id)
    /// the machine provides whose thread the account may reach, because
    /// that Space is shared with it. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// A thread grant's role on that Space: `viewer` or `shared`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_role: Option<String>,
}

/// The `role` of a thread grant: an account the machine is not shared with
/// reaching the thread of one Space it provides ([`AssertionClaims::space`]).
/// A machine that predates it refuses the call (the role is neither
/// `viewer` nor `shared`).
pub const THREAD_ROLE: &str = "thread";

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
        remove_stale_temps(path)?;
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

    /// Signs `claims` as a relay-only token of type `typ` (the JWT `typ`
    /// header). Machines never accept these: only [`RelayKey::verify_json`]
    /// with the same `typ` does, so a browser session or a login code can
    /// never stand in for a principal assertion, or the other way round.
    pub fn sign_json<T: serde::Serialize>(&self, typ: &str, claims: &T) -> String {
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.kid.clone());
        header.typ = Some(typ.to_owned());
        jsonwebtoken::encode(&header, claims, &self.encoding).expect("EdDSA signing")
    }

    /// Verifies a token made by [`RelayKey::sign_json`] with this key and
    /// the same `typ`. Expiry is the caller's to check (`exp` in the
    /// claims).
    pub fn verify_json<T: serde::de::DeserializeOwned>(
        &self,
        typ: &str,
        token: &str,
    ) -> Result<T, String> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| "not a token".to_owned())?;
        if header.alg != Algorithm::EdDSA || header.typ.as_deref() != Some(typ) {
            return Err("wrong token type".into());
        }
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.validate_exp = false;
        validation.validate_aud = false;
        validation.required_spec_claims.clear();
        jsonwebtoken::decode::<T>(token, &DecodingKey::from_ed_der(&self.public), &validation)
            .map(|d| d.claims)
            .map_err(|e| format!("bad signature: {e}"))
    }
}

/// Writes a secret file readable only by its owner. The bytes go to a
/// fresh, exclusively created 0600 temp file (never an existing file or
/// symlink) that is renamed over `path`, so `path` holds either the old or
/// the new content, never a mix.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    write_private_with(path, |w| w.write_all(bytes))
}

/// [`write_private`], streaming: `fill` writes the content through a
/// buffer, so a large file is never held in memory whole.
pub fn write_private_with(
    path: &std::path::Path,
    fill: impl FnOnce(&mut dyn std::io::Write) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write as _;
    let (dir, name) = temp_parts(path);
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
        let file = match options.open(&tmp) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        let mut out = std::io::BufWriter::new(file);
        let written = fill(&mut out)
            .and_then(|()| out.flush())
            .and_then(|()| out.get_ref().sync_all());
        drop(out);
        if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        // The rename itself is durable only once the directory is synced.
        #[cfg(unix)]
        std::fs::File::open(dir).and_then(|d| d.sync_all())?;
        return Ok(());
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free temp name")))
}

fn temp_parts(path: &std::path::Path) -> (&std::path::Path, String) {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => std::path::Path::new("."),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    (dir, name)
}

/// Removes `path`; `Ok(false)` when someone else removed it first.
///
/// On Windows a file another thread or process is deleting stays "delete
/// pending" until that deleter closes its handle, and opening it meanwhile
/// (which `remove_file` does) fails with ERROR_ACCESS_DENIED rather than
/// not-found. That state lasts only until the other delete completes, so
/// wait for it briefly and then see the file gone.
pub(crate) fn remove_if_present(path: &std::path::Path) -> std::io::Result<bool> {
    let mut attempt = 0;
    loop {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e)
                if cfg!(windows)
                    && e.kind() == std::io::ErrorKind::PermissionDenied
                    && attempt < 100 =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Deletes the temp files [`write_private`] leaves next to `path` when the
/// process dies mid-write (an OOM kill): `.<name>.<uuid>.tmp`, which also
/// covers sibling files named `<name>.<suffix>`. Each can be as large as
/// the file itself, so a few of them fill a small volume. Call it at
/// startup, before this process writes `path` (one writer per state file).
/// Returns how many files and bytes it removed.
pub fn remove_stale_temps(path: &std::path::Path) -> std::io::Result<(usize, u64)> {
    let (dir, name) = temp_parts(path);
    let prefix = format!(".{name}.");
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(e),
    };
    let (mut files, mut bytes) = (0, 0);
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !file_name.starts_with(&prefix) || !file_name.ends_with(".tmp") {
            continue;
        }
        // Gone between the listing and here (its writer renamed it): fine.
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if !meta.is_file() {
            continue;
        }
        if remove_if_present(&entry.path())? {
            files += 1;
            bytes += meta.len();
        }
    }
    if files > 0 {
        tracing::warn!(
            path = %path.display(),
            files,
            bytes,
            "removed temp files left by an interrupted write"
        );
    }
    Ok((files, bytes))
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

/// The relay public keys a machine trusts, plus the in-process key the
/// machine's own mesh endpoint signs with (never leaves the process).
#[derive(Default)]
pub struct TrustedKeys {
    keys: RwLock<Vec<(Option<String>, DecodingKey)>>,
    local: RwLock<Vec<(Option<String>, DecodingKey)>>,
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

    /// Also trusts `key`, an in-process key (the mesh endpoint's, which
    /// vouches for peers the relay's network map identified). Replacing the
    /// relay's keys never removes it.
    pub fn trust_local(&self, key: &RelayKey) -> Result<(), String> {
        let set: JwkSet =
            serde_json::from_value(key.jwks()).map_err(|e| format!("local JWKS: {e}"))?;
        let jwk = set.keys.first().ok_or("local JWKS has no key")?;
        let decoding = DecodingKey::from_jwk(jwk).map_err(|e| format!("local key: {e}"))?;
        self.local
            .write()
            .expect("local")
            .push((jwk.common.key_id.clone(), decoding));
        Ok(())
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
        let local = self.local.read().expect("local");
        if keys.is_empty() && local.is_empty() {
            return Err("no relay key is trusted".into());
        }
        let mut last = String::from("assertion: no matching relay key");
        for (kid, key) in keys.iter().chain(local.iter()) {
            if header.kid.is_some() && kid.is_some() && header.kid != *kid {
                continue;
            }
            match jsonwebtoken::decode::<AssertionClaims>(token, key, &validation) {
                Ok(data) => {
                    let c = data.claims;
                    if c.mid != machine_id {
                        return Err("assertion: computer id mismatch".into());
                    }
                    if c.exp < c.iat || c.exp - c.iat > MAX_ASSERTION_TTL_SECS {
                        return Err("assertion: lifetime too long".into());
                    }
                    if c.iat > now_secs() + LEEWAY_SECS {
                        return Err(
                            "assertion: issued in the future; the clock on this machine or on the \
                             machine that checks it is off, so set both to automatic time"
                                .into(),
                        );
                    }
                    return Ok(c);
                }
                Err(e) => last = assertion_error(&e),
            }
        }
        Err(last)
    }
}

/// The words for a token that did not verify. An expired or not yet valid
/// token is nearly always a clock that is off, so it says so (it used to
/// read `assertion: ExpiredSignature`, and the machine looked broken for no
/// reason a person could find).
fn assertion_error(e: &jsonwebtoken::errors::Error) -> String {
    use jsonwebtoken::errors::ErrorKind as K;
    match e.kind() {
        K::ExpiredSignature => "assertion: the sign-in proof has expired; the clock on this \
             machine or on the machine that checks it is more than a few minutes off, so set both \
             to automatic time"
            .into(),
        K::ImmatureSignature => "assertion: the sign-in proof is not valid yet; the clock on this \
             machine or on the machine that checks it is more than a few minutes off, so set both \
             to automatic time"
            .into(),
        _ => format!("assertion: {e}"),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn loading_the_signing_key_removes_an_interrupted_writes_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("signing.key");
        let stale = dir.path().join(".signing.key.0123abcd.tmp");
        std::fs::write(&stale, "partial").unwrap();
        let key = RelayKey::load_or_create(&path).unwrap();
        assert!(!stale.exists());
        assert_eq!(RelayKey::load_or_create(&path).unwrap().kid(), key.kid());
    }

    #[test]
    fn removing_stale_temps_racing_their_removal_does_not_fail() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let names: Vec<_> = (0..4000)
            .map(|_| {
                let id = uuid::Uuid::new_v4().simple();
                dir.path().join(format!(".devices.json.{id}.tmp"))
            })
            .collect();
        for name in &names {
            std::fs::write(name, "partial").unwrap();
        }
        let barrier = Arc::new(Barrier::new(2));
        let remover = {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for name in &names {
                    let _ = std::fs::remove_file(name);
                }
            })
        };
        barrier.wait();
        let removed = super::remove_stale_temps(&path);
        remover.join().unwrap();
        removed.unwrap();
    }

    #[test]
    fn stale_temps_of_the_file_and_its_siblings_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        std::fs::write(&path, "live").unwrap();
        for stale in [
            ".devices.json.0123abcd.tmp",
            ".devices.json.audit.jsonl.4567ef.tmp",
        ] {
            std::fs::write(dir.path().join(stale), "partial").unwrap();
        }
        for kept in [
            ".machines.json.89ab.tmp",
            "devices.json.audit-key",
            ".devices.json",
        ] {
            std::fs::write(dir.path().join(kept), "keep").unwrap();
        }
        assert_eq!(remove_stale_temps(&path).unwrap(), (2, 14));
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                ".devices.json",
                ".machines.json.89ab.tmp",
                "devices.json",
                "devices.json.audit-key"
            ]
        );
        assert_eq!(remove_stale_temps(&path).unwrap(), (0, 0));
        assert_eq!(
            remove_stale_temps(&dir.path().join("missing/devices.json")).unwrap(),
            (0, 0)
        );
    }

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
            teams: Vec::new(),
            space: None,
            space_role: None,
        }
    }

    #[test]
    fn teams_claim_is_additive() {
        // An assertion from a relay that predates `teams` still parses...
        let mut v = serde_json::to_value(claims("machine-01", 60)).unwrap();
        assert!(v.get("teams").is_none(), "empty teams are not serialized");
        let parsed: AssertionClaims = serde_json::from_value(v.clone()).unwrap();
        assert!(parsed.teams.is_empty());
        // ...and an older verifier (which has no such field) ignores it.
        v["teams"] = serde_json::json!(["team-a", "team-b"]);
        let with: AssertionClaims = serde_json::from_value(v).unwrap();
        assert_eq!(with.teams, ["team-a", "team-b"]);
        let key = RelayKey::generate();
        let trusted = TrustedKeys::default();
        trusted
            .set_from_header(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.jwks().to_string()),
            )
            .unwrap();
        let mut c = claims("machine-01", 60);
        c.teams = vec!["team-a".into()];
        assert_eq!(
            trusted.verify(&key.sign(&c), "machine-01").unwrap().teams,
            ["team-a"]
        );
    }

    #[test]
    fn relay_only_tokens_are_typed_and_never_assertions() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct C {
            who: String,
        }
        let key = RelayKey::generate();
        let c = C { who: "ada".into() };
        let token = key.sign_json("share-session", &c);
        assert_eq!(key.verify_json::<C>("share-session", &token).unwrap(), c);
        // Another type, another relay key, a tampered body: refused.
        assert!(key.verify_json::<C>("share-code", &token).is_err());
        assert!(RelayKey::generate()
            .verify_json::<C>("share-session", &token)
            .is_err());
        let mut parts: Vec<&str> = token.split('.').collect();
        let forged = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"who":"eve"}"#);
        parts[1] = &forged;
        assert!(key
            .verify_json::<C>("share-session", &parts.join("."))
            .is_err());
        // A machine's verifier never takes one as an assertion, and the
        // relay never takes an assertion as one.
        let trusted = TrustedKeys::default();
        trusted
            .set_from_header(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.jwks().to_string()),
            )
            .unwrap();
        assert!(trusted.verify(&token, "machine-01").is_err());
        assert!(key
            .verify_json::<C>("share-session", &key.sign(&claims("machine-01", 60)))
            .is_err());
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
        let e = trusted.verify(&key.sign(&old), "machine-01").unwrap_err();
        // A clock that is off is named, not `ExpiredSignature`.
        assert!(e.contains("clock") && e.contains("automatic time"), "{e}");
        assert!(!e.contains("ExpiredSignature"), "{e}");
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
