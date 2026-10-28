// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Signing with an AWS KMS asymmetric key (`RSA_2048`, key usage
//! `SIGN_VERIFY`), through the official AWS SDK for Rust (feature `kms`).
//!
//! The private key never leaves KMS. Credentials come from the default
//! chain: IRSA (web identity) or the instance role in the cluster. The
//! service needs `kms:Sign` and `kms:GetPublicKey` on the active key and
//! `kms:GetPublicKey` on the next and previous keys.

use async_trait::async_trait;
use aws_sdk_kms::primitives::Blob;
use aws_sdk_kms::types::{MessageType, SigningAlgorithmSpec};

use crate::jwk::RsaPublicKey;
use crate::signer::{SignError, Signer};

/// Signs with `kms:Sign`, `RSASSA_PKCS1_V1_5_SHA_256`.
pub struct KmsSigner {
    client: aws_sdk_kms::Client,
    key_id: String,
    public: RsaPublicKey,
}

impl std::fmt::Debug for KmsSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KmsSigner")
            .field("key_id", &self.key_id)
            .field("kid", &self.public.thumbprint())
            .finish()
    }
}

async fn client() -> aws_sdk_kms::Client {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    aws_sdk_kms::Client::new(&config)
}

async fn fetch_public(client: &aws_sdk_kms::Client, key_id: &str) -> Result<RsaPublicKey, String> {
    let out = client
        .get_public_key()
        .key_id(key_id)
        .send()
        .await
        .map_err(|e| {
            format!(
                "KMS GetPublicKey {key_id}: {}",
                aws_sdk_kms::error::DisplayErrorContext(e)
            )
        })?;
    if !out
        .signing_algorithms()
        .contains(&SigningAlgorithmSpec::RsassaPkcs1V15Sha256)
    {
        return Err(format!(
            "KMS key {key_id} does not support RSASSA_PKCS1_V1_5_SHA_256"
        ));
    }
    let der = out
        .public_key()
        .ok_or_else(|| format!("KMS key {key_id} returned no public key"))?;
    RsaPublicKey::from_spki_der(der.as_ref()).map_err(|e| format!("KMS key {key_id}: {e}"))
}

/// The public key of KMS key `key_id`.
pub async fn public_key(key_id: &str) -> Result<RsaPublicKey, String> {
    fetch_public(&client().await, key_id).await
}

impl KmsSigner {
    /// Connects and reads the key's public half (fails fast at start when
    /// the key is missing, of the wrong type, or not permitted).
    pub async fn connect(key_id: &str) -> Result<Self, String> {
        let client = client().await;
        let public = fetch_public(&client, key_id).await?;
        Ok(Self {
            client,
            key_id: key_id.to_owned(),
            public,
        })
    }
}

#[async_trait]
impl Signer for KmsSigner {
    fn public_key(&self) -> &RsaPublicKey {
        &self.public
    }

    async fn sign(&self, message: &[u8]) -> Result<Vec<u8>, SignError> {
        // Send the digest, not the message: KMS caps RAW messages at 4 KiB.
        let digest = ring::digest::digest(&ring::digest::SHA256, message);
        let out = self
            .client
            .sign()
            .key_id(&self.key_id)
            .message(Blob::new(digest.as_ref()))
            .message_type(MessageType::Digest)
            .signing_algorithm(SigningAlgorithmSpec::RsassaPkcs1V15Sha256)
            .send()
            .await
            .map_err(|e| {
                SignError(format!(
                    "KMS Sign: {}",
                    aws_sdk_kms::error::DisplayErrorContext(e)
                ))
            })?;
        out.signature()
            .map(|b| b.as_ref().to_vec())
            .ok_or_else(|| SignError("KMS Sign returned no signature".into()))
    }
}
