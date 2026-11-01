// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cua Driver for the coding agents on this Mac, run by cua-spacesd as its
//! own child (macOS).
//!
//! The Cua Spaces app runs cua-spacesd from `Contents/Helpers`, and macOS
//! holds the app responsible for it. A `cua-driver serve --embedded` that
//! cua-spacesd spawns directly stays in that chain (embedded mode never
//! disclaims responsibility), so Accessibility and Screen Recording are Cua
//! Spaces' and there is no second app to allow. Coding agents reach it
//! through `cua-driver mcp --embedded --socket <socket> --host-bundle-id
//! <id>`, which Set up writes into their configs.
//!
//! [`EmbeddedDriver`] owns that child through the SDK's
//! [`EmbeddedCuaDriverHost`] (arguments, readiness, generations, shutdown
//! when cua-spacesd's end of its stdin closes), restarts it when it exits,
//! and is the one place the app asks for the driver's status, its settings
//! (`get_config` / `set_config`), its permissions and a live test.
//!
//! **Off by default** until the live check on a macOS VM passes (the
//! cursor draws, the parent is cua-spacesd, System Settings lists only Cua
//! Spaces): `CUA_SPACESD_EMBEDDED_DRIVER=1` turns it on.
//!
//! **Cursor overlay.** The child draws the agent cursor for coding agents
//! (the SDK default). cua-spacesd's own overlay keeps drawing remote people
//! and host-Space agents. Each cursor has one owner; whether two overlay
//! windows coexist cleanly is part of the VM check.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cua_driver_sdk::{
    CuaDriver, EmbeddedCuaDriverHost, EmbeddedDriverConnection, EmbeddedDriverHostOptions,
    EmbeddedDriverHostState,
};
use cua_home::channel::CuaChannel;
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Turns the embedded driver on (`1`).
pub const ENABLE_VAR: &str = "CUA_SPACESD_EMBEDDED_DRIVER";
/// A cua-driver to run instead of the bundled one (development).
pub const BINARY_VAR: &str = "CUA_SPACESD_DRIVER_BIN";

/// What to run, where it listens and whose permissions it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedDriverConfig {
    pub binary: PathBuf,
    pub socket: PathBuf,
    pub host_bundle_id: String,
}

impl EmbeddedDriverConfig {
    /// The configuration from the environment, or `None` (with the reason)
    /// when the embedded driver is off or there is no driver to run.
    pub fn from_env() -> Result<Self, String> {
        let get = |name: &str| std::env::var(name).ok();
        let exe = std::env::current_exe().map_err(|e| format!("this executable's path: {e}"))?;
        Self::resolve(
            get(ENABLE_VAR),
            get(BINARY_VAR),
            &exe,
            CuaChannel::current(),
        )
    }

    /// [`Self::from_env`] with its inputs explicit (testable).
    pub fn resolve(
        enable: Option<String>,
        binary_override: Option<String>,
        exe: &Path,
        channel: CuaChannel,
    ) -> Result<Self, String> {
        if enable.as_deref().map(str::trim) != Some("1") {
            return Err(format!("off ({ENABLE_VAR} is not 1)"));
        }
        let binary = match binary_override.filter(|v| !v.trim().is_empty()) {
            Some(path) => PathBuf::from(path),
            None => bundled_driver_beside(exe).ok_or_else(|| {
                format!(
                    "no cua-driver next to {} (Cua Spaces carries it in Contents/Helpers)",
                    exe.display()
                )
            })?,
        };
        Ok(Self {
            binary,
            socket: channel.embedded_driver_socket(),
            host_bundle_id: channel.bundle_id().to_string(),
        })
    }
}

/// The `cua-driver` in the same folder as `exe`: in the app, cua-spacesd
/// and cua-driver are both in `Contents/Helpers`.
pub fn bundled_driver_beside(exe: &Path) -> Option<PathBuf> {
    let candidate = exe.parent()?.join("cua-driver");
    is_executable(&candidate).then_some(candidate)
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// How long to wait before restart number `attempt` (1-based) after the
/// driver exited: 1 s, doubling, at most a minute. A driver that stayed up
/// for [`STABLE_UPTIME`] starts again from 1 s.
pub fn restart_delay(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.saturating_sub(1).min(6);
    Duration::from_secs(secs.min(60))
}

/// Uptime after which an exit counts as a fresh failure, not a crash loop.
pub const STABLE_UPTIME: Duration = Duration::from_secs(120);

/// The result of the last live test ([`EmbeddedDriver::live_test`]).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LiveTest {
    pub passed: bool,
    /// Seconds since the Unix epoch.
    pub at: u64,
    /// Which step failed and why, in plain words.
    pub failure: Option<String>,
}

/// What the app shows about the driver.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DriverStatus {
    /// `stopped`, `starting`, `ready` or `stopping`.
    pub state: &'static str,
    pub version: Option<String>,
    pub pid: Option<u32>,
    pub generation: Option<String>,
    pub binary: String,
    pub socket: String,
    pub host_bundle_id: String,
    /// Restarts after an unexpected exit since cua-spacesd started.
    pub restarts: u32,
    pub last_error: Option<String>,
    pub last_test: Option<LiveTest>,
}

#[derive(Default)]
struct Tracked {
    restarts: u32,
    last_error: Option<String>,
    last_test: Option<LiveTest>,
}

/// The driver cua-spacesd runs for coding agents.
pub struct EmbeddedDriver {
    config: EmbeddedDriverConfig,
    host: Arc<EmbeddedCuaDriverHost>,
    tracked: Mutex<Tracked>,
}

impl EmbeddedDriver {
    pub fn new(config: EmbeddedDriverConfig) -> Result<Arc<Self>, String> {
        let host = EmbeddedCuaDriverHost::with_options(EmbeddedDriverHostOptions {
            binary_path: config.binary.display().to_string(),
            host_bundle_id: config.host_bundle_id.clone(),
            socket_path: Some(config.socket.display().to_string()),
            startup_timeout_ms: None,
            shutdown_timeout_ms: None,
            permission_mode: None,
            capability_manifest_path: None,
            approve_capability_manifest: false,
            session_policy_path: None,
            approve_session_policy: false,
            dangerously_bypass_approvals: false,
            environment: Vec::new(),
            inherit_stderr: true,
            // The child draws coding agents' cursors (see the module docs).
            no_overlay: false,
        })
        .map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            config,
            host,
            tracked: Mutex::new(Tracked::default()),
        }))
    }

    /// Keeps the driver running until `shutdown`: starts it, waits for it
    /// to exit, restarts it after [`restart_delay`], and stops it (and
    /// removes its socket) on shutdown.
    pub async fn supervise(self: Arc<Self>, shutdown: CancellationToken) {
        if let Some(dir) = self.config.socket.parent() {
            if let Err(error) = create_private_dir(dir) {
                self.note_error(format!("can't create {}: {error}", dir.display()));
                return;
            }
        }
        let mut attempt = 0u32;
        loop {
            let started = std::time::Instant::now();
            let connection = tokio::select! {
                _ = shutdown.cancelled() => break,
                result = self.host.clone().start() => result,
            };
            match connection {
                Ok(connection) => {
                    tracing::info!(
                        pid = connection.pid,
                        version = %connection.driver_version,
                        socket = %connection.socket_path,
                        "Cua Driver for coding agents is running"
                    );
                    let exit = tokio::select! {
                        _ = shutdown.cancelled() => break,
                        exit = self.host.clone().wait_for_exit(connection.generation.clone()) => exit,
                    };
                    let reason = match exit {
                        Ok(exit) => format!("Cua Driver stopped (exit code {:?})", exit.code),
                        Err(error) => error.to_string(),
                    };
                    tracing::warn!(%reason, "Cua Driver for coding agents stopped; restarting");
                    self.note_error(reason);
                }
                Err(error) => {
                    tracing::warn!(%error, "Cua Driver for coding agents did not start");
                    self.note_error(error.to_string());
                }
            }
            if started.elapsed() >= STABLE_UPTIME {
                attempt = 0;
            }
            attempt += 1;
            self.tracked.lock().unwrap().restarts += 1;
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(restart_delay(attempt)) => {}
            }
        }
        if let Err(error) = self.host.clone().stop().await {
            tracing::warn!(%error, "stopping Cua Driver for coding agents");
        }
    }

    fn note_error(&self, error: String) {
        self.tracked.lock().unwrap().last_error = Some(error);
    }

    fn connection(&self) -> Option<EmbeddedDriverConnection> {
        self.host.connection()
    }

    pub fn status(&self) -> DriverStatus {
        let connection = self.connection();
        let tracked = self.tracked.lock().unwrap();
        DriverStatus {
            state: match self.host.state() {
                EmbeddedDriverHostState::Stopped => "stopped",
                EmbeddedDriverHostState::Starting => "starting",
                EmbeddedDriverHostState::Ready => "ready",
                EmbeddedDriverHostState::Stopping => "stopping",
            },
            version: connection.as_ref().map(|c| c.driver_version.clone()),
            pid: connection.as_ref().map(|c| c.pid),
            generation: connection.as_ref().map(|c| c.generation.clone()),
            binary: self.config.binary.display().to_string(),
            socket: self.config.socket.display().to_string(),
            host_bundle_id: self.config.host_bundle_id.clone(),
            restarts: tracked.restarts,
            last_error: tracked.last_error.clone(),
            last_test: tracked.last_test.clone(),
        }
    }

    /// One tool call on the running driver; Err is a plain reason.
    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let connection = self
            .connection()
            .ok_or_else(|| "Cua Driver isn't running".to_string())?;
        let client = CuaDriver::connect(Some(connection.socket_path)).map_err(|e| e.to_string())?;
        let result = client
            .call_tool(tool.to_string(), arguments.to_string())
            .await
            .map_err(|e| e.to_string())?;
        if result.is_error {
            return Err(if result.text.is_empty() {
                format!("{tool} failed")
            } else {
                result.text
            });
        }
        Ok(result
            .structured_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or(Value::String(result.text)))
    }

    /// The driver's settings (`get_config`).
    pub async fn get_config(&self) -> Result<Value, String> {
        self.call("get_config", serde_json::json!({})).await
    }

    /// Changes some of the driver's settings (`set_config` with only the
    /// given keys) and returns the settings after the change.
    pub async fn set_config(
        &self,
        changes: serde_json::Map<String, Value>,
    ) -> Result<Value, String> {
        self.call("set_config", Value::Object(changes)).await
    }

    /// Accessibility and Screen Recording as the driver sees them. In
    /// embedded mode these are Cua Spaces' grants. Never shows a prompt.
    pub async fn permissions(&self) -> Result<Value, String> {
        self.call("check_permissions", serde_json::json!({ "prompt": false }))
            .await
    }

    /// Proves the driver works: both permissions granted, a screenshot,
    /// and one agent-cursor move (the real pointer stays put). Kept as the
    /// last test in [`Self::status`].
    pub async fn live_test(&self) -> LiveTest {
        let failure = self.run_live_test().await.err();
        let test = LiveTest {
            passed: failure.is_none(),
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            failure,
        };
        self.tracked.lock().unwrap().last_test = Some(test.clone());
        test
    }

    async fn run_live_test(&self) -> Result<(), String> {
        let permissions = self.permissions().await?;
        for (key, name) in [
            ("accessibility", "Accessibility"),
            ("screen_recording", "Screen Recording"),
        ] {
            if !granted(&permissions, key) {
                return Err(format!("{name} is off for Cua Spaces"));
            }
        }
        self.call("get_screen_size", serde_json::json!({}))
            .await
            .map_err(|e| format!("couldn't read the screen: {e}"))?;
        self.call(
            "move_cursor",
            serde_json::json!({
                "x": 200,
                "y": 200,
                "scope": "window",
                "cursor_id": "cua-spaces-setup-test",
                "session": "cua-spaces-setup-test",
            }),
        )
        .await
        .map_err(|e| format!("couldn't move the agent cursor: {e}"))?;
        Ok(())
    }
}

/// Whether `check_permissions` reports `key` as granted. Accepts the
/// boolean form (`"accessibility": true`) and the object form
/// (`"accessibility": {"granted": true}`).
pub fn granted(permissions: &Value, key: &str) -> bool {
    match permissions.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Object(o)) => o.get("granted").and_then(Value::as_bool).unwrap_or(false),
        _ => false,
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Starts the embedded driver when it is on, under `shutdown`. Returns the
/// handle the app's status and settings calls go through.
pub fn spawn(shutdown: CancellationToken) -> Option<Arc<EmbeddedDriver>> {
    let config = match EmbeddedDriverConfig::from_env() {
        Ok(config) => config,
        Err(reason) => {
            tracing::debug!(%reason, "no embedded Cua Driver");
            return None;
        }
    };
    match EmbeddedDriver::new(config) {
        Ok(driver) => {
            tokio::spawn(driver.clone().supervise(shutdown));
            Some(driver)
        }
        Err(error) => {
            tracing::warn!(%error, "Cua Driver for coding agents is misconfigured");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe_with_driver() -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let helpers = dir.path().join("Cua Spaces.app/Contents/Helpers");
        std::fs::create_dir_all(&helpers).unwrap();
        let driver = helpers.join("cua-driver");
        std::fs::write(&driver, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
        let exe = helpers.join("cua-spacesd");
        (dir, exe)
    }

    #[test]
    fn off_unless_enabled() {
        let (_dir, exe) = exe_with_driver();
        for enable in [None, Some("0".to_string()), Some("yes".to_string())] {
            let r = EmbeddedDriverConfig::resolve(enable, None, &exe, CuaChannel::Stable);
            assert!(r.unwrap_err().starts_with("off"));
        }
    }

    #[test]
    fn uses_the_bundled_driver_and_the_channels_names() {
        let (_dir, exe) = exe_with_driver();
        let c = EmbeddedDriverConfig::resolve(Some("1".into()), None, &exe, CuaChannel::Stable)
            .unwrap();
        assert_eq!(c.binary, exe.parent().unwrap().join("cua-driver"));
        assert_eq!(c.host_bundle_id, "com.trycua.spaces.macos");
        assert_eq!(c.socket, CuaChannel::Stable.embedded_driver_socket());
        let l =
            EmbeddedDriverConfig::resolve(Some("1".into()), None, &exe, CuaChannel::Local).unwrap();
        assert_eq!(l.host_bundle_id, "com.trycua.spaces.macos.local");
        assert_eq!(l.socket, CuaChannel::Local.embedded_driver_socket());
    }

    #[test]
    fn an_override_wins_and_a_missing_driver_says_where_it_looked() {
        let (_dir, exe) = exe_with_driver();
        let c = EmbeddedDriverConfig::resolve(
            Some("1".into()),
            Some("/opt/dev/cua-driver".into()),
            &exe,
            CuaChannel::Stable,
        )
        .unwrap();
        assert_eq!(c.binary, PathBuf::from("/opt/dev/cua-driver"));
        let empty = tempfile::tempdir().unwrap();
        let err = EmbeddedDriverConfig::resolve(
            Some("1".into()),
            None,
            &empty.path().join("cua-spacesd"),
            CuaChannel::Stable,
        )
        .unwrap_err();
        assert!(err.contains("Contents/Helpers"), "{err}");
    }

    #[test]
    fn restarts_back_off_to_a_minute() {
        let secs: Vec<u64> = (1..=9).map(|a| restart_delay(a).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
    }

    #[test]
    fn permissions_read_both_shapes() {
        let v = serde_json::json!({
            "accessibility": true,
            "screen_recording": {"granted": false},
        });
        assert!(granted(&v, "accessibility"));
        assert!(!granted(&v, "screen_recording"));
        assert!(!granted(&v, "missing"));
    }
}
