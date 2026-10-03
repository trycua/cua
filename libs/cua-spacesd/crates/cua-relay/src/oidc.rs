// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Account-token validation: OIDC access tokens (JWT) from the user's
//! cua.ai login, checked against the issuer's JWKS, the configured
//! audience(s) and expiry.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};

/// Default audience relay account tokens must carry.
pub const DEFAULT_AUDIENCE: &str = "cua-relay";

/// `amr` (Authentication Methods References, RFC 8176) values that count as
/// a second factor when no realm-specific list is configured.
pub const DEFAULT_MFA_AMR_VALUES: &[&str] = &["otp", "totp", "hwk", "webauthn", "u2f", "mfa", "sc"];

/// Issuer settings.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Expected `iss` (also the discovery base).
    pub issuer: String,
    /// JWKS URL; discovered from `<issuer>/.well-known/openid-configuration`
    /// when unset.
    pub jwks_url: Option<String>,
    /// Accepted audiences (any of them).
    pub audiences: Vec<String>,
    /// Claim naming the account (default `sub`).
    pub account_claim: String,
    /// `acr` values that count as multi-factor authentication, per the
    /// issuer's own ACR-to-LoA mapping (Keycloak: Realm settings > Login >
    /// "ACR to Level of Authentication (LoA) Mapping", with a step-up
    /// authentication flow requiring OTP/WebAuthn at that level). Empty
    /// (the default) disables the `acr` check: configure this to whatever
    /// your realm's mapping actually emits, or rely on `amr` instead.
    pub mfa_acr_values: Vec<String>,
    /// `amr` values that count as multi-factor (RFC 8176), checked in
    /// addition to `mfa_acr_values`. Defaults to [`DEFAULT_MFA_AMR_VALUES`].
    pub mfa_amr_values: Vec<String>,
}

impl OidcConfig {
    /// Settings for `issuer` with the default audience and account claim.
    pub fn new(issuer: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            jwks_url: None,
            audiences: vec![DEFAULT_AUDIENCE.into()],
            account_claim: "sub".into(),
            mfa_acr_values: Vec::new(),
            mfa_amr_values: DEFAULT_MFA_AMR_VALUES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// An authenticated account user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// User id (`sub`).
    pub user: String,
    /// Account id (the configured account claim; `sub` by default).
    pub account: String,
    /// Email, only when the issuer marked it verified (`email_verified`):
    /// allowlists match on it, so an unverified address is never used.
    pub email: Option<String>,
    /// Display name (`name` or `preferred_username`), if present.
    pub name: Option<String>,
    /// When the user last signed in interactively (`auth_time`), if the
    /// issuer says. Refreshing a session keeps it, so it tells a fresh
    /// sign-in from a long-lived session (device bootstrap).
    pub auth_time: Option<u64>,
    /// Whether this token's authentication context proves a second factor
    /// (`acr` in [`OidcConfig::mfa_acr_values`], or `amr` intersecting
    /// [`OidcConfig::mfa_amr_values`]). Used to enroll a brand-new device on
    /// an account that already has a strong enrolled device (S4) without
    /// needing an approval.
    pub mfa: bool,
}

/// Minimum time between JWKS refreshes triggered by unknown key ids.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(30);
/// Keys are refreshed at least this often.
const MAX_KEY_AGE: Duration = Duration::from_secs(3600);

struct Keys {
    set: Option<JwkSet>,
    fetched: Option<Instant>,
}

/// Validates account tokens.
pub struct OidcValidator {
    config: OidcConfig,
    http: reqwest::Client,
    keys: RwLock<Keys>,
    fetch: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for OidcValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcValidator")
            .field("issuer", &self.config.issuer)
            .finish()
    }
}

impl OidcValidator {
    /// A validator that fetches keys from the issuer.
    pub fn new(config: OidcConfig) -> Self {
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

    /// A validator with fixed keys (tests, air-gapped relays).
    pub fn with_jwks(config: OidcConfig, jwks: JwkSet) -> Self {
        let v = Self::new(config);
        *v.keys.write().expect("keys") = Keys {
            set: Some(jwks),
            fetched: Some(Instant::now() + Duration::from_secs(10 * 365 * 86400)),
        };
        v
    }

    /// The issuer settings.
    pub fn config(&self) -> &OidcConfig {
        &self.config
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

    /// Validates an account token.
    pub async fn validate(&self, token: &str) -> Result<Identity, String> {
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
                    .ok_or("unknown signing key")?
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
            .map_err(|e| format!("account token: {e}"))?;
        let claims = data.claims;
        let text = |name: &str| {
            claims
                .get(name)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let user = text("sub").ok_or("account token has no sub")?;
        let account = text(&self.config.account_claim).unwrap_or_else(|| user.clone());
        // Sharing by email grants access, so only an address the issuer
        // verified counts (some issuers encode the flag as a string).
        let email_verified = match claims.get("email_verified") {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("true"),
            _ => false,
        };
        let acr = text("acr");
        let amr: Vec<String> = claims
            .get("amr")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let mfa = acr.is_some_and(|acr| self.config.mfa_acr_values.iter().any(|v| v == &acr))
            || amr.iter().any(|m| {
                self.config
                    .mfa_amr_values
                    .iter()
                    .any(|v| v.eq_ignore_ascii_case(m))
            });
        Ok(Identity {
            user,
            account,
            email: text("email")
                .filter(|_| email_verified)
                .map(|e| e.to_ascii_lowercase()),
            name: text("name").or_else(|| text("preferred_username")),
            auth_time: claims.get("auth_time").and_then(|v| v.as_u64()),
            mfa,
        })
    }
}

/// A fake OIDC issuer for tests: ES256 keys generated in-process.
pub mod testing {
    use super::*;
    use base64::Engine as _;
    use ring::signature::KeyPair as _;

    /// Signs account tokens and publishes the matching JWKS.
    pub struct FakeIssuer {
        /// Issuer URL (`iss`).
        pub issuer: String,
        /// `auth_time` claims per subject (none by default).
        auth_times: std::sync::Mutex<std::collections::HashMap<String, i64>>,
        /// `amr` claims per subject (none by default): set to prove MFA.
        amrs: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
        /// `acr` claims per subject (none by default).
        acrs: std::sync::Mutex<std::collections::HashMap<String, String>>,
        kid: String,
        encoding: jsonwebtoken::EncodingKey,
        public: Vec<u8>,
    }

    impl FakeIssuer {
        /// A new issuer called `issuer`.
        pub fn new(issuer: impl Into<String>) -> Self {
            let rng = ring::rand::SystemRandom::new();
            let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
            let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).expect("p256");
            let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng)
                .expect("p256 parse");
            Self {
                issuer: issuer.into(),
                auth_times: std::sync::Mutex::default(),
                amrs: std::sync::Mutex::default(),
                acrs: std::sync::Mutex::default(),
                kid: format!("fake-{}", uuid::Uuid::new_v4().simple()),
                encoding: jsonwebtoken::EncodingKey::from_ec_der(pkcs8.as_ref()),
                public: pair.public_key().as_ref().to_vec(),
            }
        }

        /// Tokens for `sub` carry `auth_time` = `at` (Unix seconds) from now
        /// on: a fresh interactive sign-in when `at` is recent.
        pub fn set_auth_time(&self, sub: &str, at: i64) {
            self.auth_times
                .lock()
                .expect("auth times")
                .insert(sub.to_owned(), at);
        }

        fn auth_time(&self, sub: &str) -> Option<i64> {
            self.auth_times
                .lock()
                .expect("auth times")
                .get(sub)
                .copied()
        }

        /// Tokens for `sub` carry `amr` = `methods` from now on (e.g.
        /// `&["otp"]`), the way a step-up / conditional-OTP sign-in does.
        pub fn set_amr(&self, sub: &str, methods: &[&str]) {
            self.amrs.lock().expect("amrs").insert(
                sub.to_owned(),
                methods.iter().map(|m| m.to_string()).collect(),
            );
        }

        fn amr(&self, sub: &str) -> Option<Vec<String>> {
            self.amrs.lock().expect("amrs").get(sub).cloned()
        }

        /// Tokens for `sub` carry `acr` = `value` from now on, the way a
        /// step-up authentication flow (Keycloak ACR-to-LoA mapping) does.
        pub fn set_acr(&self, sub: &str, value: &str) {
            self.acrs
                .lock()
                .expect("acrs")
                .insert(sub.to_owned(), value.to_owned());
        }

        fn acr(&self, sub: &str) -> Option<String> {
            self.acrs.lock().expect("acrs").get(sub).cloned()
        }

        /// The issuer's JWKS as JSON.
        pub fn jwks_json(&self) -> serde_json::Value {
            let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            // Uncompressed point: 0x04 || x || y.
            serde_json::json!({"keys": [{
                "kty": "EC", "crv": "P-256", "kid": self.kid, "alg": "ES256", "use": "sig",
                "x": b64.encode(&self.public[1..33]), "y": b64.encode(&self.public[33..65]),
            }]})
        }

        /// The issuer's JWKS.
        pub fn jwks(&self) -> JwkSet {
            serde_json::from_value(self.jwks_json()).expect("jwks")
        }

        /// An access token for `sub` (audience `aud`, valid `ttl` seconds)
        /// with a verified `email`.
        pub fn token(&self, sub: &str, email: Option<&str>, aud: &str, ttl: i64) -> String {
            self.token_with(sub, email, true, aud, ttl)
        }

        /// Like [`Self::token`], choosing whether the email is verified.
        pub fn token_with(
            &self,
            sub: &str,
            email: Option<&str>,
            email_verified: bool,
            aud: &str,
            ttl: i64,
        ) -> String {
            let now = super::super::assertion::now_secs() as i64;
            let mut claims = serde_json::json!({
                "iss": self.issuer, "sub": sub, "aud": aud,
                "iat": now, "exp": now + ttl, "name": format!("User {sub}"),
            });
            if let Some(auth_time) = self.auth_time(sub) {
                claims["auth_time"] = auth_time.into();
            }
            if let Some(amr) = self.amr(sub) {
                claims["amr"] = amr.into();
            }
            if let Some(acr) = self.acr(sub) {
                claims["acr"] = acr.into();
            }
            if let Some(email) = email {
                claims["email"] = email.into();
                claims["email_verified"] = email_verified.into();
            }
            let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
            header.kid = Some(self.kid.clone());
            jsonwebtoken::encode(&header, &claims, &self.encoding).expect("sign")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeIssuer;
    use super::*;

    #[tokio::test]
    async fn validates_issuer_audience_and_expiry() {
        let issuer = FakeIssuer::new("https://auth.example/realms/cua");
        let v = OidcValidator::with_jwks(OidcConfig::new(issuer.issuer.clone()), issuer.jwks());
        let id = v
            .validate(&issuer.token("u1", Some("Ada@Example.com"), "cua-relay", 300))
            .await
            .unwrap();
        assert_eq!(id.user, "u1");
        assert_eq!(id.account, "u1");
        assert_eq!(id.email.as_deref(), Some("ada@example.com"));
        // Wrong audience, expired, other issuer's key, garbage.
        assert!(v
            .validate(&issuer.token("u1", None, "account", 300))
            .await
            .is_err());
        assert!(v
            .validate(&issuer.token("u1", None, "cua-relay", -120))
            .await
            .is_err());
        let other = FakeIssuer::new(issuer.issuer.clone());
        assert!(v
            .validate(&other.token("u1", None, "cua-relay", 300))
            .await
            .is_err());
        assert!(v.validate("nope").await.is_err());
        // An unverified email is dropped (allowlists must not match on it).
        let unverified = v
            .validate(&issuer.token_with("u2", Some("victim@example.com"), false, "cua-relay", 300))
            .await
            .unwrap();
        assert_eq!(unverified.email, None);
        let mut wrong_iss = OidcConfig::new("https://elsewhere");
        wrong_iss.audiences = vec!["cua-relay".into()];
        let v2 = OidcValidator::with_jwks(wrong_iss, issuer.jwks());
        assert!(v2
            .validate(&issuer.token("u1", None, "cua-relay", 300))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn mfa_is_read_from_amr_and_acr() {
        let issuer = FakeIssuer::new("https://auth.example/realms/cua");
        let v = OidcValidator::with_jwks(OidcConfig::new(issuer.issuer.clone()), issuer.jwks());
        // No amr/acr at all: not MFA.
        let id = v
            .validate(&issuer.token("u1", None, "cua-relay", 300))
            .await
            .unwrap();
        assert!(!id.mfa);
        // A default amr value (e.g. Keycloak's conditional OTP) counts.
        issuer.set_amr("u1", &["otp"]);
        let id = v
            .validate(&issuer.token("u1", None, "cua-relay", 300))
            .await
            .unwrap();
        assert!(id.mfa);
        // Password-only amr does not.
        issuer.set_amr("u1", &["pwd"]);
        let id = v
            .validate(&issuer.token("u1", None, "cua-relay", 300))
            .await
            .unwrap();
        assert!(!id.mfa);
        // A configured acr value counts too, even with no amr.
        issuer.set_amr("u1", &[]);
        let mut config = OidcConfig::new(issuer.issuer.clone());
        config.mfa_acr_values = vec!["gold".into()];
        let v2 = OidcValidator::with_jwks(config.clone(), issuer.jwks());
        let id = v2
            .validate(&issuer.token("u1", None, "cua-relay", 300))
            .await
            .unwrap();
        assert!(!id.mfa, "acr claim absent from the token");
        issuer.set_acr("u1", "gold");
        let id = v2
            .validate(&issuer.token("u1", None, "cua-relay", 300))
            .await
            .unwrap();
        assert!(id.mfa);
        // An acr value not in the configured list does not count.
        issuer.set_acr("u1", "silver");
        let id = v2
            .validate(&issuer.token("u1", None, "cua-relay", 300))
            .await
            .unwrap();
        assert!(!id.mfa);
    }
}
