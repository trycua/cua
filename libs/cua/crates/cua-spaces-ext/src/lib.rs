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
//!   computer access), with the agent bridge; agents as members of a
//!   Space and each Space's thread ([`thread`]).
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
pub mod durable_member;
pub mod hooks;
pub mod named_volumes;
pub mod persistent;
mod persistent_tools;
pub mod presence_datagrams;
pub mod session_journal;
pub mod stream;
pub mod teleport;
pub mod teleport_app;
pub mod thread;
mod thread_tools;

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
    register_budget_alerts();
    builder
        .extension(Arc::new(DriveExtension::new(drive)))
        .extension(Arc::new(TeleportExtension::new(sessions)))
}

/// A cloud's budget alerts (50, 80 and 100% of its monthly budget) go to
/// the notifications feed the apps show, as kind `budget`.
pub fn register_budget_alerts() {
    cua_sandbox_core::byoc::set_budget_alert_sink(std::sync::Arc::new(|home, alert| {
        let feed = persistent::notify::Feed::new(&home.join(persistent::DIR));
        if let Err(e) = feed.post(
            None,
            BUDGET_KIND,
            &alert.title_line(),
            &alert.body_line(),
            None,
            None,
        ) {
            tracing::warn!(cloud = %alert.provider, "budget alert not posted: {e}");
        }
    }));
}

/// The notification kind of a cloud budget alert.
pub const BUDGET_KIND: &str = "budget";

// ------------------------------------------------------------ Cua Volume

/// The Cua Volume tools, its runtime (storage, this machine's mount, sync,
/// the cache, the volume in each Space) and persistent agents.
pub struct DriveExtension {
    drive: cua_volume::Drive,
    live: Arc<tokio::sync::Mutex<persistent::Live>>,
    /// What working sessions have said so far this turn (the thread's streaming rows).
    partials: persistent::Partials,
    owner: Arc<std::sync::RwLock<thread::Author>>,
    team_admins: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    pub(crate) runtime: drive_runtime::DriveRuntime,
}

impl DriveExtension {
    /// Serves `drive`.
    pub fn new(drive: cua_volume::Drive) -> Self {
        DriveExtension {
            drive,
            live: Default::default(),
            partials: Default::default(),
            owner: Arc::new(std::sync::RwLock::new(thread::Author::owner())),
            team_admins: Default::default(),
            runtime: Default::default(),
        }
    }

    /// The identity this machine's own user posts under in Space threads
    /// (default: `owner`, "Owner").
    pub fn with_owner(self, owner: thread::Author) -> Self {
        if let Ok(mut o) = self.owner.write() {
            *o = owner;
        }
        self
    }

    pub(crate) fn owner(&self) -> Arc<std::sync::RwLock<thread::Author>> {
        self.owner.clone()
    }

    pub(crate) fn team_admins(&self) -> Arc<std::sync::RwLock<std::collections::HashSet<String>>> {
        self.team_admins.clone()
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

    pub(crate) fn partials(&self) -> persistent::Partials {
        self.partials.clone()
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
            || thread_tools::TOOLS.contains(&tool)
            || tool == cua_spaces::agents::MEMBER_OP
            || named_volumes::OPS.contains(&tool)
            || tool == "agent_start.home"
            || tool == cua_spaces::agents::WATCH_OP
            || tool == cua_spaces::extension::MEMBER_AUTH_OP
    }

    fn call_tool<'a>(&'a self, call: ToolCall<'a>) -> BoxFuture<'a, Result<ToolOutcome>> {
        Box::pin(async move {
            let ToolCall {
                spaces,
                tool,
                args,
                pinned,
                caller,
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
            if tool == cua_spaces::extension::MEMBER_AUTH_OP {
                let cred: persistent::join::MemberCredential = crate::args(tool, args)?;
                let caller = spaces.persistent().member_auth(&cred).await?;
                let cua_spaces::extension::Caller::Member { handle, space } = caller else {
                    return Err(Error::invalid("not a member"));
                };
                return Ok(ToolOutcome::json(
                    &json!({"handle": handle, "space": space}),
                ));
            }
            if tool == cua_spaces::agents::WATCH_OP {
                let field = |k: &str| {
                    args.get(k)
                        .and_then(Value::as_str)
                        .ok_or_else(|| Error::invalid(format!("{tool}: `{k}` is required")))
                };
                spaces.persistent().watch_run(
                    field("space")?,
                    field("run_id")?,
                    field("agent")?,
                )?;
                return Ok(ToolOutcome::json(&json!({"watched": true})));
            }
            if named_volumes::OPS.contains(&tool) {
                return named_volumes::call(spaces, tool, args).await;
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
            if thread_tools::TOOLS.contains(&tool) || tool == cua_spaces::agents::MEMBER_OP {
                return thread_tools::call(spaces, &caller, tool, args).await;
            }
            persistent_tools::call(spaces, &caller, tool, args).await
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
