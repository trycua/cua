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

/// Whether `m` is a Space that registered itself on the relay as its own
/// machine (`relay_register`, or its first share): not a host from `cua
/// host setup`, not a Space a host provides, not a Space in someone's own
/// cloud. Only that Space's own driver ever joins as it, so once the Space
/// is gone the record is stale.
pub(crate) fn is_registered_space(m: &RelayMachine) -> bool {
    m.host.as_deref().is_none_or(str::is_empty)
        && !crate::cloud::is_cloud_machine(m)
        && !m.meta.contains_key(cua_host::META_PROVIDES_SPACES)
        && m.id.starts_with("space-")
}

impl crate::Spaces {
    /// Removes `relay:<machine_id>` from the account's relay directory and
    /// forgets it here. Only the machine's owner may: a machine shared with
    /// you is refused here, and the relay refuses anyone but the owner
    /// (`DELETE /v1/machines/{id}` is owner-only). A host (or a Space in
    /// your cloud) that is connected now is refused too: `cua host remove`
    /// on it takes it off cleanly (`cua spaces delete` deletes the other). A
    /// Space this device attached to the relay is detached first. Returns
    /// the removed row, or `None` when the directory no longer lists it
    /// (it is forgotten here all the same).
    pub(crate) async fn forget_relay_machine(
        &self,
        machine_id: &str,
    ) -> crate::Result<Option<RelayMachine>> {
        use crate::Error;
        let id = format!("relay:{machine_id}");
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let m = match client.machine(&token, machine_id).await {
            Ok(m) => m,
            Err(cua_host::Error::NotFound(_)) => {
                // Already gone from the directory: forget it here.
                self.forget_relay_locally(machine_id).await;
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        if m.role != "owner" {
            let owner = m
                .owner
                .email
                .clone()
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| m.owner.id.clone());
            return Err(Error::Relay(cua_host::Error::PermissionDenied(format!(
                "{id} is shared with you by {owner}; only its owner can remove it from the relay directory"
            ))));
        }
        // A Space a host provides is the account's own registration (its
        // host keeps providing it, or it is a stale entry): it goes. A host
        // or a Space in your cloud that is connected now does not.
        let provided = m.host.as_deref().is_some_and(|h| !h.is_empty());
        if m.online && !provided && !is_registered_space(&m) {
            let name = if m.name.is_empty() { &m.id } else { &m.name };
            return Err(Error::invalid(if crate::cloud::is_cloud_machine(&m) {
                format!("{id} ({name}) is a running Space; delete it with `cua spaces delete {id}`")
            } else {
                format!(
                    "{id} ({name}) is one of your machines and is connected now; take it off \
                     the relay with `cua host remove` on that machine"
                )
            }));
        }
        #[cfg(feature = "spaces-agents")]
        self.detach_attached_driver(machine_id).await;
        match client.delete(&token, machine_id).await {
            Ok(()) | Err(cua_host::Error::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        self.forget_relay_locally(machine_id).await;
        Ok(Some(m))
    }

    /// Forgets `relay:<machine_id>` on this device: the cached row, any
    /// registry entry, its thumbnail and connection, and (with sharing) the
    /// Space attached as it.
    async fn forget_relay_locally(&self, machine_id: &str) {
        let id = crate::SpaceId::Relay {
            machine_id: machine_id.to_string(),
        };
        self.drop_connection(&id).await;
        self.inner
            .relay_cache
            .lock()
            .expect("relay cache")
            .retain(|m| m.id != machine_id);
        let _ = self.inner.registry.remove(&id.to_string());
        self.inner.thumbnails.remove(&id.to_string());
        #[cfg(feature = "spaces-agents")]
        self.forget_attached_machine(machine_id);
    }
}
