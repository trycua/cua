//! `relay:<machine-id>`: machines of the signed-in cua.ai account
//! (owned, or shared with it) on a `cua-relay`, listed from the relay's
//! machine directory and reached at `<relay>/m/<machine-id>` with the
//! account token as bearer. The relay verifies the account, strips it, and
//! forwards a short-lived relay-signed identity the machine's spacesd
//! checks, so no env token is ever copied between machines.
//!
//! With [`RelayAccount::with_device`] every call also carries this client
//! device's session (see `cua_host::device`): the relay requires an enrolled
//! device, not just the account token.

use std::sync::Arc;

pub use cua_host::relay::{AccountTokens, NoAccount, StaticToken};
pub use cua_host::{ConnectedClient, DEFAULT_RELAY_URL, Identity, relay_url_from_env};

/// A machine row of the relay directory.
pub type RelayMachine = cua_host::Machine;

pub use cua_host::DeviceAuth;

/// The relay and the account Spaces lists machines for.
#[derive(Clone)]
pub struct RelayAccount {
    /// Relay base URL (`https://relay.cua.ai`).
    pub url: String,
    /// Fresh account tokens (refreshed per call).
    pub tokens: Arc<dyn AccountTokens>,
    /// This client device (its session rides on every relay call).
    pub device: Option<Arc<DeviceAuth>>,
}

impl RelayAccount {
    /// `url` with `tokens`.
    pub fn new(url: impl Into<String>, tokens: Arc<dyn AccountTokens>) -> Self {
        Self {
            url: url.into(),
            tokens,
            device: None,
        }
    }

    /// Sends `device`'s session with every relay call.
    pub fn with_device(mut self, device: Arc<DeviceAuth>) -> Self {
        self.device = Some(device);
        self
    }

    /// This device's session, when it has one.
    pub async fn device_session(&self) -> Option<String> {
        match &self.device {
            Some(d) => d.try_session().await,
            None => None,
        }
    }

    /// A directory client carrying the device session.
    pub async fn client(&self) -> cua_host::Result<cua_host::RelayClient> {
        Ok(cua_host::RelayClient::new(&self.url)?.with_device_session(self.device_session().await))
    }
}

/// The gateway bearer of `relay:<id>` connections: the account token in
/// `authorization` plus the device session header, both fetched per call.
pub(crate) struct RelayBearer {
    pub(crate) account: RelayAccount,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl cua_spacesd_client::BearerProvider for RelayBearer {
    fn bearer(
        &self,
        _force_refresh: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<String, BoxError>> + Send + '_>,
    > {
        Box::pin(async move {
            self.account
                .tokens
                .access_token()
                .await
                .map_err(|e| Box::new(e) as BoxError)
        })
    }

    fn extra_headers(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<cua_spacesd_client::ExtraHeaders, BoxError>,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            Ok(self
                .account
                .device_session()
                .await
                .map(|s| vec![(cua_host::relay::DEVICE_SESSION_HEADER.to_string(), s)])
                .unwrap_or_default())
        })
    }
}

impl std::fmt::Debug for RelayAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayAccount")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}
