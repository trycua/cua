// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The tokens the issuer mints: RS256 JWTs a customer's cloud trusts.
//!
//! | claim | value |
//! |---|---|
//! | `iss` | `{base}/a/{acct}` (no trailing slash) |
//! | `sub` | `account:{acct}` |
//! | `aud` | the cloud's audience (a single string) |
//! | `iat`, `nbf`, `exp` | `exp - iat` = 900 s; `nbf` = `iat` |
//! | `jti` | random, audited |
//! | `cua_conn`, `cua_device`, `cua_purpose` | connection, `dev_…`, purpose |
//! | `cua_agent` | the agent's name, only when an agent asked |
//!
//! The header carries `kid` (the active key's thumbprint) and `typ: JWT`.

use base64::Engine as _;
use serde::Serialize;

use crate::jwk::b64;
use crate::signer::{SignError, Signer};

/// Token lifetime: AWS's minimum role session.
pub const TOKEN_TTL_SECS: u64 = 900;

/// The claims of a minted token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Claims {
    /// Issuer.
    pub iss: String,
    /// `account:{acct}`.
    pub sub: String,
    /// Audience.
    pub aud: String,
    /// Issued at.
    pub iat: u64,
    /// Not before.
    pub nbf: u64,
    /// Expiry.
    pub exp: u64,
    /// Token id.
    pub jti: String,
    /// Connection id.
    pub cua_conn: String,
    /// Device id.
    pub cua_device: String,
    /// Agent name, when an agent asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cua_agent: Option<String>,
    /// Purpose.
    pub cua_purpose: String,
}

/// The per-account issuer URL.
pub fn issuer_url(base: &str, acct: &str) -> String {
    format!("{}/a/{acct}", base.trim_end_matches('/'))
}

/// A random token id.
pub fn new_jti() -> String {
    use rand::RngCore as _;
    let mut buf = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    b64().encode(buf)
}

/// Signs `claims` with the active key; the signature is checked against
/// the published public key before the token leaves (a KMS key that does
/// not match its JWKS entry fails here, not at the cloud).
pub async fn sign(signer: &dyn Signer, claims: &Claims) -> Result<String, SignError> {
    let header = serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": signer.kid()});
    let input = format!(
        "{}.{}",
        b64().encode(serde_json::to_vec(&header).expect("json")),
        b64().encode(serde_json::to_vec(claims).expect("json"))
    );
    let sig = signer.sign(input.as_bytes()).await?;
    let public = signer.public_key();
    ring::signature::RsaPublicKeyComponents {
        n: &public.n,
        e: &public.e,
    }
    .verify(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        input.as_bytes(),
        &sig,
    )
    .map_err(|_| SignError("the signature does not match the published key".into()))?;
    Ok(format!("{input}.{}", b64().encode(sig)))
}
