// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Device proofs: a DPoP-style (RFC 9449) JWS that the device signs with its
//! enrolled P-256 key, sent in the `DPoP` header next to the Keycloak access
//! token. It binds one request to one device: a stolen access token alone
//! mints nothing.
//!
//! Format (compact JWS):
//!
//! - header: `{"typ": "dpop+jwt", "alg": "ES256", "jwk": {"kty": "EC",
//!   "crv": "P-256", "x": …, "y": …}}` (the device's public key, no `d`);
//! - payload: `{"htu", "htm", "iat", "jti", "ath", "connection", "purpose",
//!   "agent"?}`:
//!   - `htu`: the full request URL without query, on the issuer's public
//!     base URL (`https://oidc.cua.ai/v1/token`);
//!   - `htm`: the HTTP method (`POST`, `DELETE`);
//!   - `iat`: Unix seconds, within ±60 s of the issuer's clock;
//!   - `jti`: 16-128 random characters, never reused (a replay is refused);
//!   - `ath`: base64url SHA-256 of the access token sent with it;
//!   - `connection`: the connection id (`""` when creating one);
//!   - `purpose`: for `POST /v1/token` the token purpose (`create`,
//!     `manage`, `check`, `cost`); for connection changes the action
//!     (`connection.create`, `connection.limits`, `connection.approve`,
//!     `connection.revoke`, `connection.delete`, `connection.spend`);
//!   - `agent`: the agent or routine the daemon acts for; absent or `null`
//!     when a person acts.
//! - signature: ES256, the 64-byte `r || s` form.
//!
//! The device id is derived from the `jwk` ([`crate::devices::device_id`]),
//! so the issuer needs no separate device claim.

use base64::Engine as _;
use serde::Deserialize;

use crate::devices::{device_id, EcJwk};
use crate::jwk::b64;

/// Accepted clock skew of `iat`.
pub const PROOF_SKEW_SECS: u64 = 60;
/// Header carrying the proof.
pub const PROOF_HEADER: &str = "dpop";

/// A verified proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    /// `dev_…` of the signing key.
    pub device: String,
    /// The signing key.
    pub jwk: EcJwk,
    /// `jti`.
    pub jti: String,
    /// `iat`.
    pub iat: u64,
    /// `connection`.
    pub connection: String,
    /// `purpose`.
    pub purpose: String,
    /// `agent`.
    pub agent: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    typ: Option<String>,
    alg: String,
    jwk: serde_json::Value,
}

#[derive(Deserialize)]
struct Claims {
    htu: String,
    htm: String,
    iat: u64,
    jti: String,
    ath: String,
    connection: String,
    purpose: String,
    #[serde(default)]
    agent: Option<String>,
}

/// What the request the proof must match looks like.
#[derive(Debug, Clone, Copy)]
pub struct Expect<'a> {
    /// HTTP method.
    pub htm: &'a str,
    /// The request URL on the public base URL, without query.
    pub htu: &'a str,
    /// The access token sent with the proof.
    pub access_token: &'a str,
    /// Unix seconds now.
    pub now: u64,
}

/// Verifies a proof's signature, freshness and request binding. The
/// replay check (`jti`) and the claim match against the body are the
/// caller's.
pub fn verify(proof: &str, expect: Expect<'_>) -> Result<Proof, String> {
    let mut parts = proof.trim().split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("the device proof is not a compact JWS".into());
    };
    let header: Header = b64()
        .decode(h)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("the device proof header is not valid JSON")?;
    if header.typ.as_deref() != Some("dpop+jwt") {
        return Err("the device proof must have typ dpop+jwt".into());
    }
    if header.alg != "ES256" {
        return Err("the device proof must be ES256".into());
    }
    if header.jwk.get("d").is_some() {
        return Err("the device proof's jwk must not hold a private key".into());
    }
    let jwk: EcJwk = serde_json::from_value(header.jwk)
        .map_err(|_| "the device proof's jwk is not a P-256 key")?;
    let point = jwk
        .uncompressed()
        .ok_or("the device proof's jwk is not a P-256 key")?;
    let sig = b64()
        .decode(s)
        .map_err(|_| "the device proof signature is not base64url")?;
    let signed = &proof.trim()[..h.len() + 1 + p.len()];
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, &point)
        .verify(signed.as_bytes(), &sig)
        .map_err(|_| "the device proof signature does not verify")?;
    let claims: Claims = b64()
        .decode(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("the device proof claims are missing or malformed")?;
    if !claims.htm.eq_ignore_ascii_case(expect.htm) {
        return Err(format!(
            "the device proof is for {}, not {}",
            claims.htm, expect.htm
        ));
    }
    let strip = |u: &str| u.split(['?', '#']).next().unwrap_or_default().to_owned();
    if strip(&claims.htu) != strip(expect.htu) {
        return Err(format!(
            "the device proof is for {}, not {}",
            claims.htu, expect.htu
        ));
    }
    if claims.iat.abs_diff(expect.now) > PROOF_SKEW_SECS {
        return Err("the device proof is too old or from the future (check the clock)".into());
    }
    if !(16..=128).contains(&claims.jti.len()) || claims.jti.chars().any(|c| c.is_control()) {
        return Err("the device proof's jti must be 16-128 characters".into());
    }
    let ath = b64().encode(
        ring::digest::digest(&ring::digest::SHA256, expect.access_token.as_bytes()).as_ref(),
    );
    {
        use subtle::ConstantTimeEq as _;
        if claims.ath.len() != ath.len() || !bool::from(claims.ath.as_bytes().ct_eq(ath.as_bytes()))
        {
            return Err("the device proof is bound to another access token".into());
        }
    }
    let agent = claims.agent.filter(|a| !a.is_empty());
    if let Some(a) = &agent {
        if !crate::model::valid_agent(a) {
            return Err("the device proof's agent must be 1-128 printable characters".into());
        }
    }
    Ok(Proof {
        device: device_id(&point),
        jwk,
        jti: claims.jti,
        iat: claims.iat,
        connection: claims.connection,
        purpose: claims.purpose,
        agent,
    })
}

/// Builds proofs (tests, and a reference for clients).
pub mod testing {
    use super::*;
    use ring::signature::KeyPair as _;

    /// A device key.
    pub struct DeviceKey {
        pair: ring::signature::EcdsaKeyPair,
    }

    impl Default for DeviceKey {
        fn default() -> Self {
            Self::generate()
        }
    }

    impl DeviceKey {
        /// A fresh P-256 key.
        pub fn generate() -> Self {
            let rng = ring::rand::SystemRandom::new();
            let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
            let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).expect("p256");
            Self {
                pair: ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng)
                    .expect("p256"),
            }
        }

        /// The public key as a JWK.
        pub fn jwk(&self) -> EcJwk {
            let p = self.pair.public_key().as_ref();
            EcJwk {
                kty: "EC".into(),
                crv: "P-256".into(),
                x: b64().encode(&p[1..33]),
                y: b64().encode(&p[33..65]),
            }
        }

        /// `dev_…`.
        pub fn id(&self) -> String {
            device_id(self.pair.public_key().as_ref())
        }

        /// Signs a proof with arbitrary header and claims.
        pub fn sign_raw(&self, header: &serde_json::Value, claims: &serde_json::Value) -> String {
            let input = format!(
                "{}.{}",
                b64().encode(serde_json::to_vec(header).expect("json")),
                b64().encode(serde_json::to_vec(claims).expect("json"))
            );
            let rng = ring::rand::SystemRandom::new();
            let sig = self.pair.sign(&rng, input.as_bytes()).expect("sign");
            format!("{input}.{}", b64().encode(sig.as_ref()))
        }

        /// A proof for one request.
        #[allow(clippy::too_many_arguments)]
        pub fn proof(
            &self,
            htm: &str,
            htu: &str,
            access_token: &str,
            connection: &str,
            purpose: &str,
            agent: Option<&str>,
            iat: u64,
            jti: &str,
        ) -> String {
            let ath = b64().encode(
                ring::digest::digest(&ring::digest::SHA256, access_token.as_bytes()).as_ref(),
            );
            self.sign_raw(
                &serde_json::json!({"typ": "dpop+jwt", "alg": "ES256", "jwk": self.jwk()}),
                &serde_json::json!({
                    "htu": htu, "htm": htm, "iat": iat, "jti": jti, "ath": ath,
                    "connection": connection, "purpose": purpose, "agent": agent,
                }),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::DeviceKey;
    use super::*;

    const HTU: &str = "https://oidc.test/v1/token";

    fn expect(now: u64) -> Expect<'static> {
        Expect {
            htm: "POST",
            htu: HTU,
            access_token: "access",
            now,
        }
    }

    #[test]
    fn a_good_proof_verifies_and_names_the_device() {
        let key = DeviceKey::generate();
        let p = key.proof(
            "POST",
            HTU,
            "access",
            "conn_1",
            "create",
            Some("ci"),
            1000,
            "jti-0123456789abcdef",
        );
        let proof = verify(&p, expect(1030)).unwrap();
        assert_eq!(proof.device, key.id());
        assert_eq!(proof.agent.as_deref(), Some("ci"));
        assert_eq!(proof.connection, "conn_1");
        assert_eq!(proof.purpose, "create");
    }

    #[test]
    fn bad_proofs_are_refused() {
        let key = DeviceKey::generate();
        let jti = "jti-0123456789abcdef";
        let ok = |p: String| verify(&p, expect(1000));
        // Stale, wrong method, wrong URL, other access token, short jti.
        assert!(ok(key.proof("POST", HTU, "access", "c", "create", None, 900, jti)).is_err());
        assert!(ok(key.proof("GET", HTU, "access", "c", "create", None, 1000, jti)).is_err());
        assert!(ok(key.proof(
            "POST",
            "https://evil/v1/token",
            "access",
            "c",
            "create",
            None,
            1000,
            jti
        ))
        .is_err());
        assert!(ok(key.proof("POST", HTU, "other", "c", "create", None, 1000, jti)).is_err());
        assert!(ok(key.proof("POST", HTU, "access", "c", "create", None, 1000, "short")).is_err());
        // Signed by another key than the header names.
        let other = DeviceKey::generate();
        let forged = other.sign_raw(
            &serde_json::json!({"typ": "dpop+jwt", "alg": "ES256", "jwk": key.jwk()}),
            &serde_json::json!({"htu": HTU, "htm": "POST", "iat": 1000, "jti": jti,
                "ath": b64().encode(ring::digest::digest(&ring::digest::SHA256, b"access").as_ref()),
                "connection": "c", "purpose": "create"}),
        );
        assert!(ok(forged).is_err());
        // alg none / wrong typ.
        let none = key.sign_raw(
            &serde_json::json!({"typ": "dpop+jwt", "alg": "none", "jwk": key.jwk()}),
            &serde_json::json!({}),
        );
        assert!(ok(none).is_err());
        let typ = key.sign_raw(
            &serde_json::json!({"typ": "JWT", "alg": "ES256", "jwk": key.jwk()}),
            &serde_json::json!({}),
        );
        assert!(ok(typ).is_err());
        assert!(ok("a.b".into()).is_err());
    }
}
