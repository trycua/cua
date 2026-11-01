// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Spaces daemon extension: what `cua daemon` hosts in the Cua
//! Spaces build, registered through [`cua_daemon::extension`].
//!
//! - The Cua Volume (local or S3-compatible) behind the drive
//!   tools and persistent agents ([`crate::DriveExtension`]), asking for the
//!   same user presence as the Keyvault before it widens access.
//! - Teleport ([`crate::TeleportExtension`]) over the host's app-session
//!   providers (a fixture home when the runtime names one).
//! - The Keyvault broker ([`keyvault::Keyvault`]) in the daemon's own
//!   process: it owns teleport delivery, mediates the `teleport_app` and
//!   `request_site_login` tools and serves `keyvault.sock` beside `cua.sock`.
//! - Sharing confirmation by user presence.
//! - The persistent-agent supervisor.
//!
//! The security properties are the Keyvault's own and do not depend on this
//! wiring: presence and deny-by-default, the audit log, grants bound to a
//! target, item, action and TTL.

use std::any::Any;
use std::path::Path;
use std::sync::Arc;

use cua_daemon::extension::{AttachedExtension, DaemonExtension, ExtensionContext};
use cua_spaces::{Spaces, SpacesBuilder};

pub mod drive;
pub mod keyvault;
pub mod site_login;

/// The Cua Spaces daemon extension. [`CuaSpacesDaemon::default`] is what
/// ships; [`CuaSpacesDaemon::with_test_presence`] is for tests and fixtures.
#[derive(Default, Clone)]
pub struct CuaSpacesDaemon {
    test_presence: Option<Arc<dyn cua_keyvault::UserPresence>>,
    test_host: Option<Arc<cua_teleport::FakeHost>>,
}

impl CuaSpacesDaemon {
    /// Tests and fixtures only (never shipped): the Keyvault uses `presence`
    /// in place of Touch ID and never the OS key store, so the vault under
    /// the runtime's cua home is passphrase-only with a file generation
    /// anchor and nothing reaches the login keychain. Every other Keyvault
    /// rule (first-party callers, consent, grants) is unchanged.
    pub fn with_test_presence(presence: Arc<dyn cua_keyvault::UserPresence>) -> Self {
        CuaSpacesDaemon {
            test_presence: Some(presence),
            test_host: None,
        }
    }

    /// Tests only: `host` stands in for the whole machine (the teleport
    /// providers and the Keyvault's saved-password reader act on it), in
    /// place of the one built from `RuntimeConfig::teleport_home`.
    pub fn with_test_host(mut self, host: Arc<cua_teleport::FakeHost>) -> Self {
        self.test_host = Some(host);
        self
    }

    fn presence(&self) -> Arc<dyn cua_keyvault::UserPresence> {
        self.test_presence.clone().unwrap_or_else(|| {
            Arc::new(keyvault::OsPresence) as Arc<dyn cua_keyvault::UserPresence>
        })
    }

    fn fake_host(&self, cx: &ExtensionContext<'_>) -> Option<Arc<cua_teleport::FakeHost>> {
        // A fixture home stands in for the whole host: the teleport
        // providers and the Keyvault's saved-password reader both act on
        // it, never on the real machine.
        self.test_host.clone().or_else(|| {
            cx.teleport_home
                .map(|home| Arc::new(cua_teleport::FakeHost::new().with_home(home.to_path_buf())))
        })
    }
}

/// Set to `0` to keep the drive's services (change feed, mount) from
/// starting with the daemon (they still start on the first drive runtime
/// tool call).
pub const ENV_DAEMON_DRIVE: &str = "CUA_DAEMON_DRIVE";

/// Set to `0` to keep this machine's user as `owner` ("Owner") in Space
/// threads instead of the signed-in account.
pub const ENV_DAEMON_OWNER_IDENTITY: &str = "CUA_DAEMON_OWNER_IDENTITY";

/// How often the signed-in account is read again (a sign-in or sign-out
/// shows in the thread within this).
const OWNER_REFRESH: std::time::Duration = std::time::Duration::from_secs(600);

fn owner_identity_enabled() -> bool {
    std::env::var(ENV_DAEMON_OWNER_IDENTITY).as_deref() != Ok("0")
        // A test process never reads the OS credential store here.
        && !cua_home::is_test_process()
        && !keyvault::test_identity_active()
}

/// The signed-in account as a thread author: `Ok(None)` when nobody is
/// signed in, `Err` when the store can't be read now (keep what we had).
fn signed_in_owner() -> Result<Option<crate::thread::Author>, String> {
    let creds = cua_auth::Store::from_env()
        .load()
        .map_err(|e| e.to_string())?;
    Ok(creds.and_then(|c| owner_author(&c.identity())))
}

/// `identity` as a thread author (its `sub` is the relay account id).
pub fn owner_author(identity: &cua_auth::Identity) -> Option<crate::thread::Author> {
    let id = identity.subject.clone().filter(|s| !s.trim().is_empty())?;
    let name = identity
        .name
        .clone()
        .or_else(|| identity.username.clone())
        .or_else(|| {
            identity
                .email
                .as_deref()
                .and_then(|e| e.split('@').next())
                .map(str::to_string)
        })
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Owner".into());
    Some(crate::thread::Author {
        kind: crate::thread::AuthorKind::Person,
        id,
        name,
        identity: None,
    })
}

/// The accounts that are owners or admins of a team the signed-in account
/// is in. `None` when they can't be read now (keep what we had); empty when
/// nobody is signed in.
async fn team_admins(spaces: &Spaces) -> Option<std::collections::HashSet<String>> {
    let Some(account) = spaces.relay_account() else {
        return Some(Default::default());
    };
    if !account.tokens.signed_in().await {
        return Some(Default::default());
    }
    let token = account.tokens.access_token().await.ok()?;
    let api = account.spaces_api().ok()?;
    match api.teams(&token).await {
        Ok(snapshot) => Some(admins_of(&snapshot)),
        Err(e) => {
            tracing::debug!(error = %e, "threads: team admins could not be read");
            None
        }
    }
}

/// Owners and admins of every team in `snapshot`.
pub fn admins_of(
    snapshot: &cua_host::spaces_api::TeamsSnapshot,
) -> std::collections::HashSet<String> {
    snapshot
        .teams
        .iter()
        .flat_map(|t| t.members.iter())
        .filter(|m| matches!(m.role.as_str(), "owner" | "admin") && !m.sub.is_empty())
        .map(|m| m.sub.clone())
        .collect()
}

async fn refresh_team_admins(spaces: Spaces) {
    use crate::SpacesPersistent as _;
    loop {
        if let Some(admins) = team_admins(&spaces).await {
            spaces.persistent().set_team_admins(admins);
        }
        tokio::time::sleep(OWNER_REFRESH).await;
    }
}

async fn refresh_owner(owner: Arc<std::sync::RwLock<crate::thread::Author>>, spaces: Spaces) {
    use crate::SpacesPersistent as _;
    loop {
        match tokio::task::spawn_blocking(signed_in_owner).await {
            Ok(Ok(found)) => {
                let next = found.unwrap_or_else(crate::thread::Author::owner);
                let mut before = None;
                if let Ok(mut o) = owner.write()
                    && *o != next
                {
                    tracing::info!(account = %next.id, "threads: this machine's user posts as {}", next.name);
                    before = Some(std::mem::replace(&mut *o, next));
                }
                // Signed out (or another account signed in): the agents the
                // last account connected from here stop at once.
                if let Some(prev) = before.filter(|p| p.id != crate::thread::Author::owner().id) {
                    match spaces.persistent().drop_members_of(&prev).await {
                        Ok(gone) if !gone.is_empty() => {
                            tracing::info!(removed = ?gone, "threads: removed the agents {} connected", prev.name)
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "threads: connected agents not removed")
                        }
                    }
                }
            }
            Ok(Err(e)) => tracing::debug!(error = %e, "threads: the sign-in could not be read"),
            Err(_) => {}
        }
        tokio::time::sleep(OWNER_REFRESH).await;
    }
}

/// Registers the Cua Spaces daemon extension for every runtime this process
/// builds (the Cua Spaces `cua`, the Spaces apps' embedded runtimes).
pub fn register() {
    crate::stream::register();
    crate::presence_datagrams::register();
    cua_daemon::extension::register(Arc::new(CuaSpacesDaemon::default()));
}

impl DaemonExtension for CuaSpacesDaemon {
    fn name(&self) -> &str {
        "cua-spaces"
    }

    fn features(&self) -> Vec<String> {
        // Space threads and agent members (`thread_*`, `agent_add`, ...).
        vec![crate::thread::FEATURE.into()]
    }

    fn configure(&self, builder: SpacesBuilder, cx: &ExtensionContext<'_>) -> SpacesBuilder {
        // The drive asks for the same user presence the Keyvault does
        // (Touch ID, the login password or the vault passphrase) before it
        // widens anyone's access.
        let drive_presence: Arc<dyn cua_volume::Presence> =
            Arc::new(drive::KeyvaultPresence(self.presence()));
        let drive = drive::open(cx.home, drive_presence);
        let sessions = Arc::new(match self.fake_host(cx) {
            Some(h) => crate::teleport::AppSessions::with_host(h),
            None => crate::teleport::AppSessions::builtin(),
        });
        // Sharing a Space asks for the same user presence the Keyvault does.
        let builder = builder.share_consent(Arc::new(keyvault::PresenceConsent(self.presence())));
        builder
            .extension(Arc::new(
                crate::DriveExtension::new(drive).with_keys(drive::keys()),
            ))
            .extension(Arc::new(crate::TeleportExtension::new(Some(sessions))))
    }

    fn attach(
        &self,
        spaces: &Spaces,
        cx: &ExtensionContext<'_>,
    ) -> Option<Arc<dyn AttachedExtension>> {
        let sessions = spaces
            .extension::<crate::TeleportExtension>()
            .and_then(crate::TeleportExtension::sessions)?;
        let keyvault = build_keyvault(
            cx.home_explicit.then_some(cx.home),
            spaces.clone(),
            sessions,
            self.fake_host(cx)
                .map(|h| h as Arc<dyn cua_teleport::HostEffects>),
            self.test_presence.clone(),
        );
        // `request_site_login` signs in through this Keyvault.
        spaces.set_site_login_broker(keyvault.as_ref().map(|k| k.site_login_broker()));
        Some(Arc::new(Attached { keyvault }))
    }
}

/// Builds the daemon's Keyvault broker over `<cua home>/keyvault`. It opens
/// (does not unlock) any existing vault.
fn build_keyvault(
    spaces_home: Option<&Path>,
    spaces: Spaces,
    sessions: Arc<crate::teleport::AppSessions>,
    host: Option<Arc<dyn cua_teleport::HostEffects>>,
    test_presence: Option<Arc<dyn cua_keyvault::UserPresence>>,
) -> Option<keyvault::Keyvault> {
    let dir = spaces_home
        .map(|h| h.join("keyvault"))
        .or_else(cua_keyvault::default_dir)?;
    let mut backend = keyvault::DaemonBackend::new(spaces, sessions);
    if let Some(h) = host {
        backend = backend.with_host(h);
    }
    let backend = Arc::new(backend);
    // A test gate never pairs with the OS key store: the vault is
    // passphrase-only with a file generation anchor.
    let os_protector = test_presence.is_none()
        // A debug daemon serving a test identity never touches the login
        // keychain (passphrase-only vault, file generation anchor).
        && cfg!(any(target_os = "macos", target_os = "windows", target_os = "linux"))
        && !keyvault::test_identity_active();
    let presence = test_presence
        .unwrap_or_else(|| Arc::new(keyvault::OsPresence) as Arc<dyn cua_keyvault::UserPresence>);
    match keyvault::Keyvault::new(dir, backend, presence, os_protector) {
        Ok(kv) => Some(kv),
        Err(e) => {
            tracing::warn!(error = %e, "keyvault: broker not started");
            None
        }
    }
}

/// The live part: the Keyvault broker (when a vault directory resolved).
pub struct Attached {
    keyvault: Option<keyvault::Keyvault>,
}

impl Attached {
    /// The daemon's Keyvault, when a vault directory resolved. It owns
    /// teleport delivery and the MCP teleport seam.
    pub fn keyvault(&self) -> Option<&keyvault::Keyvault> {
        self.keyvault.as_ref()
    }
}

impl AttachedExtension for Attached {
    fn session_broker(&self) -> Option<Arc<dyn cua_spaces::teleport_broker::SessionBroker>> {
        self.keyvault.as_ref().map(|kv| kv.session_broker())
    }

    fn daemon_tasks(
        &self,
        runtime: &cua_daemon::Runtime,
        socket_dir: Option<&Path>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        use crate::SpacesPersistent as _;
        let mut tasks = Vec::new();
        // The persistent-agent supervisor: saves agent homes after each
        // turn, answers agents' bridges, posts notifications and fires
        // routines, with or without an app open. One process per cua home
        // supervises (a file lock), so an embedded runtime next to this
        // daemon stays idle.
        if std::env::var(cua_daemon::server::ENV_DAEMON_PERSISTENT).as_deref() != Ok("0") {
            tasks.push(
                runtime
                    .spaces()
                    .spawn_supervisor(cua_daemon::server::PERSISTENT_TICK),
            );
        }
        // The Keyvault socket (`$CUA_HOME/keyvault.sock`, beside `cua.sock`)
        // for external first-party clients (the SDK and the CLI). The broker
        // also mediates the daemon-hosted teleport MCP tools in-process.
        // (On Windows `keyvault.sock` only names the pipe that serves it.)
        #[cfg(any(unix, windows))]
        if let (Some(kv), Some(dir)) = (self.keyvault.as_ref(), socket_dir) {
            let path = dir.join("keyvault.sock");
            tasks.push(tokio::spawn(keyvault::serve_socket(kv.broker(), path)));
        }
        #[cfg(not(any(unix, windows)))]
        let _ = socket_dir;
        // This machine's user posts in Space threads as the signed-in
        // account (its relay account id and display name), like everyone
        // who reaches this daemon through the relay.
        if let Some(owner) = runtime
            .spaces()
            .extension::<crate::DriveExtension>()
            .map(crate::DriveExtension::owner)
            .filter(|_| owner_identity_enabled())
        {
            tasks.push(tokio::spawn(refresh_owner(owner, runtime.spaces().clone())));
            // Admins of the owner's teams count as admins for agent rights.
            tasks.push(tokio::spawn(refresh_team_admins(runtime.spaces().clone())));
        }
        // The drive's change feed, and this machine's mount when the user
        // turned it on.
        if std::env::var(ENV_DAEMON_DRIVE).as_deref() != Ok("0") {
            let spaces = runtime.spaces().clone();
            tasks.push(tokio::spawn(async move {
                use crate::SpacesDrive as _;
                if let Err(e) = spaces.start_drive_service().await {
                    tracing::warn!(error = %e, "drive: services did not start");
                }
            }));
        }
        tasks
    }

    fn shutdown<'a>(
        &'a self,
        runtime: &'a cua_daemon::Runtime,
    ) -> cua_spaces::extension::BoxFuture<'a, ()> {
        // The volume must not outlive the daemon serving it: Spaces'
        // mounts end, pending uploads land, this machine's mount goes.
        Box::pin(async move {
            use crate::SpacesDrive as _;
            runtime.spaces().stop_drive_service().await;
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
