// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Azure: an issuer token (audience `api://AzureADTokenExchange`) used as
//! the client assertion of an Entra `client_credentials` grant for the
//! `cua-spaces` managed identity, which gives an Azure Resource Manager
//! token. Cached until 2 minutes before it expires.

use std::sync::Arc;

use serde_json::Value;

use super::{
    CachedToken, CloudRefused, ExchangeError, IssuerClient, Purpose, TokenRequest, now_secs,
};

/// Microsoft Entra's sign-in root.
pub const LOGIN_URL: &str = "https://login.microsoftonline.com";

/// The scope Cua asks for: Azure Resource Manager.
pub const SCOPE: &str = "https://management.azure.com/.default";

pub use crate::trust::azure::AUDIENCE;

/// Entra errors that mean the federated credential does not trust Cua's
/// token (none matches, the identity is gone, the issuer or subject
/// differs, the token was refused).
const UNTRUSTED: &[&str] = &[
    "AADSTS70021",
    "AADSTS700016",
    "AADSTS700024",
    "AADSTS700211",
    "AADSTS700212",
    "AADSTS700213",
    "AADSTS700223",
];

/// One connection's Azure Resource Manager token for one purpose and agent.
pub struct Exchange {
    issuer: Arc<dyn IssuerClient>,
    http: reqwest::Client,
    login_url: String,
    tenant_id: String,
    client_id: String,
    request: TokenRequest,
    cache: CachedToken,
}

impl std::fmt::Debug for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("azure::Exchange")
            .field("connection", &self.request.connection)
            .field("tenant_id", &self.tenant_id)
            .field("client_id", &self.client_id)
            .field("purpose", &self.request.purpose)
            .finish_non_exhaustive()
    }
}

impl Exchange {
    /// Tokens for connection `conn_id`, signing in as managed identity
    /// `client_id` of `tenant_id` at `login_url`.
    pub fn new(
        issuer: Arc<dyn IssuerClient>,
        login_url: &str,
        conn_id: &str,
        tenant_id: &str,
        client_id: &str,
        purpose: Purpose,
        agent: Option<String>,
    ) -> Self {
        Exchange {
            issuer,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            login_url: login_url.trim_end_matches('/').into(),
            tenant_id: tenant_id.into(),
            client_id: client_id.into(),
            request: TokenRequest {
                connection: conn_id.into(),
                purpose,
                audience: AUDIENCE.into(),
                agent,
            },
            cache: CachedToken::default(),
        }
    }

    /// Forgets the cached token (Azure said it expired).
    pub async fn forget(&self) {
        self.cache.forget().await;
    }

    /// A fresh (or cached) Azure Resource Manager token.
    pub async fn access_token(&self) -> Result<String, ExchangeError> {
        self.cache.get_or(|| self.exchange()).await
    }

    async fn exchange(&self) -> Result<(String, u64), ExchangeError> {
        let minted = self
            .issuer
            .token(&self.request)
            .await
            .map_err(ExchangeError::Issuer)?;
        let url = format!("{}/{}/oauth2/v2.0/token", self.login_url, self.tenant_id);
        let form = [
            ("grant_type", "client_credentials"),
            ("client_id", self.client_id.as_str()),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", minted.token.as_str()),
            ("scope", SCOPE),
        ];
        let resp =
            self.http.post(&url).form(&form).send().await.map_err(|e| {
                ExchangeError::Other(format!("Microsoft Entra: {}", e.without_url()))
            })?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let code = v["error"].as_str().unwrap_or_default().to_string();
            let msg = v["error_description"]
                .as_str()
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            let trust = matches!(
                code.as_str(),
                "invalid_client" | "unauthorized_client" | "invalid_grant"
            ) || UNTRUSTED.iter().any(|c| msg.contains(c))
                || (code == "invalid_request" && msg.contains("tenant"));
            return Err(ExchangeError::Refused(CloudRefused {
                cloud: "Azure",
                who: format!("identity {} in tenant {}", self.client_id, self.tenant_id),
                detail: if code.is_empty() {
                    format!("HTTP {status}: {msg}")
                } else {
                    format!("{code}: {msg}")
                },
                trust,
                hint: "the cua-spaces identity or its federated credential is missing, still being \
                       created, or trusts another Cua account (run `cua cloud connect azure` for \
                       the Deploy to Azure link that sets it up)",
            }));
        }
        let token = v["access_token"].as_str().unwrap_or_default().to_string();
        if token.is_empty() {
            return Err(ExchangeError::Other(
                "Microsoft Entra answered without an access token".into(),
            ));
        }
        Ok((token, now_secs() + v["expires_in"].as_u64().unwrap_or(3600)))
    }
}
