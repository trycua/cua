// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! RS256 signing keys and the published key set.
//!
//! - [`Signer`]: signs with RSASSA-PKCS1-v1_5 SHA-256. [`FileSigner`] holds a
//!   PKCS#8 RSA key (tests, self-hosted issuers); `KmsSigner` (feature
//!   `kms`) calls AWS KMS `Sign`, and the private key never leaves KMS.
//! - [`KeySet`]: the active signer plus the optional next and previous
//!   public keys. The JWKS lists all three, so a rotation never strands a
//!   token: the next key is published before signing switches to it, and
//!   the previous key stays published after.
//!
//! Key specs (`CUA_OIDC_SIGNING_KEY`, `CUA_OIDC_NEXT_KEY`,
//! `CUA_OIDC_PREVIOUS_KEY`): `file:<path>` (a PKCS#8 RSA private key, PEM
//! or DER; for next/previous also a `PUBLIC KEY` SubjectPublicKeyInfo) or
//! `kms:<key id or ARN>`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::jwk::{pem_or_der, Jwk, RsaPublicKey};

/// A signing failure (KMS unreachable, throttled, key disabled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignError(pub String);

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SignError {}

/// An RS256 signer.
#[async_trait]
pub trait Signer: Send + Sync {
    /// The public key.
    fn public_key(&self) -> &RsaPublicKey;
    /// RSASSA-PKCS1-v1_5 SHA-256 signature of `message`.
    async fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SignError>;
    /// The `kid`: the public key's RFC 7638 thumbprint.
    fn kid(&self) -> String {
        self.public_key().thumbprint()
    }
}

/// Signs with a PKCS#8 RSA private key held in memory.
pub struct FileSigner {
    pair: ring::signature::RsaKeyPair,
    public: RsaPublicKey,
}

impl std::fmt::Debug for FileSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSigner")
            .field("kid", &self.public.thumbprint())
            .finish()
    }
}

impl FileSigner {
    /// From a PKCS#8 RSA private key, PEM (`PRIVATE KEY`) or DER.
    pub fn from_pkcs8(input: &[u8]) -> Result<Self, String> {
        let der = pem_or_der(input, "PRIVATE KEY")?;
        let pair = ring::signature::RsaKeyPair::from_pkcs8(&der)
            .map_err(|e| format!("not a PKCS#8 RSA private key: {e}"))?;
        let public = RsaPublicKey::from_pkcs1_der(pair.public().as_ref())?;
        Ok(Self { pair, public })
    }

    /// Reads [`Self::from_pkcs8`] from a file.
    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_pkcs8(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[async_trait]
impl Signer for FileSigner {
    fn public_key(&self) -> &RsaPublicKey {
        &self.public
    }

    async fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SignError> {
        let rng = ring::rand::SystemRandom::new();
        let mut sig = vec![0u8; self.pair.public().modulus_len()];
        self.pair
            .sign(&ring::signature::RSA_PKCS1_SHA256, &rng, message, &mut sig)
            .map_err(|_| SignError("RSA signing failed".into()))?;
        Ok(sig)
    }
}

/// The active signer and the other published keys.
#[derive(Clone)]
pub struct KeySet {
    active: Arc<dyn Signer>,
    next: Option<RsaPublicKey>,
    previous: Option<RsaPublicKey>,
}

impl std::fmt::Debug for KeySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeySet")
            .field("active", &self.active.kid())
            .field("next", &self.next.as_ref().map(|k| k.thumbprint()))
            .field("previous", &self.previous.as_ref().map(|k| k.thumbprint()))
            .finish()
    }
}

impl KeySet {
    /// Only an active key.
    pub fn new(active: Arc<dyn Signer>) -> Self {
        Self {
            active,
            next: None,
            previous: None,
        }
    }

    /// Publishes `key` as the next key (signing switches to it later).
    pub fn with_next(mut self, key: RsaPublicKey) -> Self {
        self.next = Some(key);
        self
    }

    /// Keeps publishing `key`, the key signing used before.
    pub fn with_previous(mut self, key: RsaPublicKey) -> Self {
        self.previous = Some(key);
        self
    }

    /// The active signer.
    pub fn active(&self) -> &Arc<dyn Signer> {
        &self.active
    }

    /// The published keys: active, next, previous (duplicates dropped).
    pub fn jwks(&self) -> Vec<Jwk> {
        let mut out: Vec<Jwk> = Vec::with_capacity(3);
        for key in [
            Some(self.active.public_key()),
            self.next.as_ref(),
            self.previous.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let jwk = key.jwk();
            if !out.iter().any(|k| k.kid == jwk.kid) {
                out.push(jwk);
            }
        }
        out
    }
}

/// A parsed key spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySpec {
    /// `file:<path>`.
    File(std::path::PathBuf),
    /// `kms:<key id or ARN>`.
    Kms(String),
}

impl std::str::FromStr for KeySpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some(path) = s.strip_prefix("file:") {
            return Ok(Self::File(path.into()));
        }
        if let Some(id) = s.strip_prefix("kms:") {
            if id.is_empty() {
                return Err("kms: needs a key id or ARN".into());
            }
            return Ok(Self::Kms(id.to_owned()));
        }
        Err(format!("{s:?}: use file:<path> or kms:<key id>"))
    }
}

/// Loads the signer of `spec`.
pub async fn load_signer(spec: &KeySpec) -> Result<Arc<dyn Signer>, String> {
    match spec {
        KeySpec::File(path) => Ok(Arc::new(FileSigner::from_file(path)?)),
        #[cfg(feature = "kms")]
        KeySpec::Kms(id) => Ok(Arc::new(crate::kms::KmsSigner::connect(id).await?)),
        #[cfg(not(feature = "kms"))]
        KeySpec::Kms(_) => Err("this build has no KMS support (cargo feature `kms`)".into()),
    }
}

/// Loads only the public key of `spec` (next / previous keys).
pub async fn load_public(spec: &KeySpec) -> Result<RsaPublicKey, String> {
    match spec {
        KeySpec::File(path) => {
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if let Ok(signer) = FileSigner::from_pkcs8(&bytes) {
                return Ok(signer.public);
            }
            let der = pem_or_der(&bytes, "PUBLIC KEY")?;
            RsaPublicKey::from_spki_der(&der).map_err(|e| format!("{}: {e}", path.display()))
        }
        #[cfg(feature = "kms")]
        KeySpec::Kms(id) => crate::kms::public_key(id).await,
        #[cfg(not(feature = "kms"))]
        KeySpec::Kms(_) => Err("this build has no KMS support (cargo feature `kms`)".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_signer_signs_rs256_that_verifies() {
        let signer =
            FileSigner::from_pkcs8(include_bytes!("../tests/fixtures/test-rsa-a.pk8.der")).unwrap();
        let sig = signer.sign(b"hello").await.unwrap();
        let public = signer.public_key();
        ring::signature::RsaPublicKeyComponents {
            n: &public.n,
            e: &public.e,
        }
        .verify(&ring::signature::RSA_PKCS1_2048_8192_SHA256, b"hello", &sig)
        .unwrap();
        assert_eq!(signer.kid(), public.thumbprint());
    }

    #[tokio::test]
    async fn public_keys_load_from_a_spki_pem_or_a_private_key() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let from_pem = load_public(&KeySpec::File(dir.join("test-rsa-c.pub.pem")))
            .await
            .unwrap();
        let from_private = load_public(&KeySpec::File(dir.join("test-rsa-c.pk8.der")))
            .await
            .unwrap();
        assert_eq!(from_pem, from_private);
        assert!(load_signer(&KeySpec::File(dir.join("test-rsa-c.pub.pem")))
            .await
            .is_err());
        #[cfg(not(feature = "kms"))]
        assert!(load_signer(&KeySpec::Kms("alias/x".into())).await.is_err());
    }

    #[test]
    fn key_specs() {
        assert_eq!(
            "file:/k.pem".parse::<KeySpec>().unwrap(),
            KeySpec::File("/k.pem".into())
        );
        assert_eq!(
            "kms:alias/cua-oidc".parse::<KeySpec>().unwrap(),
            KeySpec::Kms("alias/cua-oidc".into())
        );
        assert!("kms:".parse::<KeySpec>().is_err());
        assert!("/k.pem".parse::<KeySpec>().is_err());
    }
}
