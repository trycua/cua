//! Fleet through the signed-in user's session (`cua auth login`, the SDK's
//! `Auth`, the Spaces app): a refreshing bearer from the shared credential
//! store (`cua-auth`), so every process sees the same, current token.

use cua_fleet::sdk::{AccessTokenProvider, AccessTokenProviderError};
use cua_fleet::{FleetClient, FleetConfig};
use std::sync::Arc;

/// A Fleet bearer from a `cua-auth` session.
pub struct SessionTokens(pub Arc<cua_auth::Session>);

#[async_trait::async_trait]
impl AccessTokenProvider for SessionTokens {
    async fn get_access_token(
        &self,
        force_refresh: bool,
    ) -> std::result::Result<String, AccessTokenProviderError> {
        self.0
            .access_token(force_refresh)
            .await
            .map_err(|e| AccessTokenProviderError::Failed {
                reason: e.to_string(),
            })
    }
}

/// A Fleet client on the stored session when `config` carries no
/// credentials of its own and a session exists; `None` otherwise (the
/// Fleet provider then stays unconfigured, as before).
pub fn session_fleet_client(config: &FleetConfig) -> Option<FleetClient> {
    if config.has_auth() {
        return None;
    }
    let session = cua_auth::Session::from_env();
    session.credentials().ok().flatten()?;
    FleetClient::connect_with_token_provider(
        config.clone(),
        Arc::new(SessionTokens(Arc::new(session))),
        None,
    )
    .ok()
}
