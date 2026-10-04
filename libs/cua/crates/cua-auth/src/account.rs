//! The Cua account API at `https://run.cua.ai` (`CUA_FLEET_BASE_URL`): the
//! account's billing status (`GET /api/billing/status`), and the base URL
//! and bearer `cua-volume` uses for its `cloud` backend.
//!
//! The bearer is the one the cloud sandbox client used: a static token
//! (`FLEETS_TOKEN`), OAuth client credentials (`CUA_CLIENT_ID` +
//! `CUA_CLIENT_SECRET` at `CUA_TOKEN_URL`), or a signed-in [`Session`].

use crate::{Error, Result, Session, http_err};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Default account API base URL (`CUA_FLEET_BASE_URL`).
pub const DEFAULT_BASE_URL: &str = "https://run.cua.ai";
/// Default OAuth token endpoint for client credentials (`CUA_TOKEN_URL`).
pub const DEFAULT_TOKEN_URL: &str =
    "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token";

/// A source of bearer tokens for the account API.
#[async_trait::async_trait]
pub trait AccessTokens: Send + Sync {
    /// A valid access token, refreshed when `force_refresh`.
    async fn access_token(&self, force_refresh: bool) -> Result<String>;
}

#[async_trait::async_trait]
impl AccessTokens for Session {
    async fn access_token(&self, force_refresh: bool) -> Result<String> {
        Session::access_token(self, force_refresh).await
    }
}

struct StaticToken(String);

#[async_trait::async_trait]
impl AccessTokens for StaticToken {
    async fn access_token(&self, _force_refresh: bool) -> Result<String> {
        Ok(self.0.clone())
    }
}

/// OAuth client credentials, cached until shortly before they expire.
struct ClientCredentials {
    http: reqwest::Client,
    token_url: String,
    client_id: String,
    client_secret: String,
    cached: tokio::sync::Mutex<Option<(String, Instant)>>,
}

#[async_trait::async_trait]
impl AccessTokens for ClientCredentials {
    async fn access_token(&self, force_refresh: bool) -> Result<String> {
        let mut cached = self.cached.lock().await;
        if !force_refresh
            && let Some((token, until)) = cached.as_ref()
            && Instant::now() < *until
        {
            return Ok(token.clone());
        }
        let r = self
            .http
            .post(&self.token_url)
            .header("accept", "application/json")
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(http_err)?;
        let status = r.status().as_u16();
        let v: serde_json::Value = r.json().await.unwrap_or_default();
        if status != 200 {
            return Err(Error::Unauthenticated(format!(
                "client credentials refused (HTTP {status})"
            )));
        }
        let token = v["access_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| Error::Http("token response without access_token".into()))?
            .to_string();
        let ttl = v["expires_in"].as_u64().unwrap_or(300).saturating_sub(30);
        *cached = Some((token.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(token)
    }
}

/// The saved card (never its number).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingCard {
    /// `visa`, `mastercard`, ...
    pub brand: String,
    /// Last four digits.
    pub last4: String,
    /// Expiry month (1-12).
    pub exp_month: u32,
    /// Expiry year.
    pub exp_year: u32,
}

/// The account's credit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingCredit {
    /// What is left, US cents.
    pub balance_usd_cents: i64,
    /// The signup grant's amount, US cents (0: none).
    #[serde(default)]
    pub signup_grant_usd_cents: i64,
    /// A signup grant exists and none of it has been used.
    #[serde(default)]
    pub signup_grant_unused: bool,
}

/// `GET /api/billing/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingStatus {
    /// Billing is on for this account (off: nothing to set up).
    pub billing_enabled: bool,
    /// A default card is saved.
    pub payment_method_present: bool,
    /// That card.
    #[serde(default)]
    pub card: Option<BillingCard>,
    /// `none`, `payg`, or a plan key.
    #[serde(default = "none_plan")]
    pub plan: String,
    /// A card can be added for pay as you go.
    #[serde(default)]
    pub payg_available: bool,
    /// The account's credit (none when the account has no credit).
    #[serde(default)]
    pub credit: Option<BillingCredit>,
    /// The website billing page.
    #[serde(default)]
    pub billing_url: Option<String>,
}

fn none_plan() -> String {
    "none".into()
}

impl BillingStatus {
    /// What an API without the status route means: billing off.
    pub fn disabled() -> Self {
        Self {
            billing_enabled: false,
            payment_method_present: false,
            card: None,
            plan: none_plan(),
            payg_available: false,
            credit: None,
            billing_url: None,
        }
    }
}

/// A client of the account API. Cheap to clone.
#[derive(Clone)]
pub struct AccountApi {
    base_url: String,
    tokens: Arc<dyn AccessTokens>,
    http: reqwest::Client,
}

impl std::fmt::Debug for AccountApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountApi")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

fn non_empty(get: &dyn Fn(&str) -> Option<String>, k: &str) -> Option<String> {
    get(k)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl AccountApi {
    /// The account API at `base_url` with `tokens` as the bearer.
    pub fn new(base_url: &str, tokens: Arc<dyn AccessTokens>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            tokens,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// The base URL from `CUA_FLEET_BASE_URL`, else [`DEFAULT_BASE_URL`].
    pub fn base_url_from_env() -> String {
        Self::base_url_from_lookup(&|k| std::env::var(k).ok())
    }

    /// [`Self::base_url_from_env`] over an arbitrary lookup.
    pub fn base_url_from_lookup(get: &dyn Fn(&str) -> Option<String>) -> String {
        non_empty(get, "CUA_FLEET_BASE_URL")
            .unwrap_or_else(|| DEFAULT_BASE_URL.into())
            .trim_end_matches('/')
            .to_string()
    }

    /// From the environment's credentials: `FLEETS_TOKEN`, else
    /// `CUA_CLIENT_ID` + `CUA_CLIENT_SECRET` (at `CUA_TOKEN_URL`). `None`
    /// without either.
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(&|k| std::env::var(k).ok())
    }

    /// [`Self::from_env`] over an arbitrary lookup.
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let base = Self::base_url_from_lookup(get);
        if let Some(token) = non_empty(get, "FLEETS_TOKEN") {
            return Some(Self::new(&base, Arc::new(StaticToken(token))));
        }
        let (Some(client_id), Some(client_secret)) = (
            non_empty(get, "CUA_CLIENT_ID"),
            non_empty(get, "CUA_CLIENT_SECRET"),
        ) else {
            return None;
        };
        let tokens = ClientCredentials {
            http: reqwest::Client::new(),
            token_url: non_empty(get, "CUA_TOKEN_URL").unwrap_or_else(|| DEFAULT_TOKEN_URL.into()),
            client_id,
            client_secret,
            cached: tokio::sync::Mutex::new(None),
        };
        Some(Self::new(&base, Arc::new(tokens)))
    }

    /// The signed-in `session` as the bearer, at the environment's base URL.
    pub fn with_session(session: Arc<Session>) -> Self {
        Self::new(&Self::base_url_from_env(), session)
    }

    /// The stored session ([`Session::from_env`]) when one exists (no
    /// network); `None` otherwise.
    pub fn from_stored_session() -> Option<Self> {
        let session = Session::from_env();
        session.credentials().ok().flatten()?;
        Some(Self::with_session(Arc::new(session)))
    }

    /// The base URL (no trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// A bearer for the account API.
    pub async fn access_token(&self, force_refresh: bool) -> Result<String> {
        self.tokens.access_token(force_refresh).await
    }

    /// The account's billing status. An API without the route (404)
    /// answers [`BillingStatus::disabled`].
    pub async fn billing_status(&self) -> Result<BillingStatus> {
        let token = self.access_token(false).await?;
        let r = self
            .http
            .get(format!("{}/api/billing/status", self.base_url))
            .bearer_auth(token)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(http_err)?;
        match r.status().as_u16() {
            200 => r
                .json()
                .await
                .map_err(|e| Error::Http(format!("unexpected billing status: {e}"))),
            404 => Ok(BillingStatus::disabled()),
            status => Err(Error::Http(format!("read billing status: HTTP {status}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_status_parses_with_defaults() {
        let s: BillingStatus = serde_json::from_value(serde_json::json!({
            "billing_enabled": true, "payment_method_present": false
        }))
        .unwrap();
        assert_eq!((s.plan.as_str(), s.credit, s.card), ("none", None, None));
    }

    #[test]
    fn credentials_come_from_the_environment() {
        let lookup = |env: HashMap<&'static str, &'static str>| {
            move |k: &str| env.get(k).map(|v| v.to_string())
        };
        assert!(AccountApi::from_lookup(&lookup(HashMap::new())).is_none());
        let a = AccountApi::from_lookup(&lookup(
            [
                ("FLEETS_TOKEN", "t"),
                ("CUA_FLEET_BASE_URL", "https://x.test/"),
            ]
            .into(),
        ))
        .unwrap();
        assert_eq!(a.base_url(), "https://x.test");
        let a = AccountApi::from_lookup(&lookup(
            [("CUA_CLIENT_ID", "id"), ("CUA_CLIENT_SECRET", "s")].into(),
        ))
        .unwrap();
        assert_eq!(a.base_url(), DEFAULT_BASE_URL);
        assert!(
            AccountApi::from_lookup(&lookup([("CUA_CLIENT_ID", "id")].into())).is_none(),
            "both client credentials are needed"
        );
    }
}
