// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Spaces daemon extension: what `cua daemon` hosts in the Cua
//! Spaces build, registered through [`cua_daemon::extension`].
//!
//! - The Cua Volume (local, S3-compatible or the Cua cloud) behind the drive
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

    fn configure(&self, builder: SpacesBuilder, cx: &ExtensionContext<'_>) -> SpacesBuilder {
        // The drive asks for the same user presence the Keyvault does
        // (Touch ID, the login password or the vault passphrase) before it
        // widens anyone's access.
        let drive_presence: Arc<dyn cua_volume::Presence> =
            Arc::new(drive::KeyvaultPresence(self.presence()));
        let drive = drive::open(cx.home, cx.fleet, drive_presence);
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
        && cfg!(target_os = "macos")
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
        #[cfg(unix)]
        if let (Some(kv), Some(dir)) = (self.keyvault.as_ref(), socket_dir) {
            let path = dir.join("keyvault.sock");
            tasks.push(tokio::spawn(keyvault::serve_socket(kv.broker(), path)));
        }
        #[cfg(not(unix))]
        let _ = socket_dir;
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
