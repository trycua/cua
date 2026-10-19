// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One structured log line per 401 the relay answers, so a failed sign-in
//! or host setup can be traced to its cause from the relay's logs alone.
//!
//! Every line goes to target [`TARGET`] at `info` with the fields:
//!
//! - `reason`: a [`Reason`] category (`expired`, `wrong_audience`, ...);
//! - `route`: the endpoint class ([`Route`]);
//! - `client`: the client kind, bucketed from the credential and the
//!   `User-Agent` (`spacesd`, `machine`, `cli`, `app`, `sdk`, `browser`,
//!   `other`); the raw header is never logged;
//! - `token_issuer` / `token_audience`: only values the relay knows to be
//!   public (its configured issuer and audiences, `*.cua.ai` issuers, a few
//!   well-known audiences); anything else is logged as `other`;
//! - `clock_skew_secs`: for time-based failures, how far off the token or
//!   proof was: seconds past `exp` (`expired`), seconds until `nbf`
//!   (`not_yet_valid`), or the proof's timestamp minus the relay's clock
//!   (`device_proof_stale`; positive when the device clock is ahead).
//!
//! Tokens, token fragments, emails, account and user ids, IP addresses,
//! machine ids and device names are never logged.

use axum::http::{header, HeaderMap};

/// The `tracing` target of the 401 log lines.
pub const TARGET: &str = "cua_relay::auth";

/// Why a request was refused with 401.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// No credential where one is required.
    MissingToken,
    /// Not a JWT, undecodable claims or an unusable key.
    Malformed,
    /// A symmetric (HS*) or otherwise unsupported signing algorithm.
    UnsupportedAlgorithm,
    /// Signed by a key the issuer does not publish (another issuer or
    /// environment, or a rotated key).
    UnknownSigningKey,
    /// The issuer's JWKS could not be fetched.
    JwksUnavailable,
    /// The signature does not verify.
    BadSignature,
    /// `iss` is not the configured issuer.
    WrongIssuer,
    /// `aud` holds none of the configured audiences.
    WrongAudience,
    /// `exp` has passed (beyond the leeway).
    Expired,
    /// `nbf` is in the future.
    NotYetValid,
    /// A required claim (`exp`, `iss`, `aud`, `sub`) is missing.
    MissingClaim,
    /// A machine token the directory does not know (revoked, rotated,
    /// unregistered).
    UnknownMachineToken,
    /// A static relay token that is not configured.
    InvalidRelayToken,
    /// Wrong admin token.
    InvalidAdminToken,
    /// A device proof outside the allowed clock window.
    DeviceProofStale,
    /// A device proof whose signature does not verify.
    DeviceProofBadSignature,
    /// A device proof that was already used.
    DeviceProofReplayed,
    /// A request to a static-token machine without any spacesd credential
    /// (`--require-client-credentials`).
    SpacesdCredentialMissing,
}

impl Reason {
    /// The category as logged.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingToken => "missing_token",
            Self::Malformed => "malformed",
            Self::UnsupportedAlgorithm => "unsupported_algorithm",
            Self::UnknownSigningKey => "unknown_signing_key",
            Self::JwksUnavailable => "jwks_unavailable",
            Self::BadSignature => "bad_signature",
            Self::WrongIssuer => "wrong_issuer",
            Self::WrongAudience => "wrong_audience",
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::MissingClaim => "missing_claim",
            Self::UnknownMachineToken => "unknown_machine_token",
            Self::InvalidRelayToken => "invalid_relay_token",
            Self::InvalidAdminToken => "invalid_admin_token",
            Self::DeviceProofStale => "device_proof_stale",
            Self::DeviceProofBadSignature => "device_proof_bad_signature",
            Self::DeviceProofReplayed => "device_proof_replayed",
            Self::SpacesdCredentialMissing => "spacesd_credential_missing",
        }
    }
}

/// The endpoint class a 401 came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `/relay/v1/connect` (a machine joining).
    Connect,
    /// `/relay/v1/machines` (admin).
    Admin,
    /// `/v1/machines…` (directory API).
    Machines,
    /// `/v1/devices…` (device enrollment).
    Devices,
    /// A client request forwarded to a machine.
    Proxy,
}

impl Route {
    /// The class as logged.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Admin => "admin",
            Self::Machines => "machines",
            Self::Devices => "devices",
            Self::Proxy => "proxy",
        }
    }
}

/// Audiences that are public names, logged as they are.
const KNOWN_AUDIENCES: &[&str] = &[
    crate::oidc::DEFAULT_AUDIENCE,
    "account",
    "cua",
    "cua-cli",
    "cua-app",
    "cua-spaces",
    "cua-cloud",
];

/// Token details safe to log, kept on a failed account-token validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenFacts {
    /// `iss`, if public (else `other`).
    pub issuer: Option<String>,
    /// `aud` values, if public (each unknown one as `other`), comma-joined.
    pub audience: Option<String>,
    /// Seconds past `exp` (expired) or until `nbf` (not yet valid).
    pub clock_skew_secs: Option<i64>,
}

impl TokenFacts {
    /// Reads `iss`, `aud`, `exp` and `nbf` from an (unverified) JWT,
    /// keeping only what is safe to log. `issuer` and `audiences` are the
    /// relay's own configuration.
    pub fn from_token(token: &str, reason: Reason, issuer: &str, audiences: &[String]) -> Self {
        let Some(claims) = unverified_claims(token) else {
            return Self::default();
        };
        let now = crate::assertion::now_secs() as i64;
        let clock_skew_secs = match reason {
            Reason::Expired => claims.get("exp").and_then(|v| v.as_i64()).map(|e| now - e),
            Reason::NotYetValid => claims.get("nbf").and_then(|v| v.as_i64()).map(|n| n - now),
            _ => None,
        };
        let iss = claims
            .get("iss")
            .and_then(|v| v.as_str())
            .map(|s| public_issuer(s, issuer).to_owned());
        let aud = match claims.get("aud") {
            Some(serde_json::Value::String(a)) => Some(vec![a.as_str()]),
            Some(serde_json::Value::Array(a)) => {
                Some(a.iter().filter_map(|v| v.as_str()).take(8).collect())
            }
            _ => None,
        }
        .map(|values| {
            values
                .into_iter()
                .map(|a| public_audience(a, audiences))
                .collect::<Vec<_>>()
                .join(",")
        });
        Self {
            issuer: iss,
            audience: aud,
            clock_skew_secs,
        }
    }
}

fn unverified_claims(token: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    match serde_json::from_slice(&raw).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

/// `iss` as logged: the configured issuer or a `cua.ai` URL, else `other`.
fn public_issuer<'a>(iss: &'a str, configured: &str) -> &'a str {
    if iss.trim_end_matches('/') == configured.trim_end_matches('/') {
        return iss;
    }
    let host = url::Url::parse(iss)
        .ok()
        .filter(|u| u.scheme() == "https" && u.username().is_empty() && u.password().is_none())
        .and_then(|u| u.host_str().map(str::to_owned));
    match host {
        Some(h) if (h == "cua.ai" || h.ends_with(".cua.ai")) && iss.len() <= 128 => iss,
        _ => "other",
    }
}

/// An `aud` value as logged: configured or well known, else `other`.
fn public_audience<'a>(aud: &'a str, configured: &[String]) -> &'a str {
    if configured.iter().any(|c| c == aud) || KNOWN_AUDIENCES.contains(&aud) {
        aud
    } else {
        "other"
    }
}

/// The client kind: from the credential when it says (machine token,
/// spacesd join), else a bucket of the `User-Agent`.
pub fn client_kind(headers: &HeaderMap) -> &'static str {
    let machine_token = crate::server::bearer(headers, header::AUTHORIZATION.as_str())
        .is_some_and(|t| t.starts_with(crate::directory::MACHINE_TOKEN_PREFIX));
    if machine_token {
        return "machine";
    }
    if headers.contains_key(crate::VERSION_HEADER) || headers.contains_key(crate::MACHINE_ID_HEADER)
    {
        return "spacesd";
    }
    let Some(ua) = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
    else {
        // The SDK's relay client (host setup, `cua` and the app's core)
        // sends no User-Agent.
        return "sdk";
    };
    let lower = ua.to_ascii_lowercase();
    if lower.starts_with("cua-cli/") || lower.starts_with("cua-host/") || lower.starts_with("cua/")
    {
        "cli"
    } else if lower.contains("spacesd") {
        "spacesd"
    } else if lower.contains("cfnetwork")
        || lower.contains("cua spaces")
        || lower.contains("cua%20")
    {
        "app"
    } else if lower.starts_with("mozilla/") {
        "browser"
    } else {
        "other"
    }
}

/// Logs one 401.
pub fn rejected(headers: &HeaderMap, route: Route, reason: Reason, facts: Option<&TokenFacts>) {
    let facts = facts.cloned().unwrap_or_default();
    tracing::info!(
        target: TARGET,
        reason = reason.as_str(),
        route = route.as_str(),
        client = client_kind(headers),
        token_issuer = facts.issuer.as_deref(),
        token_audience = facts.audience.as_deref(),
        clock_skew_secs = facts.clock_skew_secs,
        "401 unauthenticated"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn jwt(claims: serde_json::Value) -> String {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!(
            "{}.{}.sig",
            b64.encode(br#"{"alg":"ES256"}"#),
            b64.encode(claims.to_string())
        )
    }

    #[test]
    fn facts_keep_only_public_issuer_and_audience() {
        let audiences = vec!["cua-relay".to_owned()];
        let issuer = "https://auth.cua.ai/realms/cua";
        let facts = TokenFacts::from_token(
            &jwt(serde_json::json!({
                "iss": "https://auth.cua.ai/realms/cua/", "aud": ["account", "secret-client-123"],
                "sub": "u1", "email": "ada@example.com", "exp": 1,
            })),
            Reason::WrongAudience,
            issuer,
            &audiences,
        );
        assert_eq!(
            facts.issuer.as_deref(),
            Some("https://auth.cua.ai/realms/cua/")
        );
        assert_eq!(facts.audience.as_deref(), Some("account,other"));
        assert_eq!(facts.clock_skew_secs, None, "only time failures get skew");

        let foreign = TokenFacts::from_token(
            &jwt(serde_json::json!({"iss": "https://evil.example/private-tenant-42", "aud": "x"})),
            Reason::WrongIssuer,
            issuer,
            &audiences,
        );
        assert_eq!(foreign.issuer.as_deref(), Some("other"));
        assert_eq!(foreign.audience.as_deref(), Some("other"));
        // A cua.ai look-alike with credentials in the URL is not public.
        assert_eq!(
            public_issuer("https://user:pw@auth.cua.ai/", issuer),
            "other"
        );
        assert_eq!(
            public_issuer("https://auth.cua.ai.evil.example/", issuer),
            "other"
        );
    }

    #[test]
    fn skew_is_measured_for_time_failures() {
        let now = crate::assertion::now_secs() as i64;
        let expired = TokenFacts::from_token(
            &jwt(serde_json::json!({"exp": now - 600})),
            Reason::Expired,
            "https://i",
            &[],
        );
        let skew = expired.clock_skew_secs.unwrap();
        assert!((599..=605).contains(&skew), "{skew}");
        let early = TokenFacts::from_token(
            &jwt(serde_json::json!({"nbf": now + 120})),
            Reason::NotYetValid,
            "https://i",
            &[],
        );
        let skew = early.clock_skew_secs.unwrap();
        assert!((115..=120).contains(&skew), "{skew}");
        assert_eq!(
            TokenFacts::from_token("garbage", Reason::Malformed, "https://i", &[]),
            TokenFacts::default()
        );
    }

    #[test]
    fn client_kinds() {
        let with = |pairs: &[(&'static str, &str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(*k, HeaderValue::from_str(v).unwrap());
            }
            client_kind(&h)
        };
        assert_eq!(with(&[]), "sdk");
        assert_eq!(with(&[("authorization", "Bearer cmt_abc")]), "machine");
        assert_eq!(with(&[(crate::VERSION_HEADER, "0.2.2")]), "spacesd");
        assert_eq!(with(&[("user-agent", "cua-cli/0.9.0")]), "cli");
        assert_eq!(
            with(&[("user-agent", "Cua%20Spaces/5 CFNetwork/1568 Darwin/25.5.0")]),
            "app"
        );
        assert_eq!(
            with(&[("user-agent", "Mozilla/5.0 (Macintosh)")]),
            "browser"
        );
        assert_eq!(with(&[("user-agent", "curl/8.7.1")]), "other");
    }
}
