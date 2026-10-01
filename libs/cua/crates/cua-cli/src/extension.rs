//! The commands that ship with Cua Spaces: `cua teleport`, `cua keyvault`,
//! `cua volume config` and the Keyvault broker behind the MCP
//! `teleport_browser_session` tool.
//!
//! Their command line is defined in this crate (MIT); their implementation
//! (source-available, FSL-1.1-MIT) is in `cua-spaces-cli`, whose `cua`
//! registers a [`CliExtension`] and then runs [`crate::main`]. The Cua Spaces
//! apps ship that build. This build, alone, hands those commands to the Cua
//! Spaces `cua` when it finds one ([`spaces_cli`]): `CUA_SPACES_CLI`, the
//! Cua Spaces app bundle, or `cua-spaces-cli` on `PATH`. Otherwise they fail
//! with an error that says where they ship.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use cua_sdk::{Cua, CuaError, MediaEvent, MediaOpenOptions, MediaSession, SpacesdClient};

use crate::drive_cmd::ConfigCmd;
use crate::keyvault_cmd::KeyvaultCmd;
use crate::teleport::TeleportCmd;
use crate::teleport_session::Broker;

/// Set by the Cua Spaces `cua` on the commands it is handed, so a
/// misconfigured helper can never hand them back.
pub const ENV_FORWARDED: &str = "CUA_SPACES_CLI_FORWARDED";
/// An explicit path to the Cua Spaces `cua`.
pub const ENV_SPACES_CLI: &str = "CUA_SPACES_CLI";

/// What the Cua Spaces build of `cua` adds.
#[async_trait::async_trait(?Send)]
pub trait CliExtension: Send + Sync {
    /// `cua teleport <cmd>`. `cua` is open for `push` only.
    async fn teleport(
        &self,
        cua: Option<&Cua>,
        cmd: TeleportCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError>;

    /// `cua keyvault <cmd>`.
    async fn keyvault(
        &self,
        cmd: KeyvaultCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError>;

    /// `cua volume config <cmd>`.
    fn drive_config(
        &self,
        cmd: ConfigCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError>;

    /// The Keyvault broker the MCP `teleport_browser_session` tool files
    /// requests with.
    fn session_broker(&self) -> Option<Arc<dyn Broker>>;

    /// `cua viewer`: the standalone HTML5 viewer server.
    async fn viewer(
        &self,
        cua: Arc<Cua>,
        listen: &str,
        no_open: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError>;

    /// Opens a media session whose video `frames` receives decoded (packed
    /// BGRA) frames (`cua sb stream-probe`).
    async fn open_media_decoded(
        &self,
        env: Arc<SpacesdClient>,
        options: MediaOpenOptions,
        frames: Arc<dyn DecodedFrames>,
    ) -> Result<Arc<MediaSession>, CuaError>;
}

/// Receives decoded frames (see [`CliExtension::open_media_decoded`]).
pub trait DecodedFrames: Send + Sync {
    /// One frame: `width` x `height`, packed top-down BGRA.
    fn frame(&self, width: u32, height: u32, bgra: Vec<u8>);
    /// A control event.
    fn event(&self, event: MediaEvent);
}

static EXTENSION: OnceLock<Arc<dyn CliExtension>> = OnceLock::new();

/// Registers the Cua Spaces commands (once, before [`crate::main`]).
pub fn register(extension: Arc<dyn CliExtension>) {
    let _ = EXTENSION.set(extension);
}

/// The registered extension, if any.
pub fn get() -> Option<&'static Arc<dyn CliExtension>> {
    EXTENSION.get()
}

pub(crate) async fn teleport(
    cua: Option<&Cua>,
    cmd: TeleportCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match get() {
        Some(e) => e.teleport(cua, cmd, json, out).await,
        None => forward("teleport"),
    }
}

pub(crate) async fn keyvault(
    cmd: KeyvaultCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match get() {
        Some(e) => e.keyvault(cmd, json, out).await,
        None => forward("the Keyvault"),
    }
}

pub(crate) async fn viewer(
    cua: Arc<Cua>,
    listen: &str,
    no_open: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match get() {
        Some(e) => e.viewer(cua, listen, no_open, out).await,
        None => forward("the standalone viewer (`cua viewer`)"),
    }
}

pub(crate) fn drive_config(
    cmd: ConfigCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match get() {
        Some(e) => e.drive_config(cmd, json, out),
        None => forward("the Cua Volume's configuration"),
    }
}

/// Runs this process's command line in the Cua Spaces `cua` and returns its
/// exit code, or explains where `what` ships.
fn forward(what: &str) -> Result<i32, CuaError> {
    let missing = || {
        CuaError::Unsupported(format!(
            "{what} ships with Cua Spaces (source-available, FSL-1.1-MIT) and this `cua` \
             does not include it; install Cua Spaces (https://cua.ai/download) and run its \
             `cua`, or set {ENV_SPACES_CLI} to it"
        ))
    };
    if std::env::var_os(ENV_FORWARDED).is_some() {
        return Err(missing());
    }
    let Some(helper) = spaces_cli() else {
        return Err(missing());
    };
    let status = std::process::Command::new(&helper)
        .args(std::env::args_os().skip(1))
        .env(ENV_FORWARDED, "1")
        .status()
        .map_err(|e| CuaError::Internal(format!("{}: {e}", helper.display())))?;
    Ok(status.code().unwrap_or(1))
}

/// The Cua Spaces `cua`: `CUA_SPACES_CLI`, else the Cua Spaces app's bundled
/// `cua`, else `cua-spaces-cli` on `PATH`. Never this executable.
pub fn spaces_cli() -> Option<PathBuf> {
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let not_me = |p: PathBuf| -> Option<PathBuf> {
        let real = p.canonicalize().ok()?;
        (Some(&real) != me.as_ref() && real.is_file()).then_some(p)
    };
    if let Some(p) = std::env::var_os(ENV_SPACES_CLI).filter(|v| !v.is_empty()) {
        return not_me(PathBuf::from(p));
    }
    // Test processes only use an explicit `CUA_SPACES_CLI`, never an
    // installed app.
    if std::env::var_os("CUA_ENV_TEST_SANDBOX").is_some() {
        return None;
    }
    app_bundle_candidates()
        .into_iter()
        .find_map(not_me)
        .or_else(|| {
            on_path(if cfg!(windows) {
                "cua-spaces-cli.exe"
            } else {
                "cua-spaces-cli"
            })
            .and_then(not_me)
        })
}

fn app_bundle_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        out.push(PathBuf::from(
            "/Applications/Cua Spaces.app/Contents/MacOS/cua",
        ));
        if let Some(home) = std::env::var_os("HOME") {
            out.push(Path::new(&home).join("Applications/Cua Spaces.app/Contents/MacOS/cua"));
        }
    } else if cfg!(windows) {
        for var in ["LOCALAPPDATA", "ProgramFiles"] {
            if let Some(dir) = std::env::var_os(var) {
                out.push(Path::new(&dir).join("Cua Spaces").join("cua.exe"));
            }
        }
    }
    out
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// The extensions this process registered that the running daemon does not
/// report (`GetInfoResponse.features`, `extension:<name>`); empty when none
/// is missing or no daemon answers.
pub(crate) async fn missing_daemon_extensions() -> Vec<String> {
    let want: Vec<String> = cua_daemon::extension::registered()
        .iter()
        .map(|e| e.name().to_string())
        .collect();
    if want.is_empty() {
        return want;
    }
    let Ok(client) = cua_daemon::client::existing_daemon().await else {
        return Vec::new();
    };
    let Ok(info) = client.info().await else {
        return Vec::new();
    };
    want.into_iter()
        .filter(|name| {
            !info
                .features
                .iter()
                .any(|f| f.strip_prefix(cua_daemon::extension::FEATURE_PREFIX) == Some(name))
        })
        .collect()
}

/// Asks the running daemon to stop and waits (bounded) for it to go.
pub(crate) async fn stop_daemon(discovery: &Path, pid: u32) -> Result<(), CuaError> {
    if let Ok(client) = cua_daemon::client::existing_daemon().await {
        let _ = client.shutdown().await;
    }
    for _ in 0..100 {
        let gone = match cua_daemon::Discovery::read(discovery) {
            Some(d) => d.pid != pid,
            None => true,
        };
        if gone && cua_daemon::client::live_address().is_none() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(CuaError::Timeout(format!(
        "the running cua daemon (pid {pid}) did not stop within 10 s"
    )))
}
