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
    /// The Spaces API Teams go through (`None`: the environment's,
    /// [`cua_host::spaces_api::SpacesApiClient::current`]).
    pub spaces_api: Option<String>,
}

impl RelayAccount {
    /// `url` with `tokens`.
    pub fn new(url: impl Into<String>, tokens: Arc<dyn AccountTokens>) -> Self {
        Self {
            url: url.into(),
            tokens,
            device: None,
            spaces_api: None,
        }
    }

    /// Teams (organizations) go through the Spaces API at `url` instead of
    /// the environment's.
    pub fn with_spaces_api(mut self, url: impl Into<String>) -> Self {
        self.spaces_api = Some(url.into());
        self
    }

    /// The Spaces API client Teams go through.
    pub fn spaces_api(&self) -> cua_host::Result<cua_host::spaces_api::SpacesApiClient> {
        match &self.spaces_api {
            Some(u) => cua_host::spaces_api::SpacesApiClient::new(u),
            None => cua_host::spaces_api::SpacesApiClient::current(),
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

    /// Which account is signed in now (the access token's `sub`), when the
    /// session says. A long-running node compares it with the account it
    /// started for.
    pub async fn account_subject(&self) -> Option<String> {
        let token = self.tokens.access_token().await.ok()?;
        cua_host::token_subject(&token)
    }

    /// A directory client carrying the device session.
    pub async fn client(&self) -> cua_host::Result<cua_host::RelayClient> {
        Ok(cua_host::RelayClient::new(&self.url)?.with_device_session(self.device_session().await))
    }
}

/// Another account is signed in now than the one state was started for
/// (`started`; each as [`RelayAccount::account_subject`]). Unknown on either
/// side is never a change: only a known switch restarts anything.
pub fn account_changed(started: &Option<String>, now: &Option<String>) -> bool {
    matches!((started, now), (Some(a), Some(b)) if a != b)
}

/// The gateway bearer of `relay:<id>` connections: the account token in
/// `authorization` plus the device session header, both fetched per call.
pub(crate) struct RelayBearer {
    pub(crate) account: RelayAccount,
}

/// How long a computer the relay lists as connected has to answer before
/// removing it from the account counts it as a stale entry.
const LIVE_PROBE: std::time::Duration = std::time::Duration::from_secs(6);

/// How long deleting one child Space through its computer may take when a
/// computer is removed with its Spaces.
const CHILD_DELETE: std::time::Duration = std::time::Duration::from_secs(20);

/// How long the account's default Space is kept before the relay is asked
/// again (lists mark it from this).
const DEFAULT_TTL: std::time::Duration = std::time::Duration::from_secs(30);
/// The most a list waits for the default (never holds a list up).
const DEFAULT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

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

/// Makes `name` the name of relay computer `machine_id` in every copy this
/// device keeps, after the relay took it (`PATCH /v1/machines/{id}`): the
/// host setup's own name when it is this computer, the saved directory
/// listing (read first when the app starts), the `relay:<id>` record in the
/// registry, and the host entries named like the computer was, which the
/// "Run on" list matches by name. The computer is found by its id
/// everywhere, so a rename never makes it a different or an offline one.
/// Best effort: a copy that cannot be written is logged.
pub fn adopt_machine_name(home: &std::path::Path, relay_url: &str, machine_id: &str, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    if let Err(e) = cua_host::Host::new(home).adopt_name(machine_id, name) {
        tracing::debug!(error = %e, "host name not updated");
    }
    let mut old: Vec<String> = Vec::new();
    let mut saved = crate::listing::load_directory(home, relay_url);
    let mut changed = false;
    for m in saved.iter_mut().filter(|m| m.id == machine_id) {
        if m.name != name {
            old.push(std::mem::replace(&mut m.name, name.to_string()));
            changed = true;
        }
    }
    if changed {
        crate::listing::save_directory(home, relay_url, &saved);
    }
    let registry = crate::registry::Registry::new(home);
    let space = format!("relay:{machine_id}");
    match registry.get(&space) {
        Ok(Some(mut record)) => {
            if record.name != name {
                old.push(record.name.clone());
                record.name = name.to_string();
                let credential = registry
                    .credential(&space)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if let Err(e) = registry.upsert(record, credential) {
                    tracing::debug!(error = %e, "{space} record not renamed");
                }
            }
        }
        Ok(None) => {}
        Err(e) => tracing::debug!(error = %e, "{space} record not read"),
    }
    if !old.is_empty()
        && let Err(e) = registry.update_direct_hosts(|hosts| {
            for h in hosts.iter_mut().filter(|h| {
                !h.space.starts_with("relay:")
                    && old.iter().any(|o| o.eq_ignore_ascii_case(&h.name))
            }) {
                h.name = name.to_string();
            }
        })
    {
        tracing::debug!(error = %e, "host entries not renamed");
    }
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
        delete_spaces: bool,
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
        // host keeps providing it, or it is a stale entry): it goes. A Space
        // in your cloud that is connected now does not (the cloud ends it).
        // A computer the relay lists as connected goes only when it does not
        // answer: the listing can outlive a computer that is gone (a relay
        // that handed over, a crash behind a proxy), and such a row would
        // otherwise be a ghost nobody can remove. One that answers is
        // connected for real and is taken off from that computer.
        let provided = m.host.as_deref().is_some_and(|h| !h.is_empty());
        if m.online && !provided && !is_registered_space(&m) {
            let name = if m.name.is_empty() { &m.id } else { &m.name };
            if crate::cloud::is_cloud_machine(&m) {
                return Err(Error::invalid(format!(
                    "{id} ({name}) is a running Space; delete it with `cua spaces delete {id}`"
                )));
            }
            if self.answers_through_relay(machine_id).await {
                return Err(Error::invalid(format!(
                    "{id} ({name}) is one of your computers and is connected now; take it off \
                     the relay with `cua host remove` on that computer"
                )));
            }
            tracing::info!(machine = %machine_id, "listed connected but not answering: removing the stale entry");
        }
        #[cfg(feature = "spaces-agents")]
        self.detach_attached_driver(machine_id).await;
        match client
            .delete_machine(&token, machine_id, delete_spaces)
            .await
        {
            Ok(()) | Err(cua_host::Error::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        self.forget_relay_locally(machine_id).await;
        Ok(Some(m))
    }

    /// Removes one of your computers from the relay. With `delete_spaces`
    /// the Spaces it provides are deleted first, files included, through the
    /// computer while it answers (best effort), and then their records go
    /// with it; without, they stay on the account, detached. Returns false
    /// when it was not on the relay.
    ///
    /// Kept apart from [`crate::Spaces::relay_unregister_computer`] (which
    /// only changes what the relay forgets) because deleting a Space can
    /// itself unregister a computer.
    pub async fn remove_computer(&self, space: &str, delete_spaces: bool) -> crate::Result<bool> {
        if delete_spaces && let Ok(crate::SpaceId::Relay { machine_id }) = self.resolve(space) {
            self.delete_children_through_computer(&machine_id).await;
        }
        self.relay_unregister_computer(space, delete_spaces).await
    }

    /// Deletes the Spaces `machine_id` provides, files included, through
    /// the computer while it answers. Best effort: one that cannot be
    /// reached is logged, and the relay forgets its record either way
    /// (`spaces=delete`).
    pub(crate) async fn delete_children_through_computer(&self, machine_id: &str) {
        let children: Vec<String> = self
            .inner
            .relay_cache
            .lock()
            .expect("relay cache")
            .iter()
            .filter(|m| m.host.as_deref() == Some(machine_id))
            .map(|m| format!("relay:{}", m.id))
            .collect();
        for child in children {
            let done = tokio::time::timeout(CHILD_DELETE, self.delete(&child)).await;
            match done {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    tracing::warn!(space = %child, error = %e, "child Space not deleted on its computer")
                }
                Err(_) => tracing::warn!(space = %child, "child Space delete timed out"),
            }
        }
    }

    /// [`adopt_machine_name`] for this runtime: its cached directory too.
    pub(crate) fn adopt_relay_machine_name(&self, machine_id: &str, name: &str) {
        if let Some(account) = self.relay_account() {
            adopt_machine_name(self.home_dir(), &account.url, machine_id, name);
        }
        for m in self
            .inner
            .relay_cache
            .lock()
            .expect("relay cache")
            .iter_mut()
            .filter(|m| m.id == machine_id)
        {
            m.name = name.trim().to_string();
        }
    }

    /// Whether `relay:<machine_id>`, which the relay lists as connected,
    /// answers a fresh connection within [`LIVE_PROBE`]. A computer that
    /// refuses because its owner stopped sharing it is alive (that is how
    /// it refused).
    pub(crate) async fn answers_through_relay(&self, machine_id: &str) -> bool {
        let id = format!("relay:{machine_id}");
        // Never an old connection: that is what may be stale.
        let _ = self.forget_connection(&id).await;
        match tokio::time::timeout(LIVE_PROBE, self.space(&id)).await {
            Ok(Ok(_)) => true,
            Ok(Err(e)) => crate::host_spaces::is_stopped_sharing(&e.to_string()),
            Err(_) => false,
        }
    }

    async fn relay_client_and_token(&self) -> crate::Result<(cua_host::RelayClient, String)> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        Ok((relay.client().await?, token))
    }

    /// The account's default Space settings, from the relay.
    pub async fn spaces_settings(&self) -> crate::Result<cua_host::SpacesSettings> {
        let (client, token) = self.relay_client_and_token().await?;
        Ok(client.spaces_settings(&token).await?)
    }

    /// Replaces the account's default Space settings (the whole object).
    pub async fn set_spaces_settings(
        &self,
        settings: &cua_host::SpacesSettings,
    ) -> crate::Result<cua_host::SpacesSettings> {
        let (client, token) = self.relay_client_and_token().await?;
        let saved = client.set_spaces_settings(&token, settings).await?;
        *self.inner.default_cache.lock().expect("default cache") = Some((
            std::time::Instant::now(),
            saved.default_space.clone().filter(|d| !d.is_empty()),
        ));
        Ok(saved)
    }

    /// Where "my default Space" resolves now. `here` is this computer's
    /// relay machine id (filled from this install's host setup when not
    /// given).
    pub async fn resolve_default_space(
        &self,
        here: Option<&str>,
    ) -> crate::Result<cua_host::DefaultSpace> {
        let (client, token) = self.relay_client_and_token().await?;
        let own = self.this_relay_machine_id();
        let here = here.map(str::to_string).or(own);
        Ok(client
            .resolve_default_space(&token, here.as_deref())
            .await?)
    }

    /// The default Space's id, for marking it in a list: from a short
    /// cache, else asked of the relay within [`DEFAULT_WAIT`]. Signed out,
    /// a relay that is down or too old, or a slow answer is `None` (and
    /// remembered for a moment): a list never waits on it or fails on it.
    pub async fn default_space_id(&self) -> Option<String> {
        if let Some((at, id)) = self
            .inner
            .default_cache
            .lock()
            .expect("default cache")
            .clone()
            && at.elapsed() < DEFAULT_TTL
        {
            return id;
        }
        let id = match self.relay_account() {
            None => None,
            Some(_) => tokio::time::timeout(DEFAULT_WAIT, self.spaces_settings())
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(|s| s.default_space)
                .filter(|d| !d.is_empty()),
        };
        *self.inner.default_cache.lock().expect("default cache") =
            Some((std::time::Instant::now(), id.clone()));
        id
    }

    /// Forgets `relay:<machine_id>` on this device: the cached row, any
    /// registry entry, its thumbnail and connection, and (with sharing) the
    /// Space attached as it.
    async fn forget_relay_locally(&self, machine_id: &str) {
        let id = crate::SpaceId::Relay {
            machine_id: machine_id.to_string(),
        };
        self.drop_connection(&id).await;
        self.relay_forget(machine_id);
        let _ = self.inner.registry.remove(&id.to_string());
        self.inner.thumbnails.remove(&id.to_string());
        #[cfg(feature = "spaces-agents")]
        self.forget_attached_machine(machine_id);
    }
}

#[cfg(test)]
mod account_tests {
    use super::*;
    use base64::Engine as _;

    fn token(sub: &str) -> String {
        let b = |v: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        format!("{}.{}.sig", b("{}"), b(&format!(r#"{{"sub":"{sub}"}}"#)))
    }

    #[tokio::test]
    async fn the_signed_in_account_is_the_tokens_subject() {
        let a = RelayAccount::new(
            "http://relay",
            Arc::new(cua_host::StaticToken(token("user-a"))),
        );
        assert_eq!(a.account_subject().await.as_deref(), Some("user-a"));
        // Not a JWT, or not signed in: unknown, never a different account.
        let opaque = RelayAccount::new("http://relay", Arc::new(cua_host::StaticToken("x".into())));
        assert_eq!(opaque.account_subject().await, None);
        let out = RelayAccount::new(
            "http://relay",
            Arc::new(cua_host::StaticToken(String::new())),
        );
        assert_eq!(out.account_subject().await, None);
    }

    #[test]
    fn only_a_known_switch_of_account_counts() {
        let s = |v: &str| Some(v.to_string());
        assert!(account_changed(&s("a"), &s("b")));
        assert!(!account_changed(&s("a"), &s("a")));
        assert!(!account_changed(&None, &s("b")));
        assert!(!account_changed(&s("a"), &None));
    }
}
