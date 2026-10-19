// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay side of a cloud sandbox: the machine it joins as (registered
//! by this device as the signed-in cua.ai account, so the cloud only ever
//! holds that machine's token), the wait until it is online, and how the
//! SDK reaches it (`<relay>/m/<machine>` with the account's bearer and this
//! device's session).

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use cua_host::relay::{AccountTokens, RegisterRequest};
use cua_host::{DeviceAuth, RelayClient};
use cua_sandbox_core::Error;
use serde::{Deserialize, Serialize};

use crate::api::Result;

/// What a guest needs to join the relay as one machine of the account.
/// Secret: the machine token is in it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Join {
    /// The relay the guest dials (`https://relay.cua.ai`).
    pub relay_url: String,
    /// The machine id it joins as.
    pub machine_id: String,
    /// That machine's token (`cmt_...`), the guest's only credential.
    pub machine_token: String,
    /// The relay's signing keys (JWKS JSON) the guest pins.
    pub jwks_json: String,
    /// The account that owns the machine.
    pub owner: String,
    /// The owner's verified email, when known.
    #[serde(default)]
    pub owner_email: String,
}

impl std::fmt::Debug for Join {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Join")
            .field("relay_url", &self.relay_url)
            .field("machine_id", &self.machine_id)
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl Join {
    /// The owner policy the guest's cua-spacesd enforces (the host policy
    /// shape): the owner, sharing on, the relay's allowlist trusted.
    pub fn policy_json(&self) -> String {
        serde_json::json!({
            "owner": self.owner,
            "owner_email": (!self.owner_email.is_empty()).then_some(&self.owner_email),
            "sharing": true,
            "trust_relay_allowlist": true,
        })
        .to_string()
    }

    /// The environment `cua-spacesd join` reads to join as this machine
    /// without files (images whose driver reads `CUA_ENV_MACHINE_ID`,
    /// `CUA_RELAY_JWKS_JSON` and `CUA_HOST_POLICY_JSON`). Secret.
    pub fn guest_env(&self) -> BTreeMap<String, String> {
        let mut env: BTreeMap<String, String> = crate::bootstrap::JOIN_ARG_VARS
            .iter()
            .map(|k| ((*k).to_string(), "join".to_string()))
            .collect();
        env.extend(BTreeMap::from([
            ("CUA_ENV_RELAY_URL".into(), self.relay_url.clone()),
            ("CUA_RELAY_TOKEN".into(), self.machine_token.clone()),
            ("CUA_ENV_MACHINE_ID".into(), self.machine_id.clone()),
            ("CUA_RELAY_JWKS_JSON".into(), self.jwks_json.clone()),
            ("CUA_HOST_POLICY_JSON".into(), self.policy_json()),
        ]));
        env
    }
}

/// The signed-in account on a relay, as this device.
#[derive(Clone)]
pub struct RelayAccess {
    /// Relay base URL.
    pub url: String,
    /// Fresh account tokens.
    pub tokens: Arc<dyn AccountTokens>,
    /// This device (its session rides on every call).
    pub device: Option<Arc<DeviceAuth>>,
}

impl std::fmt::Debug for RelayAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayAccess")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// The `cua auth login` session as account tokens (read per call, so a
/// sign-in after start works).
struct SessionTokens;

#[async_trait::async_trait]
impl AccountTokens for SessionTokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        cua_auth::Session::from_env()
            .access_token(false)
            .await
            .map_err(|e| cua_host::Error::Unauthenticated(e.to_string()))
    }
}

fn relay_err(e: cua_host::Error) -> Error {
    match e {
        cua_host::Error::Unauthenticated(m) => Error::Cloud(format!(
            "cua.ai sign-in: {m} (cloud sandboxes join the cua.ai relay as machines of your \
             account; run `cua auth login`)"
        )),
        other => Error::Cloud(format!("cua.ai relay: {other}")),
    }
}

impl RelayAccess {
    /// `url` with `tokens` (tests, fixtures).
    pub fn new(url: impl Into<String>, tokens: Arc<dyn AccountTokens>) -> Self {
        RelayAccess {
            url: url.into(),
            tokens,
            device: None,
        }
    }

    /// Sends `device`'s session with every call.
    pub fn with_device(mut self, device: Arc<DeviceAuth>) -> Self {
        self.device = Some(device);
        self
    }

    /// The relay of `CUA_RELAY_URL` (else relay.cua.ai), the `cua auth
    /// login` session, and this device's enrollment.
    pub fn from_env() -> Self {
        let url = cua_host::relay_url_from_env();
        let tokens: Arc<dyn AccountTokens> = Arc::new(SessionTokens);
        let access = RelayAccess::new(url.clone(), tokens.clone());
        match DeviceAuth::new(
            &url,
            tokens,
            Arc::new(cua_auth::Store::from_env()),
            cua_host::device_name(),
        ) {
            Ok(d) => access.with_device(Arc::new(d)),
            Err(_) => access,
        }
    }

    async fn client(&self) -> Result<(RelayClient, String)> {
        let token = self.tokens.access_token().await.map_err(relay_err)?;
        let session = match &self.device {
            Some(d) => d.try_session().await,
            None => None,
        };
        let client = RelayClient::new(&self.url)
            .map_err(relay_err)?
            .with_device_session(session);
        Ok((client, token))
    }

    /// Registers machine `id` shown as `name`.
    pub async fn register(&self, id: &str, name: &str) -> Result<Join> {
        self.register_with(id, name, Default::default()).await
    }

    /// [`RelayAccess::register`] with machine metadata (no secrets; relays
    /// that predate it ignore it).
    pub async fn register_with(
        &self,
        id: &str,
        name: &str,
        meta: BTreeMap<String, String>,
    ) -> Result<Join> {
        let (client, token) = self.client().await?;
        let reg = client
            .register(
                &token,
                &RegisterRequest {
                    id: id.into(),
                    name: name.into(),
                    allow: vec![],
                    host: None,
                    meta,
                },
            )
            .await
            .map_err(relay_err)?;
        Ok(Join {
            relay_url: self.url.clone(),
            machine_id: reg.machine.id.clone(),
            machine_token: reg.machine_token,
            jwks_json: if reg.jwks.is_null() {
                String::new()
            } else {
                reg.jwks.to_string()
            },
            owner: reg.machine.owner.id.clone(),
            owner_email: reg.machine.owner.email.clone().unwrap_or_default(),
        })
    }

    /// Removes machine `id` (its token is revoked). Gone already is fine.
    pub async fn forget(&self, id: &str) -> Result<()> {
        let (client, token) = self.client().await?;
        match client.delete(&token, id).await {
            Ok(()) | Err(cua_host::Error::NotFound(_)) => Ok(()),
            Err(e) => Err(relay_err(e)),
        }
    }

    /// Whether machine `id` is online (`None`: not registered).
    pub async fn online(&self, id: &str) -> Result<Option<bool>> {
        let (client, token) = self.client().await?;
        match client.machine(&token, id).await {
            Ok(m) => Ok(Some(m.online)),
            Err(cua_host::Error::NotFound(_)) => Ok(None),
            Err(e) => Err(relay_err(e)),
        }
    }

    /// Waits until `id` is online, up to `timeout`, running `alive` every
    /// few polls (a machine that died fails at once with its reason).
    pub async fn wait_online<F, Fut>(&self, id: &str, timeout: Duration, mut alive: F) -> Result<()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut polls = 0u32;
        loop {
            let last_error = match self.online(id).await {
                Ok(Some(true)) => return Ok(()),
                Ok(_) => None,
                // A relay that is restarting (502, 503) is not a verdict on
                // the machine: keep waiting. A sign-in that stopped working
                // is.
                Err(e) if e.to_string().contains("cua.ai sign-in") => return Err(e),
                Err(e) => Some(e.to_string()),
            };
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(match last_error {
                    Some(e) => format!(
                        "{id} did not come online on the relay within {}s (the relay last \
                         answered: {e})",
                        timeout.as_secs()
                    ),
                    None => format!(
                        "{id} did not come online on the relay within {}s",
                        timeout.as_secs()
                    ),
                }));
            }
            polls += 1;
            if polls.is_multiple_of(4) {
                alive().await?;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// `<relay>/m/<machine>`: where the machine's cua-spacesd answers.
    pub fn machine_url(&self, machine: &str) -> String {
        format!("{}/m/{machine}", self.url.trim_end_matches('/'))
    }

    /// Connect options for the machine's cua-spacesd through the relay.
    pub fn connect_options(&self, machine: &str) -> Result<cua_spacesd_client::ConnectOptions> {
        let mut o = cua_spacesd_client::ConnectOptions::parse(&self.machine_url(machine))
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        o.gateway = Some(cua_spacesd_client::GatewayAuth {
            bearer: Arc::new(Bearer(self.clone())),
            claim: None,
        });
        Ok(o)
    }
}

/// The relay gateway bearer: the account token plus this device's
/// session header, fetched per call.
struct Bearer(RelayAccess);

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl cua_spacesd_client::BearerProvider for Bearer {
    fn bearer(
        &self,
        _force_refresh: bool,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, BoxError>> + Send + '_>> {
        Box::pin(async move {
            self.0
                .tokens
                .access_token()
                .await
                .map_err(|e| Box::new(e) as BoxError)
        })
    }

    fn extra_headers(
        &self,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<cua_spacesd_client::ExtraHeaders, BoxError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let session = match &self.0.device {
                Some(d) => d.try_session().await,
                None => None,
            };
            Ok(session
                .map(|s| vec![(cua_host::relay::DEVICE_SESSION_HEADER.to_string(), s)])
                .unwrap_or_default())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_join_env_and_policy_carry_the_machine_and_owner() {
        let j = Join {
            relay_url: "https://relay.example".into(),
            machine_id: "cloud-0123456789abcdef".into(),
            machine_token: "cmt_secret".into(),
            jwks_json: r#"{"keys":[]}"#.into(),
            owner: "acct".into(),
            owner_email: "a@example.com".into(),
        };
        let env = j.guest_env();
        assert_eq!(env["CUA_SPACESD_ARGS"], "join");
        assert_eq!(env["CUA_ENV_MACHINE_ID"], "cloud-0123456789abcdef");
        let policy: serde_json::Value = serde_json::from_str(&j.policy_json()).unwrap();
        assert_eq!(policy["owner"], "acct");
        assert!(!format!("{j:?}").contains("cmt_secret"));
    }
}
