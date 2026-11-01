// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Keycloak access-token validation: the caller's normal cua.ai sign-in,
//! checked like the relay checks it (`cua-relay/src/oidc.rs`): signature
//! against the realm's JWKS (refetched on an unknown `kid`, at most every
//! 30 s, and hourly), issuer, audience (`cua-oidc` by default) and expiry.
//! Symmetric (`HS*`) tokens are refused.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};

/// Default audience the access token must carry.
pub const DEFAULT_AUDIENCE: &str = "cua-oidc";

const REFRESH_COOLDOWN: Duration = Duration::from_secs(30);
const MAX_KEY_AGE: Duration = Duration::from_secs(3600);

/// Keycloak settings.
#[derive(Debug, Clone)]
pub struct KeycloakConfig {
    /// Expected `iss` (e.g. `https://auth.cua.ai/realms/cyclops-cs`).
    pub issuer: String,
    /// JWKS URL; discovered from the issuer when unset.
    pub jwks_url: Option<String>,
    /// Accepted audiences (any of them).
    pub audiences: Vec<String>,
    /// Claim naming the account (default `sub`, as on the relay).
    pub account_claim: String,
}

impl KeycloakConfig {
    /// `issuer` with the default audience and account claim.
    pub fn new(issuer: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            jwks_url: None,
            audiences: vec![DEFAULT_AUDIENCE.into()],
            account_claim: "sub".into(),
        }
    }
}

/// A signed-in user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// `sub`.
    pub user: String,
    /// The account (the configured claim; the relay's device `account`).
    pub account: String,
}

struct Keys {
    set: Option<JwkSet>,
    fetched: Option<Instant>,
}

/// Validates Keycloak access tokens.
pub struct KeycloakValidator {
    config: KeycloakConfig,
    http: reqwest::Client,
    keys: RwLock<Keys>,
    fetch: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for KeycloakValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeycloakValidator")
            .field("issuer", &self.config.issuer)
            .finish()
    }
}

impl KeycloakValidator {
    /// Fetches keys from the issuer on demand.
    pub fn new(config: KeycloakConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("http client"),
            keys: RwLock::new(Keys {
                set: None,
                fetched: None,
            }),
            fetch: tokio::sync::Mutex::new(()),
        }
    }

    /// Fixed keys (tests).
    pub fn with_jwks(config: KeycloakConfig, jwks: JwkSet) -> Self {
        let v = Self::new(config);
        *v.keys.write().expect("keys") = Keys {
            set: Some(jwks),
            fetched: Some(Instant::now() + Duration::from_secs(10 * 365 * 86400)),
        };
        v
    }

    async fn refresh(&self, force: bool) -> Result<(), String> {
        let _guard = self.fetch.lock().await;
        {
            let keys = self.keys.read().expect("keys");
            if let Some(fetched) = keys.fetched {
                let age = Instant::now().saturating_duration_since(fetched);
                if (force && age < REFRESH_COOLDOWN) || (!force && age < MAX_KEY_AGE) {
                    return Ok(());
                }
            }
        }
        let jwks_url = match &self.config.jwks_url {
            Some(url) => url.clone(),
            None => {
                let discovery = format!(
                    "{}/.well-known/openid-configuration",
                    self.config.issuer.trim_end_matches('/')
                );
                let doc: serde_json::Value = self
                    .http
                    .get(&discovery)
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("OIDC discovery {discovery}: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("OIDC discovery {discovery}: {e}"))?;
                doc["jwks_uri"]
                    .as_str()
                    .ok_or_else(|| format!("{discovery} has no jwks_uri"))?
                    .to_owned()
            }
        };
        let set: JwkSet = self
            .http
            .get(&jwks_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("JWKS {jwks_url}: {e}"))?
            .json()
            .await
            .map_err(|e| format!("JWKS {jwks_url}: {e}"))?;
        *self.keys.write().expect("keys") = Keys {
            set: Some(set),
            fetched: Some(Instant::now()),
        };
        Ok(())
    }

    fn key_for(&self, kid: Option<&str>, alg: Algorithm) -> Option<DecodingKey> {
        let keys = self.keys.read().expect("keys");
        let set = keys.set.as_ref()?;
        set.keys
            .iter()
            .filter(|k| match (kid, k.common.key_id.as_deref()) {
                (Some(want), Some(have)) => want == have,
                (Some(_), None) => false,
                (None, _) => true,
            })
            .filter(|k| {
                matches!(
                    (&k.algorithm, alg),
                    (
                        AlgorithmParameters::RSA(_),
                        Algorithm::RS256
                            | Algorithm::RS384
                            | Algorithm::RS512
                            | Algorithm::PS256
                            | Algorithm::PS384
                            | Algorithm::PS512
                    ) | (
                        AlgorithmParameters::EllipticCurve(_),
                        Algorithm::ES256 | Algorithm::ES384
                    ) | (AlgorithmParameters::OctetKeyPair(_), Algorithm::EdDSA)
                )
            })
            .find_map(|k| DecodingKey::from_jwk(k).ok())
    }

    /// Validates an access token. The error is a short reason that is safe
    /// to show and log (it never contains the token).
    pub async fn validate(&self, token: &str) -> Result<User, String> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| "not a JWT".to_owned())?;
        if matches!(
            header.alg,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            return Err("symmetric JWTs are not accepted".into());
        }
        let _ = self.refresh(false).await;
        let key = match self.key_for(header.kid.as_deref(), header.alg) {
            Some(key) => key,
            None => {
                self.refresh(true).await?;
                self.key_for(header.kid.as_deref(), header.alg)
                    .ok_or_else(|| "unknown signing key".to_owned())?
            }
        };
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[
            self.config.issuer.trim_end_matches('/'),
            &self.config.issuer,
        ]);
        validation.set_audience(&self.config.audiences);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = 30;
        let data = jsonwebtoken::decode::<serde_json::Value>(token, &key, &validation)
            .map_err(|e| format!("access token: {e}"))?;
        let text = |name: &str| {
            data.claims
                .get(name)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let user = text("sub").ok_or_else(|| "access token has no sub".to_owned())?;
        let account = text(&self.config.account_claim).unwrap_or_else(|| user.clone());
        Ok(User { user, account })
    }
}
