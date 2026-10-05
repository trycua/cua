// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cua Spaces extensions (source-available, FSL-1.1-MIT): the Spaces
//! capabilities built on the Cua Volume, the teleport providers and the
//! Keyvault, registered into the MIT Spaces runtime through
//! [`cua_spaces::extension`].
//!
//! - [`TeleportExtension`]: session teleport (`teleport_manifest`,
//!   `teleport_app`) and the in-process export the SDK's embedded
//!   `Space.teleport` uses (`teleport.send`), over [`teleport::AppSessions`].
//! - [`DriveExtension`]: the Cua Volume tools (`drive_*`) and persistent
//!   agents (homes in the drive, routines, notifications, pause and resume,
//!   computer access), with the agent bridge.
//!
//! [`register`] adds both to a [`cua_spaces::SpacesBuilder`]; the Cua Spaces
//! daemon, the Spaces apps and their tests do exactly that.

use std::any::Any;
use std::sync::Arc;

use cua_spaces::extension::{BoxFuture, SpacesExtension, ToolCall};
use cua_spaces::mcp::ToolOutcome;
use cua_spaces::{Error, Result, SpacesBuilder};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub mod daemon;
pub mod drive_runtime;
pub mod drive_tools;
pub mod persistent;
mod persistent_tools;
pub mod presence_datagrams;
pub mod stream;
pub mod teleport;
pub mod teleport_app;

pub use drive_runtime::{SpacesDrive, VolumeInfo};
pub use persistent::SpacesPersistent;

/// The Cua Volume's error as a Spaces error, with the drive's stable kind.
pub fn drive_err(e: cua_volume::Error) -> Error {
    let kind = match &e {
        cua_volume::Error::Backend(_) => "volume_backend",
        e => e.tag(),
    };
    Error::extension(kind, e.to_string())
}

/// `?` for Cua Volume results inside Spaces code.
pub trait DriveResult<T> {
    /// The result with its error as a Spaces error ([`drive_err`]).
    fn drive(self) -> Result<T>;
}

impl<T> DriveResult<T> for cua_volume::Result<T> {
    fn drive(self) -> Result<T> {
        self.map_err(drive_err)
    }
}

pub(crate) fn args<T: DeserializeOwned>(tool: &str, value: Value) -> Result<T> {
    let value = if value.is_null() { json!({}) } else { value };
    serde_json::from_value(value)
        .map_err(|e| Error::invalid(format!("{tool}: {e} (see the tool's inputSchema)")))
}

/// Registers the Cua Spaces extensions on `builder`: the Cua Volume tools
/// and persistent agents over `drive`, and teleport over `sessions` (none:
/// the teleport tools report that no app-session providers are attached).
/// Also registers the streaming client ([`stream`]) and the presence
/// datagram channel ([`presence_datagrams`]) for this process.
pub fn register(
    builder: SpacesBuilder,
    drive: cua_volume::Drive,
    sessions: Option<Arc<teleport::AppSessions>>,
) -> SpacesBuilder {
    stream::register();
    presence_datagrams::register();
    builder
        .extension(Arc::new(DriveExtension::new(drive)))
        .extension(Arc::new(TeleportExtension::new(sessions)))
}

// ------------------------------------------------------------ Cua Volume

/// The Cua Volume tools, its runtime (storage, this machine's mount, sync,
/// the cache, the volume in each Space) and persistent agents.
pub struct DriveExtension {
    drive: cua_volume::Drive,
    live: Arc<tokio::sync::Mutex<persistent::Live>>,
    pub(crate) runtime: drive_runtime::DriveRuntime,
}

impl DriveExtension {
    /// Serves `drive`.
    pub fn new(drive: cua_volume::Drive) -> Self {
        DriveExtension {
            drive,
            live: Default::default(),
            runtime: Default::default(),
        }
    }

    /// Where the drive's S3 keys are kept (the daemon: the credential
    /// store). Default: the environment only, and saving keys is refused.
    pub fn with_keys(mut self, keys: Arc<dyn cua_volume::service::KeyStore>) -> Self {
        self.runtime.set_keys(keys);
        self
    }

    /// Whether a Space's guest mounts Cua Volume when it connects (default:
    /// yes, where the guest can; [`SpacesDrive::volume_attach`] still mounts
    /// on request).
    pub fn volume_auto(mut self, on: bool) -> Self {
        self.runtime.volume_auto = on;
        self
    }

    /// The drive this extension serves.
    pub fn drive(&self) -> &cua_volume::Drive {
        &self.drive
    }

    pub(crate) fn live(&self) -> Arc<tokio::sync::Mutex<persistent::Live>> {
        self.live.clone()
    }
}

impl SpacesExtension for DriveExtension {
    fn name(&self) -> &str {
        "drive"
    }

    fn serves(&self, tool: &str) -> bool {
        drive_tools::DRIVE_TOOLS.contains(&tool)
            || drive_tools::SERVICE_TOOLS.contains(&tool)
            || persistent_tools::TOOLS.contains(&tool)
            || tool == "agent_start.home"
    }

    fn call_tool<'a>(&'a self, call: ToolCall<'a>) -> BoxFuture<'a, Result<ToolOutcome>> {
        Box::pin(async move {
            let ToolCall {
                spaces,
                tool,
                args,
                pinned,
                ..
            } = call;
            if drive_tools::SERVICE_TOOLS.contains(&tool) {
                let ctx = drive_tools::host_context(&args)?;
                return Box::pin(drive_tools::service_tool(spaces, ctx, tool, args)).await;
            }
            if drive_tools::DRIVE_TOOLS.contains(&tool) {
                let ctx = drive_tools::host_context(&args)?;
                let feed = spaces.drive_feed();
                return drive_tools::drive_tool_synced(
                    &self.drive,
                    feed.as_deref(),
                    ctx,
                    tool,
                    args,
                )
                .await;
            }
            if tool == "agent_start.home" {
                let space = match pinned {
                    Some(s) => s.clone(),
                    None => {
                        let id = args
                            .get("space")
                            .and_then(Value::as_str)
                            .ok_or_else(|| Error::invalid("agent_start: `space` is required"))?;
                        spaces.space(id).await?
                    }
                };
                return persistent_tools::start_with_home(spaces, &space, args).await;
            }
            persistent_tools::call(spaces, tool, args).await
        })
    }

    fn space_connected(&self, spaces: &cua_spaces::Spaces, space: &str) {
        if self.runtime.volume_auto {
            drive_runtime::spawn_attach(spaces, space);
        }
    }

    fn space_dropped<'a>(
        &'a self,
        _spaces: &'a cua_spaces::Spaces,
        space: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.runtime.detach(space).await;
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ------------------------------------------------------------- teleport

/// Session teleport: `teleport_manifest`, `teleport_app` (Keyvault
/// mediated) and the in-process `teleport.send`.
pub struct TeleportExtension {
    sessions: Option<Arc<teleport::AppSessions>>,
}

impl TeleportExtension {
    /// Teleports with `sessions` (none: the tools report that no
    /// app-session providers are attached).
    pub fn new(sessions: Option<Arc<teleport::AppSessions>>) -> Self {
        TeleportExtension { sessions }
    }

    /// The app-session providers, when attached.
    pub fn sessions(&self) -> Option<Arc<teleport::AppSessions>> {
        self.sessions.clone()
    }

    fn require_sessions(&self) -> Result<Arc<teleport::AppSessions>> {
        self.sessions.clone().ok_or_else(|| {
            Error::host(
                cua_spaces_contract::host::APP_SESSIONS,
                "no app-session providers are attached to this Spaces runtime (the daemon attaches them)",
            )
        })
    }
}

impl SpacesExtension for TeleportExtension {
    fn name(&self) -> &str {
        "teleport"
    }

    fn serves(&self, tool: &str) -> bool {
        matches!(tool, "teleport_manifest" | "teleport_app" | "teleport.send")
    }

    fn call_tool<'a>(&'a self, call: ToolCall<'a>) -> BoxFuture<'a, Result<ToolOutcome>> {
        Box::pin(async move {
            let sessions = self.require_sessions()?;
            teleport::tool(sessions, call).await
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The app-session providers of a runtime that registered a
/// [`TeleportExtension`].
pub trait SpacesTeleport {
    /// The providers, when a [`TeleportExtension`] with providers is
    /// registered.
    fn app_sessions(&self) -> Option<Arc<teleport::AppSessions>>;
}

impl SpacesTeleport for cua_spaces::Spaces {
    fn app_sessions(&self) -> Option<Arc<teleport::AppSessions>> {
        self.extension::<TeleportExtension>()
            .and_then(TeleportExtension::sessions)
    }
}
