// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Apple Push Notification service provider (token-based, HTTP/2).
//!
//! It is complete and idle: it needs an APNs authentication key (a `.p8`
//! file from the Apple Developer account) that only the owner can create.
//! Nothing here stores a key; the relay reads the path from its
//! environment ([`ApnsConfig::from_env`]) and holds the key in memory only.
//!
//! - `CUA_PUSH_APNS_KEY_FILE`: path of `AuthKey_<id>.p8`.
//! - `CUA_PUSH_APNS_KEY_ID`: the key's 10 character id.
//! - `CUA_PUSH_APNS_TEAM_ID`: the team's 10 character id.
//! - `CUA_PUSH_APNS_TOPIC`: the app's bundle id (default
//!   `ai.cua.spaces.mobile`).
//! - `CUA_PUSH_APNS_ENDPOINT`: for tests, a server to use instead of
//!   Apple's (plain HTTP/2).
//!
//! A registration made by a development build (`sandbox`) goes to Apple's
//! sandbox endpoint, the rest to production.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};

use crate::event::Payload;
use crate::provider::{ProviderError, PushProvider};
use crate::registry::{Platform, Registration};

pub const PRODUCTION: &str = "https://api.push.apple.com";
pub const SANDBOX: &str = "https://api.sandbox.push.apple.com";
pub const DEFAULT_TOPIC: &str = "ai.cua.spaces.mobile";

/// Apple wants a provider token refreshed between every 20 and 60 minutes.
const TOKEN_LIFE: Duration = Duration::from_secs(40 * 60);
/// An alert not delivered within this long is dropped by Apple.
const EXPIRES_IN_SECS: u64 = 60 * 60;

/// What the provider needs.
#[derive(Clone)]
pub struct ApnsConfig {
    /// The `.p8` file's text (PKCS #8 PEM, ES256).
    pub key_pem: String,
    pub key_id: String,
    pub team_id: String,
    pub topic: String,
    /// Use this base URL for every phone, over plain HTTP/2 (tests).
    pub endpoint_override: Option<String>,
}

impl std::fmt::Debug for ApnsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApnsConfig")
            .field("key_id", &self.key_id)
            .field("team_id", &self.team_id)
            .field("topic", &self.topic)
            .field("key_pem", &"<not shown>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApnsError {
    #[error("{0}")]
    Config(String),
}

impl ApnsConfig {
    /// The configuration in the environment: `Ok(None)` when no key file is
    /// named (push stays off), an error when it is named but incomplete or
    /// unreadable (a half-set-up relay should say so, not stay silent).
    pub fn from_env() -> Result<Option<ApnsConfig>, ApnsError> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// [`ApnsConfig::from_env`] over any source of variables.
    pub fn from_vars(
        var: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<ApnsConfig>, ApnsError> {
        let get = |k: &str| {
            var(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(path) = get("CUA_PUSH_APNS_KEY_FILE") else {
            return Ok(None);
        };
        let need = |k: &str| {
            get(k).ok_or_else(|| {
                ApnsError::Config(format!("{k} is not set (CUA_PUSH_APNS_KEY_FILE is)"))
            })
        };
        let key_id = need("CUA_PUSH_APNS_KEY_ID")?;
        let team_id = need("CUA_PUSH_APNS_TEAM_ID")?;
        check_key_file_mode(&path)?;
        let key_pem = std::fs::read_to_string(&path)
            .map_err(|e| ApnsError::Config(format!("CUA_PUSH_APNS_KEY_FILE {path}: {e}")))?;
        Ok(Some(ApnsConfig {
            key_pem,
            key_id,
            team_id,
            topic: get("CUA_PUSH_APNS_TOPIC").unwrap_or_else(|| DEFAULT_TOPIC.into()),
            endpoint_override: get("CUA_PUSH_APNS_ENDPOINT"),
        }))
    }
}

/// The key file must be private to the relay's user (mode `0600`, or `0400`):
/// a signing key that other users can read is a key anyone on the machine can
/// send alerts with. The relay refuses to start with one rather than run
/// half safe. The key itself is never logged.
#[cfg(unix)]
fn check_key_file_mode(path: &str) -> Result<(), ApnsError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(path)
        .map_err(|e| ApnsError::Config(format!("CUA_PUSH_APNS_KEY_FILE {path}: {e}")))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(ApnsError::Config(format!(
            "CUA_PUSH_APNS_KEY_FILE {path} is readable by others (mode {mode:03o}); make it 0600 (chmod 600)"
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_key_file_mode(_path: &str) -> Result<(), ApnsError> {
    Ok(())
}

/// The DER inside a PEM `PRIVATE KEY` block.
fn pem_der(pem: &str) -> Result<Vec<u8>, ApnsError> {
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|_| ApnsError::Config("the APNs key is not a PEM private key".into()))
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub struct ApnsProvider {
    cfg: ApnsConfig,
    key: EcdsaKeyPair,
    http: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

impl ApnsProvider {
    pub fn new(cfg: ApnsConfig) -> Result<ApnsProvider, ApnsError> {
        let der = pem_der(&cfg.key_pem)?;
        let key = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            &der,
            &ring::rand::SystemRandom::new(),
        )
        .map_err(|_| {
            ApnsError::Config("the APNs key is not an ES256 (P-256) PKCS #8 key".into())
        })?;
        let mut http = reqwest::Client::builder().timeout(Duration::from_secs(15));
        if cfg.endpoint_override.is_some() {
            http = http.http2_prior_knowledge();
        }
        let http = http
            .build()
            .map_err(|e| ApnsError::Config(format!("http client: {e}")))?;
        Ok(ApnsProvider {
            cfg,
            key,
            http,
            token: Mutex::new(None),
        })
    }

    /// The provider token (a signed JWT), cached for [`TOKEN_LIFE`].
    pub fn provider_token(&self) -> String {
        let mut slot = self.token.lock().expect("apns token");
        if let Some((t, at)) = slot.as_ref()
            && at.elapsed() < TOKEN_LIFE
        {
            return t.clone();
        }
        let iat = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let header = b64url(format!(r#"{{"alg":"ES256","kid":"{}"}}"#, self.cfg.key_id).as_bytes());
        let claims = b64url(format!(r#"{{"iss":"{}","iat":{iat}}}"#, self.cfg.team_id).as_bytes());
        let signing_input = format!("{header}.{claims}");
        let sig = self
            .key
            .sign(&ring::rand::SystemRandom::new(), signing_input.as_bytes())
            .expect("ES256 signing");
        let jwt = format!("{signing_input}.{}", b64url(sig.as_ref()));
        *slot = Some((jwt.clone(), Instant::now()));
        jwt
    }

    fn forget_token(&self) {
        *self.token.lock().expect("apns token") = None;
    }

    fn base(&self, reg: &Registration) -> String {
        match &self.cfg.endpoint_override {
            Some(e) => e.trim_end_matches('/').to_string(),
            None if reg.sandbox => SANDBOX.to_string(),
            None => PRODUCTION.to_string(),
        }
    }
}

/// How an APNs answer maps to a result: the HTTP status and the `reason`
/// of its JSON body.
pub fn classify(status: u16, reason: &str) -> Result<(), ProviderError> {
    match (status, reason) {
        (200, _) => Ok(()),
        (410, _) | (_, "BadDeviceToken" | "Unregistered" | "DeviceTokenNotForTopic") => {
            Err(ProviderError::InvalidToken)
        }
        (429, _) | (500..=599, _) => Err(ProviderError::Transient(format!("{status} {reason}"))),
        (403, "ExpiredProviderToken") => {
            Err(ProviderError::Transient("provider token expired".into()))
        }
        _ => Err(ProviderError::Rejected(format!("{status} {reason}"))),
    }
}

#[async_trait]
impl PushProvider for ApnsProvider {
    fn name(&self) -> &'static str {
        "apns"
    }

    fn serves(&self, reg: &Registration) -> bool {
        reg.platform == Platform::Ios && (reg.app.is_empty() || reg.app == self.cfg.topic)
    }

    async fn send(&self, reg: &Registration, payload: &Payload) -> Result<(), ProviderError> {
        let url = format!("{}/3/device/{}", self.base(reg), reg.token);
        let expires = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + EXPIRES_IN_SECS;
        let r = self
            .http
            .post(url)
            .header("authorization", format!("bearer {}", self.provider_token()))
            .header("apns-topic", &self.cfg.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", if payload.urgent { "10" } else { "5" })
            .header("apns-expiration", expires.to_string())
            .header("apns-collapse-id", &payload.collapse)
            .json(&payload.apns_json())
            .send()
            .await
            .map_err(|e| ProviderError::Transient(e.to_string()))?;
        let status = r.status().as_u16();
        if status == 200 {
            return Ok(());
        }
        let reason = r
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["reason"].as_str().map(str::to_string))
            .unwrap_or_default();
        if reason == "ExpiredProviderToken" || reason == "InvalidProviderToken" {
            self.forget_token();
        }
        classify(status, &reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, PushEvent};
    use ring::signature::{ECDSA_P256_SHA256_FIXED, KeyPair as _, UnparsedPublicKey};
    use std::sync::Arc;

    /// A throwaway P-256 key as a `.p8` file would hold it.
    fn test_key() -> (String, Vec<u8>) {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref());
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{b64}\n-----END PRIVATE KEY-----\n");
        (pem, pair.public_key().as_ref().to_vec())
    }

    fn cfg(pem: String, endpoint: Option<String>) -> ApnsConfig {
        ApnsConfig {
            key_pem: pem,
            key_id: "ABC1234567".into(),
            team_id: "TEAM123456".into(),
            topic: DEFAULT_TOPIC.into(),
            endpoint_override: endpoint,
        }
    }

    fn registration(token: &str) -> Registration {
        Registration {
            id: "push_1".into(),
            platform: Platform::Ios,
            token: token.into(),
            app: DEFAULT_TOPIC.into(),
            sandbox: false,
            name: String::new(),
            categories: vec!["needs_you".into(), "done".into()],
            locale: "en".into(),
            bound: true,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn the_provider_token_is_a_valid_es256_jwt_for_the_team_and_key() {
        let (pem, public) = test_key();
        let p = ApnsProvider::new(cfg(pem, None)).unwrap();
        let jwt = p.provider_token();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let dec = |s: &str| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(s)
                .unwrap()
        };
        let header: serde_json::Value = serde_json::from_slice(&dec(parts[0])).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&dec(parts[1])).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "ABC1234567");
        assert_eq!(claims["iss"], "TEAM123456");
        assert!(claims["iat"].as_u64().unwrap() > 1_700_000_000);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &dec(parts[2]),
            )
            .expect("the signature verifies with the key's public half");
        assert_eq!(p.provider_token(), jwt, "cached");
    }

    #[test]
    fn a_bad_key_is_refused_at_start_and_never_printed() {
        assert!(ApnsProvider::new(cfg("not a key".into(), None)).is_err());
        let (pem, _) = test_key();
        let shown = format!("{:?}", cfg(pem.clone(), None));
        assert!(!shown.contains(pem.lines().nth(1).unwrap()));
    }

    #[test]
    fn the_environment_names_the_key_file_or_nothing() {
        assert!(ApnsConfig::from_vars(|_| None).unwrap().is_none());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AuthKey.p8");
        let (pem, _) = test_key();
        std::fs::write(&path, &pem).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // Readable by others: refused, naming the fix, without the key.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let p = path.to_string_lossy().to_string();
            let named = |k: &str| match k {
                "CUA_PUSH_APNS_KEY_FILE" => Some(p.clone()),
                "CUA_PUSH_APNS_KEY_ID" => Some("K".into()),
                "CUA_PUSH_APNS_TEAM_ID" => Some("T".into()),
                _ => None,
            };
            let e = ApnsConfig::from_vars(named).unwrap_err().to_string();
            assert!(
                e.contains("readable by others (mode 644)") && e.contains("0600"),
                "{e}"
            );
            assert!(!e.contains(pem.lines().nth(1).unwrap()));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let p = path.to_string_lossy().to_string();
        let only_file = |k: &str| (k == "CUA_PUSH_APNS_KEY_FILE").then(|| p.clone());
        let e = ApnsConfig::from_vars(only_file).unwrap_err().to_string();
        assert!(e.contains("CUA_PUSH_APNS_KEY_ID"), "{e}");
        let all = |k: &str| match k {
            "CUA_PUSH_APNS_KEY_FILE" => Some(p.clone()),
            "CUA_PUSH_APNS_KEY_ID" => Some("K".into()),
            "CUA_PUSH_APNS_TEAM_ID" => Some("T".into()),
            _ => None,
        };
        let c = ApnsConfig::from_vars(all).unwrap().unwrap();
        assert_eq!(c.topic, DEFAULT_TOPIC);
        assert!(ApnsProvider::new(c).is_ok());
    }

    #[test]
    fn answers_map_to_results() {
        assert!(classify(200, "").is_ok());
        for (s, r) in [
            (410, "Unregistered"),
            (400, "BadDeviceToken"),
            (400, "DeviceTokenNotForTopic"),
        ] {
            assert_eq!(classify(s, r), Err(ProviderError::InvalidToken), "{s} {r}");
        }
        assert!(matches!(
            classify(429, "TooManyRequests"),
            Err(ProviderError::Transient(_))
        ));
        assert!(matches!(
            classify(503, "ServiceUnavailable"),
            Err(ProviderError::Transient(_))
        ));
        assert!(matches!(
            classify(403, "ExpiredProviderToken"),
            Err(ProviderError::Transient(_))
        ));
        assert!(matches!(
            classify(400, "PayloadTooLarge"),
            Err(ProviderError::Rejected(_))
        ));
    }

    #[test]
    fn it_serves_ios_phones_of_its_app_only() {
        let (pem, _) = test_key();
        let p = ApnsProvider::new(cfg(pem, None)).unwrap();
        assert!(p.serves(&registration("t")));
        let mut other = registration("t");
        other.app = "com.example.other".into();
        assert!(!p.serves(&other));
        let mut android = registration("t");
        android.platform = Platform::Android;
        assert!(!p.serves(&android));
    }

    /// A local HTTP/2 server stands in for Apple: the request is the one
    /// Apple documents, and its answers are mapped.
    #[tokio::test]
    async fn the_request_matches_what_apple_documents() {
        use bytes::Bytes;
        use http_body_util::{BodyExt as _, Full};
        use hyper::{Request, Response, body::Incoming, service::service_fn};
        use hyper_util::rt::{TokioExecutor, TokioIo};
        use hyper_util::server::conn::auto::Builder;

        let seen: Arc<Mutex<Vec<(String, hyper::HeaderMap, serde_json::Value)>>> = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let log = log.clone();
                tokio::spawn(async move {
                    let svc = service_fn(move |req: Request<Incoming>| {
                        let log = log.clone();
                        async move {
                            let path = req.uri().path().to_string();
                            let headers = req.headers().clone();
                            let body = req.into_body().collect().await.unwrap().to_bytes();
                            let json: serde_json::Value =
                                serde_json::from_slice(&body).unwrap_or_default();
                            log.lock().unwrap().push((path.clone(), headers, json));
                            let (status, body) = if path.ends_with("deadbeef") {
                                (410, r#"{"reason":"Unregistered"}"#)
                            } else {
                                (200, "")
                            };
                            Ok::<_, std::convert::Infallible>(
                                Response::builder()
                                    .status(status)
                                    .body(Full::new(Bytes::from(body)))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        });

        let (pem, _) = test_key();
        let p = ApnsProvider::new(cfg(pem, Some(format!("http://{addr}")))).unwrap();
        let payload = PushEvent {
            id: "n1".into(),
            kind: EventKind::ApprovalNeeded,
            space: Some("relay:abc".into()),
            entry: Some(9),
            agent: Some("Claude Code".into()),
            scope_choices: vec!["once".into()],
            at_ms: 0,
        }
        .payload();
        p.send(&registration(&"ab".repeat(32)), &payload)
            .await
            .unwrap();
        let (path, headers, body) = seen.lock().unwrap().remove(0);
        assert_eq!(path, format!("/3/device/{}", "ab".repeat(32)));
        assert_eq!(headers["apns-topic"], DEFAULT_TOPIC);
        assert_eq!(headers["apns-push-type"], "alert");
        assert_eq!(headers["apns-priority"], "10");
        assert!(
            headers["authorization"]
                .to_str()
                .unwrap()
                .starts_with("bearer ey")
        );
        assert!(
            headers["apns-expiration"]
                .to_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > 1_700_000_000
        );
        assert_eq!(body["aps"]["category"], "CUA_APPROVAL");
        assert_eq!(body["entry"], 9);
        // Apple's "gone" answer is an invalid token.
        let gone = p.send(&registration("deadbeef"), &payload).await;
        assert_eq!(gone, Err(ProviderError::InvalidToken));
    }

    /// Apple's "gone" answers (410, `Unregistered`, `BadDeviceToken`) remove
    /// the registration: the whole path, hub to a local HTTP/2 server.
    #[tokio::test]
    async fn a_token_apple_says_is_gone_is_removed_from_the_registry() {
        use crate::hub::PushHub;
        use crate::registry::Registry;
        use bytes::Bytes;
        use http_body_util::Full;
        use hyper::{Request, Response, body::Incoming, service::service_fn};
        use hyper_util::rt::{TokioExecutor, TokioIo};
        use hyper_util::server::conn::auto::Builder;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let svc = service_fn(|req: Request<Incoming>| async move {
                        let path = req.uri().path().to_string();
                        let (status, body) = if path.ends_with("dead") {
                            (410, r#"{"reason":"Unregistered"}"#)
                        } else if path.ends_with("bad0") {
                            (400, r#"{"reason":"BadDeviceToken"}"#)
                        } else if path.ends_with("busy") {
                            (503, r#"{"reason":"ServiceUnavailable"}"#)
                        } else {
                            (200, "")
                        };
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(status)
                                .body(Full::new(Bytes::from(body)))
                                .unwrap(),
                        )
                    });
                    let _ = Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        });
        let (pem, _) = test_key();
        let provider = ApnsProvider::new(cfg(pem, Some(format!("http://{addr}")))).unwrap();
        let hub = PushHub::new(Registry::in_memory(), Arc::new(provider));
        let token = |tail: &str| format!("{}{tail}", "a".repeat(60));
        for (n, tail) in ["dead", "bad0", "busy", "fine"].iter().enumerate() {
            hub.registry
                .register(
                    "acct",
                    &format!("dev_{n:08}"),
                    Platform::Ios,
                    &token(tail),
                    DEFAULT_TOPIC,
                    false,
                    "",
                    None,
                    "en",
                    true,
                )
                .unwrap();
        }
        let ev = PushEvent {
            id: "e1".into(),
            kind: EventKind::TaskFailed,
            space: None,
            entry: None,
            agent: None,
            scope_choices: vec![],
            at_ms: 0,
        };
        let d = hub.notify("acct", &ev).await;
        assert_eq!((d.sent, d.dropped, d.failed), (1, 2, 1), "{d:?}");
        let left: Vec<String> = hub
            .registry
            .phones("acct")
            .into_iter()
            .map(|p| p.token)
            .collect();
        assert_eq!(left.len(), 2);
        assert!(
            left.iter()
                .all(|t| t.ends_with("busy") || t.ends_with("fine")),
            "{left:?}"
        );
    }
}
