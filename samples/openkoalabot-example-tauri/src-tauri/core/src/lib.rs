// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! openkoalabots's app logic on the cua Spaces runtime (`cua-spaces`, the Rust
//! crate behind the cua SDK), headless so the Tauri shell, its tests and the
//! scenario runner share one implementation.
//!
//! The flows mirror the Swift app (`samples/openkoalabot-example-swift`):
//!
//! | Flow | Module |
//! |---|---|
//! | pick / add / create / delete a Space | [`Core`] |
//! | desktop stream (PiP): keyframe-first frames, or a media ticket for the webview | [`stream`] |
//! | one long-lived agent thread per Bot, roster, transcript | [`thread`] |
//! | drop a file into the Space, SHA-256 verified by the guest | [`files`] |
//! | teleport an app session behind an explicit approval (ships with Cua Spaces) | [`teleport`] |
//! | presence: who else is in the Space, cursors | [`presence`] |
//! | routines and group chats over the Bots' threads | [`bots`] |
//! | the New Space wizard: a plan to one `Spaces::create` call | [`plan`] |
//! | the page's saved state, in the app's data directory | [`ui_state`] |
//!
//! Host safety: [`CoreConfig`] always names the Spaces registry directory;
//! tests and the scenario runner pass temp directories.
//!
//! App teleport (the "Teleport an app..." picker and window drags) ships with
//! Cua Spaces (source-available) and is not part of this sample; session
//! teleport goes through the Spaces tools and says the same when the runtime
//! has no teleport extension (see [`teleport`]).

pub mod bots;
pub mod error;
pub mod files;
pub mod plan;
pub mod presence;
pub mod scenario;
pub mod stream;
pub mod teleport;
pub mod thread;
pub mod ui_state;

pub use error::{Error, Result};

use cua_sandbox_core::{LocalRuntime, Sandboxes};
pub use cua_spaces;
pub use cua_spaces::{Space, SpaceInfo};
use cua_spaces::{SpaceCreate, Spaces};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Where the core keeps its state.
#[derive(Clone)]
pub struct CoreConfig {
    /// The Spaces registry directory (`spaces.json` + stored tokens).
    pub spaces_home: PathBuf,
    /// Where `download` lands.
    pub download_dir: PathBuf,
    /// Connect to Cua Cloud with credentials from the environment
    /// (`CUA_CLIENT_ID`/`SECRET` or `FLEETS_TOKEN`), which enables
    /// [`Core::create_cloud_space`].
    pub cloud_from_env: bool,
    /// spacesd handshake timeout.
    pub probe_timeout: Duration,
    /// The runtime behind local Spaces (the Tauri shell passes the SDK's
    /// `VmmLocal`: containers, QEMU and Lume). `None`: local Spaces are
    /// unavailable.
    pub local_runtime: Option<Arc<dyn LocalRuntime>>,
    /// Where local sandboxes keep their state (never `~/.cua`).
    pub sandboxes_home: PathBuf,
}

impl std::fmt::Debug for CoreConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreConfig")
            .field("spaces_home", &self.spaces_home)
            .field("download_dir", &self.download_dir)
            .field("cloud_from_env", &self.cloud_from_env)
            .field(
                "local_runtime",
                &self.local_runtime.as_ref().map(|r| r.backend()),
            )
            .finish()
    }
}

impl CoreConfig {
    /// A config rooted at `dir` (registry in `dir/spaces`, downloads in
    /// `dir/downloads`), no cloud.
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            spaces_home: dir.join("spaces"),
            download_dir: dir.join("downloads"),
            cloud_from_env: false,
            probe_timeout: Duration::from_secs(20),
            local_runtime: None,
            sandboxes_home: dir.join("sandboxes"),
        }
    }

    /// Enable local Spaces on `runtime`.
    pub fn with_local_runtime(mut self, runtime: Arc<dyn LocalRuntime>) -> Self {
        self.local_runtime = Some(runtime);
        self
    }

    /// Enable Cua Cloud with credentials from the environment.
    pub fn with_cloud_from_env(mut self, on: bool) -> Self {
        self.cloud_from_env = on;
        self
    }
}

/// The app's handle on the Spaces runtime.
#[derive(Clone)]
pub struct Core {
    spaces: Spaces,
    config: CoreConfig,
}

impl Core {
    /// Builds the runtime. No I/O until the first call, except creating
    /// the registry directory.
    pub fn new(config: CoreConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.spaces_home)?;
        std::fs::create_dir_all(&config.download_dir)?;
        // The streaming client behind `StreamSession` (Cua Spaces).
        cua_spaces_ext::stream::register();
        // #region docs:rs-open
        let mut builder = Spaces::builder()
            .home(config.spaces_home.clone())
            .download_dir(config.download_dir.clone())
            // Never draw on the operator's desktop: the app renders streams
            // itself (PiP), from frames or a media ticket.
            .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
            .probe_timeout(config.probe_timeout);
        if config.cloud_from_env {
            // Cua Cloud (the SDK's cloud engine client).
            builder = builder.fleet(cua_fleet::FleetClient::from_env()?);
        }
        // #endregion docs:rs-open
        if let Some(local) = &config.local_runtime {
            // Local sandboxes, with their state in the app's directory.
            builder = builder.sandboxes(
                Sandboxes::builder()
                    .local(local.clone())
                    .state_dir(config.sandboxes_home.clone())
                    .build(),
            );
        }
        Ok(Self {
            spaces: builder.build(),
            config,
        })
    }

    /// The underlying runtime.
    pub fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    /// The configuration.
    pub fn config(&self) -> &CoreConfig {
        &self.config
    }

    /// Every registered Space (the picker's list).
    pub fn list_spaces(&self) -> Result<Vec<SpaceInfo>> {
        Ok(self.spaces.list()?)
    }

    /// Adds a machine that runs cua-spacesd (`host:port`, `http(s)://…`
    /// or a Space id such as `local:<name>`) after a capabilities handshake. Never creates a sandbox.
    // #region docs:rs-add
    pub async fn add_space(
        &self,
        url: &str,
        token: Option<String>,
        name: Option<String>,
    ) -> Result<SpaceInfo> {
        Ok(self.spaces.add(url, token, name).await?)
    }
    // #endregion docs:rs-add

    /// Creates a Space in the cloud (metered): the image's registry
    /// manifest decides the kind and engine.
    pub async fn create_cloud_space(
        &self,
        image: Option<String>,
        name: Option<String>,
    ) -> Result<SpaceInfo> {
        self.spaces
            .create(SpaceCreate {
                on: Some(cua_sandbox_core::placement::On::Cloud),
                image,
                name,
                wait: Some(true),
                ..Default::default()
            })
            .await?
            .ready()
            .map_err(|p| Error::Invalid(format!("Space {} is still {}", p.id, p.phase)))
    }

    /// Creates a Space from the New Space wizard's plan: one
    /// `Spaces::create` call, locally (free) or in the cloud (metered); see
    /// [`plan::plan_call`].
    pub async fn create_space(&self, plan: &plan::SpacePlan) -> Result<SpaceInfo> {
        let create = plan::plan_call(plan)?;
        if create.on == Some(cua_sandbox_core::placement::On::Local)
            && self.config.local_runtime.is_none()
        {
            return Err(Error::Invalid(
                "local Spaces need a local runtime (CoreConfig::with_local_runtime)".into(),
            ));
        }
        self.spaces
            .create(create)
            .await?
            .ready()
            .map_err(|p| Error::Invalid(format!("Space {} is still {}", p.id, p.phase)))
    }

    /// A connected handle.
    pub async fn space(&self, id: &str) -> Result<Space> {
        Ok(self.spaces.space(id).await?)
    }

    /// Deletes a Space's sandbox (a Space added by address is only
    /// forgotten).
    pub async fn delete_space(&self, id: &str) -> Result<String> {
        Ok(self.spaces.delete(id).await?)
    }

    /// Runs a shell line in the Space.
    pub async fn bash(&self, space: &Space, line: &str, timeout: Duration) -> Result<String> {
        let out = space.bash(line, timeout).await?;
        if !out.success() {
            return Err(Error::Invalid(format!("`{line}` failed: {}", out.render())));
        }
        Ok(out.stdout)
    }
}

/// 8 random hex chars.
pub fn nonce() -> String {
    format!("{:08x}", rand::random::<u32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_in_dir_never_points_at_the_real_home() {
        let c = CoreConfig::in_dir("/tmp/x");
        assert_eq!(c.spaces_home, PathBuf::from("/tmp/x/spaces"));
        assert!(!c.cloud_from_env);
    }

    #[test]
    fn nonce_is_eight_hex() {
        let n = nonce();
        assert_eq!(n.len(), 8);
        assert!(n.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
