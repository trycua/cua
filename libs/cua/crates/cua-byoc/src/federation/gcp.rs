// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Google Cloud: an issuer token (audience: the workload identity
//! provider's full resource name) exchanged at Google's Security Token
//! Service for a federated access token, used directly (no service
//! account), or, when the connection names one, exchanged once more for
//! that service account's token (IAM Credentials `generateAccessToken`).
//! Cached until 2 minutes before it expires.

use std::sync::Arc;

use serde_json::Value;

use super::{
    CachedToken, CloudRefused, ExchangeError, IssuerClient, Purpose, TokenRequest, now_secs,
};

/// Google's Security Token Service.
pub const STS_URL: &str = "https://sts.googleapis.com/v1/token";

/// The IAM Credentials API root.
pub const IAM_CREDENTIALS_URL: &str = "https://iamcredentials.googleapis.com/v1";

/// The scope Cua asks for.
pub const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

pub use crate::trust::gcp::{POOL, PROVIDER, ROLE, project_number_of, provider_name};

/// One connection's Google access token for one purpose and agent.
pub struct Exchange {
    issuer: Arc<dyn IssuerClient>,
    http: reqwest::Client,
    sts_url: String,
    iam_url: String,
    request: TokenRequest,
    service_account: String,
    cache: CachedToken,
}

impl std::fmt::Debug for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("gcp::Exchange")
            .field("connection", &self.request.connection)
            .field("audience", &self.request.audience)
            .field("purpose", &self.request.purpose)
            .finish_non_exhaustive()
    }
}

impl Exchange {
    /// Tokens for connection `conn_id` through workload identity provider
    /// `provider` (its full resource name, the token's audience), acting as
    /// `service_account` when not empty, at `sts_url` and `iam_url`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        issuer: Arc<dyn IssuerClient>,
        sts_url: &str,
        iam_url: &str,
        conn_id: &str,
        provider: &str,
        service_account: &str,
        purpose: Purpose,
        agent: Option<String>,
    ) -> Self {
        Exchange {
            issuer,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            sts_url: sts_url.into(),
            iam_url: iam_url.trim_end_matches('/').into(),
            request: TokenRequest {
                connection: conn_id.into(),
                purpose,
                audience: provider.into(),
                agent,
            },
            service_account: service_account.into(),
            cache: CachedToken::default(),
        }
    }

    /// Forgets the cached token (Google said it expired).
    pub async fn forget(&self) {
        self.cache.forget().await;
    }

    /// A fresh (or cached) access token.
    pub async fn access_token(&self) -> Result<String, ExchangeError> {
        self.cache.get_or(|| self.exchange()).await
    }

    async fn exchange(&self) -> Result<(String, u64), ExchangeError> {
        let minted = self
            .issuer
            .token(&self.request)
            .await
            .map_err(ExchangeError::Issuer)?;
        let form = [
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("audience", self.request.audience.as_str()),
            ("scope", SCOPE),
            (
                "requested_token_type",
                "urn:ietf:params:oauth:token-type:access_token",
            ),
            ("subject_token", minted.token.as_str()),
            ("subject_token_type", "urn:ietf:params:oauth:token-type:jwt"),
        ];
        let resp = self
            .http
            .post(&self.sts_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| ExchangeError::Other(format!("Google STS: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let code = v["error"].as_str().unwrap_or_default().to_string();
            let msg = v["error_description"]
                .as_str()
                .or_else(|| v["error"]["message"].as_str())
                .unwrap_or_default()
                .to_string();
            return Err(ExchangeError::Refused(CloudRefused {
                cloud: "Google Cloud",
                who: self.request.audience.clone(),
                detail: if code.is_empty() {
                    format!("HTTP {status}: {msg}")
                } else {
                    format!("{code}: {msg}")
                },
                trust: status == 400 || status == 401 || status == 403,
                hint: "the cua workload identity pool is missing, still being set up, or trusts \
                       another Cua account (run `cua cloud connect gcp --oidc` for the Cloud Shell \
                       link that sets it up)",
            }));
        }
        let token = v["access_token"].as_str().unwrap_or_default().to_string();
        if token.is_empty() {
            return Err(ExchangeError::Other(
                "Google STS answered without an access token".into(),
            ));
        }
        let exp = now_secs() + v["expires_in"].as_u64().unwrap_or(3600);
        if self.service_account.is_empty() {
            return Ok((token, exp));
        }
        self.impersonate(&token).await
    }

    /// The service account's own token, through the federated one.
    async fn impersonate(&self, federated: &str) -> Result<(String, u64), ExchangeError> {
        let url = format!(
            "{}/projects/-/serviceAccounts/{}:generateAccessToken",
            self.iam_url, self.service_account
        );
        let body = serde_json::json!({"scope": [SCOPE], "lifetime": "900s"});
        let resp = self
            .http
            .post(&url)
            .bearer_auth(federated)
            .json(&body)
            .send()
            .await
            .map_err(|e| ExchangeError::Other(format!("IAM Credentials: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            return Err(ExchangeError::Refused(CloudRefused {
                cloud: "Google Cloud",
                who: self.service_account.clone(),
                detail: v["error"]["message"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("HTTP {status}")),
                trust: status == 403,
                hint: "grant the Cua identities roles/iam.workloadIdentityUser on that service \
                       account, or connect without --service-account",
            }));
        }
        let token = v["accessToken"].as_str().unwrap_or_default().to_string();
        if token.is_empty() {
            return Err(ExchangeError::Other(
                "IAM Credentials answered without an access token".into(),
            ));
        }
        let exp = v["expireTime"]
            .as_str()
            .and_then(|t| humantime::parse_rfc3339_weak(t.get(..19).unwrap_or(t)).ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or_else(|| now_secs() + 900);
        Ok((token, exp))
    }
}
