// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Account-token validation: OIDC access tokens (JWT) from the user's
//! cua.ai login, checked against the issuer's JWKS, the configured
//! audience(s) and expiry.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};

use crate::auth_log::Reason;

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
    /// Keycloak Organizations the user is in (the `organization` claim),
    /// with the role their org group gives. Empty when the token has none.
    pub orgs: Vec<OrgClaim>,
}

/// One entry of the `organization` claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgClaim {
    /// The organization's alias (the claim's key).
    pub alias: String,
    /// The organization's id, when the mapper includes it.
    pub id: Option<String>,
    /// `owner`, `admin` or `member` (from the `/owner` and `/admin` org
    /// groups; no group is a member).
    pub role: &'static str,
}

impl OrgClaim {
    /// The id the Spaces API knows the organization by: its id, else its
    /// alias.
    pub fn key(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.alias)
    }
}

/// Reads the `organization` claim: an object of alias -> `{id, groups}`
/// (the Organization Group Membership mapper) or a plain list of aliases.
pub fn org_claims(claims: &serde_json::Value) -> Vec<OrgClaim> {
    let role = |groups: Option<&serde_json::Value>| {
        let groups: Vec<&str> = groups
            .and_then(|g| g.as_array())
            .map(|a| a.iter().filter_map(|g| g.as_str()).collect())
            .unwrap_or_default();
        let has = |name: &str| {
            groups
                .iter()
                .any(|g| g.trim_start_matches('/').eq_ignore_ascii_case(name))
        };
        if has("owner") {
            "owner"
        } else if has("admin") {
            "admin"
        } else {
            "member"
        }
    };
    let mut out = match claims.get("organization") {
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .filter(|(alias, _)| !alias.is_empty())
            .map(|(alias, v)| OrgClaim {
                alias: alias.clone(),
                id: v
                    .get("id")
                    .and_then(|i| i.as_str())
                    .filter(|i| !i.is_empty())
                    .map(str::to_owned),
                role: role(v.get("groups")),
            })
            .collect(),
        Some(serde_json::Value::Array(list)) => list
            .iter()
            .filter_map(|a| a.as_str())
            .filter(|a| !a.is_empty())
            .map(|alias| OrgClaim {
                alias: alias.to_owned(),
                id: None,
                role: "member",
            })
            .collect(),
        _ => Vec::new(),
    };
    out.truncate(64);
    out
}

/// Minimum time between two JWKS fetch attempts, whether the last one
/// worked or not. A failing issuer is therefore asked about twice a minute,
/// never once per request.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(30);
/// The background refresher renews the keys this often. Requests never wait
/// for it: they validate against the keys already held (CUA-2323).
const REFRESH_EVERY: Duration = Duration::from_secs(600);
/// How often the background refresher checks whether a renewal is due.
const REFRESH_TICK: Duration = Duration::from_secs(15);
/// Most a request waits for a JWKS fetch (only when there are no keys at all
/// or its key id is unknown); also the budget of one whole fetch.
const FETCH_BUDGET: Duration = Duration::from_secs(6);

struct Keys {
    set: Option<JwkSet>,
    fetched: Option<Instant>,
    /// Start of the last fetch attempt, successful or not.
    attempted: Option<Instant>,
}

/// Why the keys are being fetched.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Why {
    /// No keys held yet (first request after a start).
    Missing,
    /// A token names a key id that is not held.
    Unknown,
    /// The background renewal.
    Scheduled,
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
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(5))
                .build()
                .expect("http client"),
            keys: RwLock::new(Keys {
                set: None,
                fetched: None,
                attempted: None,
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
            attempted: None,
        };
        v
    }

    /// The issuer settings.
    pub fn config(&self) -> &OidcConfig {
        &self.config
    }

    /// Starts the background renewal of the signing keys. Requests only
    /// ever read the keys already held, so a slow or unreachable issuer
    /// costs them nothing (CUA-2323).
    pub fn spawn_refresher(self: &std::sync::Arc<Self>) {
        let me = std::sync::Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(v) = me.upgrade() else { return };
                let _ = v.refresh(Why::Scheduled).await;
                drop(v);
                tokio::time::sleep(REFRESH_TICK).await;
            }
        });
    }

    /// Whether a fetch for `why` is wanted now. `Err` when there is nothing
    /// to serve from and a recent attempt failed.
    fn wanted(&self, why: Why) -> Result<bool, String> {
        let keys = self.keys.read().expect("keys");
        let now = Instant::now();
        if let Some(at) = keys.attempted {
            if now.saturating_duration_since(at) < REFRESH_COOLDOWN {
                return match (why, keys.set.is_some()) {
                    (Why::Missing, false) => {
                        Err("signing keys unavailable (fetch failed recently)".into())
                    }
                    _ => Ok(false),
                };
            }
        }
        Ok(match why {
            Why::Missing => keys.set.is_none(),
            Why::Unknown => true,
            Why::Scheduled => keys
                .fetched
                .is_none_or(|f| now.saturating_duration_since(f) >= REFRESH_EVERY),
        })
    }

    async fn refresh(&self, why: Why) -> Result<(), String> {
        if !self.wanted(why)? {
            return Ok(());
        }
        let _guard = tokio::time::timeout(FETCH_BUDGET, self.fetch.lock())
            .await
            .map_err(|_| "signing keys: a refresh is already running".to_owned())?;
        // Another caller may have fetched while this one waited.
        if !self.wanted(why)? {
            return Ok(());
        }
        self.keys.write().expect("keys").attempted = Some(Instant::now());
        let fetched = tokio::time::timeout(FETCH_BUDGET, self.fetch_keys())
            .await
            .unwrap_or_else(|_| Err(format!("timed out after {}s", FETCH_BUDGET.as_secs())));
        match fetched {
            Ok(set) => {
                let mut keys = self.keys.write().expect("keys");
                keys.set = Some(set);
                keys.fetched = Some(Instant::now());
                Ok(())
            }
            Err(e) => {
                tracing::warn!(
                    issuer = %self.config.issuer,
                    error = %e,
                    "signing keys not refreshed; keeping the keys held"
                );
                Err(e)
            }
        }
    }

    async fn fetch_keys(&self) -> Result<JwkSet, String> {
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
        self.http
            .get(&jwks_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("JWKS {jwks_url}: {e}"))?
            .json()
            .await
            .map_err(|e| format!("JWKS {jwks_url}: {e}"))
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
    pub async fn validate(&self, token: &str) -> Result<Identity, TokenError> {
        self.validate_inner(token)
            .await
            .map_err(|(reason, message)| {
                let facts = crate::auth_log::TokenFacts::from_token(
                    token,
                    reason,
                    &self.config.issuer,
                    &self.config.audiences,
                );
                TokenError {
                    reason,
                    message,
                    facts,
                }
            })
    }

    async fn validate_inner(&self, token: &str) -> Result<Identity, (Reason, String)> {
        let header = jsonwebtoken::decode_header(token)
            .map_err(|_| (Reason::Malformed, "not a JWT".to_owned()))?;
        if matches!(
            header.alg,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            return Err((
                Reason::UnsupportedAlgorithm,
                "symmetric JWTs are not accepted".into(),
            ));
        }
        // No fetch on the request path while keys are held: the background
        // refresher renews them. Only a first request (no keys) or an
        // unknown key id waits, for a bounded time and at most once per
        // cooldown.
        let _ = self.refresh(Why::Missing).await;
        let key = match self.key_for(header.kid.as_deref(), header.alg) {
            Some(key) => key,
            None => {
                self.refresh(Why::Unknown)
                    .await
                    .map_err(|e| (Reason::JwksUnavailable, e))?;
                self.key_for(header.kid.as_deref(), header.alg)
                    .ok_or((Reason::UnknownSigningKey, "unknown signing key".to_owned()))?
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
            .map_err(|e| (jwt_reason(e.kind()), format!("account token: {e}")))?;
        let claims = data.claims;
        let text = |name: &str| {
            claims
                .get(name)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let user =
            text("sub").ok_or((Reason::MissingClaim, "account token has no sub".to_owned()))?;
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
            orgs: org_claims(&claims),
        })
    }
}

/// A refused account token: the message (shown to the caller, as before)
/// and the facts the relay logs about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenError {
    /// Why, as logged.
    pub reason: Reason,
    /// The caller-facing message.
    pub message: String,
    /// Safe-to-log token details.
    pub facts: crate::auth_log::TokenFacts,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TokenError {}

fn jwt_reason(kind: &jsonwebtoken::errors::ErrorKind) -> Reason {
    use jsonwebtoken::errors::ErrorKind as K;
    match kind {
        K::InvalidSignature => Reason::BadSignature,
        K::ExpiredSignature => Reason::Expired,
        K::ImmatureSignature => Reason::NotYetValid,
        K::InvalidIssuer => Reason::WrongIssuer,
        K::InvalidAudience => Reason::WrongAudience,
        K::MissingRequiredClaim(_) => Reason::MissingClaim,
        K::InvalidAlgorithm | K::MissingAlgorithm => Reason::UnsupportedAlgorithm,
        _ => Reason::Malformed,
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
        /// `organization` claims per subject (none by default).
        orgs: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
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
                orgs: std::sync::Mutex::default(),
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

        /// Tokens for `sub` carry this `organization` claim from now on
        /// (Keycloak Organizations, e.g. `{"acme": {"id": "…", "groups":
        /// ["/owner"]}}`).
        pub fn set_orgs(&self, sub: &str, organization: serde_json::Value) {
            self.orgs
                .lock()
                .expect("orgs")
                .insert(sub.to_owned(), organization);
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
            if let Some(orgs) = self.orgs.lock().expect("orgs").get(sub).cloned() {
                claims["organization"] = orgs;
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

    #[test]
    fn reads_the_organization_claim() {
        let claims = serde_json::json!({"organization": {
            "aigis": {"id": "o1", "groups": ["/owner"]},
            "acme": {"id": "o2", "groups": ["/admin"]},
            "solo": {},
        }});
        let orgs = org_claims(&claims);
        let find = |a: &str| orgs.iter().find(|o| o.alias == a).cloned().unwrap();
        assert_eq!(find("aigis").role, "owner");
        assert_eq!(find("aigis").key(), "o1");
        assert_eq!(find("acme").role, "admin");
        assert_eq!(find("solo").role, "member");
        assert_eq!(find("solo").key(), "solo");
        let list = org_claims(&serde_json::json!({"organization": ["aigis"]}));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].role, "member");
        assert!(org_claims(&serde_json::json!({})).is_empty());
    }

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

    /// CUA-2323: with keys held, validating a token never touches the
    /// network, even when the issuer's key endpoint hangs.
    #[tokio::test]
    async fn held_keys_validate_without_asking_the_issuer() {
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let issuer = FakeIssuer::new("https://auth.example/realms/cua");
        let mut config = OidcConfig::new(issuer.issuer.clone());
        config.jwks_url = Some(format!("http://{}/certs", silent.local_addr().unwrap()));
        let v = OidcValidator::with_jwks(config, issuer.jwks());
        let started = Instant::now();
        for _ in 0..20 {
            v.validate(&issuer.token("u1", None, "cua-relay", 300))
                .await
                .unwrap();
        }
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(v.keys.read().unwrap().attempted.is_none());
    }

    /// CUA-2323: an issuer that cannot be reached is asked about once per
    /// cooldown, not once per request.
    #[tokio::test]
    async fn a_failed_key_fetch_is_not_repeated_on_every_request() {
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let issuer = FakeIssuer::new("https://auth.example/realms/cua");
        let mut config = OidcConfig::new(issuer.issuer.clone());
        config.jwks_url = Some(format!("http://{closed}/certs"));
        let v = OidcValidator::new(config);
        let token = issuer.token("u1", None, "cua-relay", 300);
        assert!(v.validate(&token).await.is_err());
        let first = v.keys.read().unwrap().attempted.expect("attempted");
        let started = Instant::now();
        for _ in 0..20 {
            assert!(v.validate(&token).await.is_err());
        }
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(v.keys.read().unwrap().attempted, Some(first));
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
