//! Rust-owned lifecycle for a host-embedded Cua Driver daemon.
//!
//! Language bindings deliberately expose this object rather than reimplementing
//! process, endpoint, readiness, authorization, and cleanup logic per runtime.

use cua_driver_contract::{
    CAPABILITY_VERSION, CONTRACT_VERSION, MCP_PROTOCOL_VERSION, TOOLS_LIST_SCHEMA_VERSION,
};
use cua_driver_core::daemon::{request_daemon_metadata, DaemonMetadata};
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncWriteExt as _;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Notify;
use uuid::Uuid;

const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_SHUTDOWN_TIMEOUT_MS: u64 = 2_000;
const HANDSHAKE_ATTEMPT_TIMEOUT_MS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EmbeddedPermissionMode {
    Standard,
    Bounded,
    Unrestricted,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmbeddedEnvironmentVariable {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EmbeddedDriverHostOptions {
    pub binary_path: String,
    pub host_bundle_id: String,
    pub socket_path: Option<String>,
    pub startup_timeout_ms: Option<u64>,
    pub shutdown_timeout_ms: Option<u64>,
    pub permission_mode: Option<EmbeddedPermissionMode>,
    pub session_policy_path: Option<String>,
    pub approve_session_policy: bool,
    pub dangerously_bypass_approvals: bool,
    pub environment: Vec<EmbeddedEnvironmentVariable>,
    pub inherit_stderr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmbeddedMcpConfiguration {
    pub command: String,
    pub args: Vec<String>,
    pub environment: Vec<EmbeddedEnvironmentVariable>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmbeddedDriverConnection {
    pub socket_path: String,
    pub pid: u32,
    pub generation: String,
    pub driver_version: String,
    pub contract_version: String,
    pub mcp_protocol_version: String,
    pub mcp: EmbeddedMcpConfiguration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EmbeddedDriverHostState {
    Stopped,
    Starting,
    Ready,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmbeddedDriverExit {
    pub generation: String,
    pub code: Option<i32>,
    pub success: bool,
}

#[derive(Debug, Error, uniffi::Error)]
pub enum EmbeddedDriverError {
    #[error("invalid embedded host configuration: {reason}")]
    Configuration { reason: String },
    #[error("embedded endpoint conflict at {path}: {reason}")]
    EndpointConflict { path: String, reason: String },
    #[error("failed to spawn embedded cua-driver at {binary_path}: {reason}")]
    Spawn { binary_path: String, reason: String },
    #[error("embedded cua-driver startup was cancelled")]
    StartupCancelled,
    #[error("embedded cua-driver did not become ready within {timeout_ms}ms")]
    StartupTimeout { timeout_ms: u64 },
    #[error("embedded cua-driver exited before readiness (code={code:?})")]
    ExitedBeforeReady { code: Option<i32> },
    #[error("embedded daemon is incompatible: {reason}")]
    IncompatibleDaemon { reason: String },
    #[error("embedded host lifecycle error: {reason}")]
    Lifecycle { reason: String },
}

#[derive(Debug, Clone)]
struct ValidatedOptions {
    binary_path: String,
    host_bundle_id: String,
    socket_path: Option<String>,
    startup_timeout: Duration,
    shutdown_timeout: Duration,
    permission_mode: EmbeddedPermissionMode,
    session_policy_path: Option<String>,
    approve_session_policy: bool,
    dangerously_bypass_approvals: bool,
    environment: Vec<EmbeddedEnvironmentVariable>,
    inherit_stderr: bool,
}

#[cfg(unix)]
#[derive(Debug, Clone)]
struct EndpointIdentity {
    device: u64,
    inode: u64,
}

#[cfg(not(unix))]
#[derive(Debug, Clone)]
struct EndpointIdentity;

struct RunningProcess {
    connection: EmbeddedDriverConnection,
    child: Child,
    liveness: Option<ChildStdin>,
    endpoint_identity: EndpointIdentity,
}

enum HostPhase {
    Stopped,
    Starting {
        generation: String,
        cancel: Arc<AtomicBool>,
    },
    Ready(Box<RunningProcess>),
    Stopping {
        generation: String,
    },
}

struct HostInner {
    phase: HostPhase,
    last_exit: Option<EmbeddedDriverExit>,
}

#[derive(uniffi::Object)]
pub struct EmbeddedCuaDriverHost {
    options: ValidatedOptions,
    inner: Mutex<HostInner>,
    changed: Notify,
}

struct StartupTransitionGuard {
    host: Arc<EmbeddedCuaDriverHost>,
    generation: String,
    armed: bool,
}

struct StopTransitionGuard {
    host: Arc<EmbeddedCuaDriverHost>,
    running: Option<RunningProcess>,
    armed: bool,
}

impl StopTransitionGuard {
    fn running_mut(&mut self) -> &mut RunningProcess {
        self.running
            .as_mut()
            .expect("stop transition must own its process")
    }

    fn disarm(&mut self) -> RunningProcess {
        self.armed = false;
        self.running
            .take()
            .expect("stop transition must own its process")
    }
}

impl Drop for StopTransitionGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(mut running) = self.running.take() else {
            return;
        };
        let generation = running.connection.generation.clone();
        drop(running.liveness.take());
        let _ = running.child.start_kill();
        cleanup_owned_endpoint(&running.connection.socket_path, &running.endpoint_identity);
        // Drop the kill-on-drop child before waking a concurrent starter.
        drop(running);

        let mut inner = self.host.inner.lock().unwrap();
        if matches!(
            &inner.phase,
            HostPhase::Stopping { generation: current } if current == &generation
        ) {
            inner.phase = HostPhase::Stopped;
        }
        drop(inner);
        self.host.changed.notify_waiters();
    }
}

impl StartupTransitionGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StartupTransitionGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut inner = self.host.inner.lock().unwrap();
        if matches!(
            &inner.phase,
            HostPhase::Starting { generation, .. } if generation == &self.generation
        ) {
            inner.phase = HostPhase::Stopped;
            drop(inner);
            self.host.changed.notify_waiters();
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl EmbeddedCuaDriverHost {
    #[uniffi::constructor]
    pub fn new(
        binary_path: String,
        host_bundle_id: String,
    ) -> Result<Arc<Self>, EmbeddedDriverError> {
        Self::with_options(EmbeddedDriverHostOptions {
            binary_path,
            host_bundle_id,
            socket_path: None,
            startup_timeout_ms: None,
            shutdown_timeout_ms: None,
            permission_mode: None,
            session_policy_path: None,
            approve_session_policy: false,
            dangerously_bypass_approvals: false,
            environment: Vec::new(),
            inherit_stderr: true,
        })
    }

    #[uniffi::constructor]
    pub fn with_options(
        options: EmbeddedDriverHostOptions,
    ) -> Result<Arc<Self>, EmbeddedDriverError> {
        Ok(Arc::new(Self {
            options: validate_options(options)?,
            inner: Mutex::new(HostInner {
                phase: HostPhase::Stopped,
                last_exit: None,
            }),
            changed: Notify::new(),
        }))
    }

    pub fn state(&self) -> EmbeddedDriverHostState {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_exit_locked(&mut inner);
        match inner.phase {
            HostPhase::Stopped => EmbeddedDriverHostState::Stopped,
            HostPhase::Starting { .. } => EmbeddedDriverHostState::Starting,
            HostPhase::Ready(_) => EmbeddedDriverHostState::Ready,
            HostPhase::Stopping { .. } => EmbeddedDriverHostState::Stopping,
        }
    }

    pub fn connection(&self) -> Option<EmbeddedDriverConnection> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_exit_locked(&mut inner);
        match &inner.phase {
            HostPhase::Ready(running) => Some(running.connection.clone()),
            _ => None,
        }
    }

    pub async fn start(self: Arc<Self>) -> Result<EmbeddedDriverConnection, EmbeddedDriverError> {
        loop {
            let notified = self.changed.notified();
            let transition = {
                let mut inner = self.inner.lock().unwrap();
                self.refresh_exit_locked(&mut inner);
                match &inner.phase {
                    HostPhase::Ready(running) => return Ok(running.connection.clone()),
                    HostPhase::Stopped => {
                        let generation = Uuid::new_v4().to_string();
                        let cancel = Arc::new(AtomicBool::new(false));
                        inner.phase = HostPhase::Starting {
                            generation: generation.clone(),
                            cancel: cancel.clone(),
                        };
                        Some((generation, cancel))
                    }
                    HostPhase::Starting { .. } | HostPhase::Stopping { .. } => None,
                }
            };
            if let Some((generation, cancel)) = transition {
                return self.clone().perform_start(generation, cancel).await;
            }
            notified.await;
        }
    }

    pub async fn stop(self: Arc<Self>) -> Result<(), EmbeddedDriverError> {
        loop {
            let notified = self.changed.notified();
            enum Action {
                Done,
                Wait,
                Stop(Box<RunningProcess>),
            }
            let action = {
                let mut inner = self.inner.lock().unwrap();
                self.refresh_exit_locked(&mut inner);
                match &mut inner.phase {
                    HostPhase::Stopped => Action::Done,
                    HostPhase::Starting { cancel, .. } => {
                        cancel.store(true, Ordering::Release);
                        Action::Wait
                    }
                    HostPhase::Stopping { .. } => Action::Wait,
                    HostPhase::Ready(_) => {
                        let phase = std::mem::replace(&mut inner.phase, HostPhase::Stopped);
                        let HostPhase::Ready(running) = phase else {
                            unreachable!()
                        };
                        inner.phase = HostPhase::Stopping {
                            generation: running.connection.generation.clone(),
                        };
                        Action::Stop(running)
                    }
                }
            };
            match action {
                Action::Done => return Ok(()),
                Action::Wait => notified.await,
                Action::Stop(running) => return self.clone().finish_stop(*running).await,
            }
        }
    }

    pub async fn restart(self: Arc<Self>) -> Result<EmbeddedDriverConnection, EmbeddedDriverError> {
        self.clone().stop().await?;
        self.start().await
    }

    pub async fn wait_for_exit(
        self: Arc<Self>,
        generation: String,
    ) -> Result<EmbeddedDriverExit, EmbeddedDriverError> {
        loop {
            {
                let mut inner = self.inner.lock().unwrap();
                self.refresh_exit_locked(&mut inner);
                if let Some(exit) = &inner.last_exit {
                    if exit.generation == generation {
                        return Ok(exit.clone());
                    }
                }
                let still_live = match &inner.phase {
                    HostPhase::Starting {
                        generation: current,
                        ..
                    }
                    | HostPhase::Stopping {
                        generation: current,
                    } => current == &generation,
                    HostPhase::Ready(running) => running.connection.generation == generation,
                    HostPhase::Stopped => false,
                };
                if !still_live {
                    return Err(EmbeddedDriverError::Lifecycle {
                        reason: format!("generation {generation} is not live or recently exited"),
                    });
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl EmbeddedCuaDriverHost {
    async fn perform_start(
        self: Arc<Self>,
        generation: String,
        cancel: Arc<AtomicBool>,
    ) -> Result<EmbeddedDriverConnection, EmbeddedDriverError> {
        let mut transition_guard = StartupTransitionGuard {
            host: self.clone(),
            generation: generation.clone(),
            armed: true,
        };
        let socket_path = self
            .options
            .socket_path
            .clone()
            .unwrap_or_else(|| default_socket_path(&generation));
        prepare_endpoint(&socket_path)?;

        let mut command = Command::new(&self.options.binary_path);
        command
            .args(self.serve_args(&socket_path))
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(if self.options.inherit_stderr {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .kill_on_drop(true);
        for variable in safe_environment(&self.options.environment) {
            command.env(variable.name, variable.value);
        }

        let mut child = command
            .spawn()
            .map_err(|error| EmbeddedDriverError::Spawn {
                binary_path: self.options.binary_path.clone(),
                reason: error.to_string(),
            })?;
        let pid = child.id().ok_or_else(|| EmbeddedDriverError::Spawn {
            binary_path: self.options.binary_path.clone(),
            reason: "spawned process has no pid".into(),
        })?;
        let mut liveness = child
            .stdin
            .take()
            .ok_or_else(|| EmbeddedDriverError::Spawn {
                binary_path: self.options.binary_path.clone(),
                reason: "failed to create parent-liveness stdin pipe".into(),
            })?;

        let deadline = tokio::time::Instant::now() + self.options.startup_timeout;
        let metadata = loop {
            if cancel.load(Ordering::Acquire) {
                terminate_startup_child(&mut child, &mut liveness).await;
                return Err(EmbeddedDriverError::StartupCancelled);
            }
            if let Some(status) =
                child
                    .try_wait()
                    .map_err(|error| EmbeddedDriverError::Lifecycle {
                        reason: format!("inspect startup child: {error}"),
                    })?
            {
                return Err(EmbeddedDriverError::ExitedBeforeReady {
                    code: status.code(),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                terminate_startup_child(&mut child, &mut liveness).await;
                return Err(EmbeddedDriverError::StartupTimeout {
                    timeout_ms: self.options.startup_timeout.as_millis() as u64,
                });
            }

            if endpoint_may_be_ready(&socket_path) {
                let path = socket_path.clone();
                let attempt = tokio::task::spawn_blocking(move || request_daemon_metadata(&path));
                match tokio::time::timeout(
                    Duration::from_millis(HANDSHAKE_ATTEMPT_TIMEOUT_MS),
                    attempt,
                )
                .await
                {
                    Ok(Ok(Ok(metadata))) => break metadata,
                    Ok(Ok(Err(_))) => {}
                    Ok(Err(join_error)) => {
                        terminate_startup_child(&mut child, &mut liveness).await;
                        return Err(EmbeddedDriverError::Lifecycle {
                            reason: format!("daemon metadata task failed: {join_error}"),
                        });
                    }
                    Err(_) => {
                        terminate_startup_child(&mut child, &mut liveness).await;
                        return Err(EmbeddedDriverError::StartupTimeout {
                            timeout_ms: self.options.startup_timeout.as_millis() as u64,
                        });
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        let endpoint_identity = match capture_endpoint_identity(&socket_path) {
            Ok(identity) => identity,
            Err(error) => {
                terminate_startup_child(&mut child, &mut liveness).await;
                return Err(error);
            }
        };
        if let Err(error) = validate_metadata(&metadata, pid, &self.options.host_bundle_id) {
            terminate_startup_child(&mut child, &mut liveness).await;
            cleanup_owned_endpoint(&socket_path, &endpoint_identity);
            return Err(error);
        }
        let connection = EmbeddedDriverConnection {
            socket_path: socket_path.clone(),
            pid,
            generation: generation.clone(),
            driver_version: metadata.driver_version,
            contract_version: metadata.contract_version,
            mcp_protocol_version: metadata.mcp_protocol_version,
            mcp: self.mcp_configuration(&socket_path),
        };

        if cancel.load(Ordering::Acquire) {
            terminate_startup_child(&mut child, &mut liveness).await;
            cleanup_owned_endpoint(&socket_path, &endpoint_identity);
            return Err(EmbeddedDriverError::StartupCancelled);
        }
        let mut pending = Some(RunningProcess {
            connection: connection.clone(),
            child,
            liveness: Some(liveness),
            endpoint_identity,
        });
        let still_starting = {
            let mut inner = self.inner.lock().unwrap();
            let still_starting = matches!(
                &inner.phase,
                HostPhase::Starting { generation: current, .. } if current == &generation
            );
            if still_starting {
                inner.last_exit = None;
                inner.phase = HostPhase::Ready(Box::new(pending.take().unwrap()));
            }
            still_starting
        };
        if !still_starting {
            let mut running = pending.unwrap();
            let mut liveness = running.liveness.take().unwrap();
            terminate_startup_child(&mut running.child, &mut liveness).await;
            cleanup_owned_endpoint(&socket_path, &running.endpoint_identity);
            return Err(EmbeddedDriverError::StartupCancelled);
        }
        transition_guard.disarm();
        self.changed.notify_waiters();
        Ok(connection)
    }

    async fn finish_stop(
        self: Arc<Self>,
        running: RunningProcess,
    ) -> Result<(), EmbeddedDriverError> {
        let mut transition_guard = StopTransitionGuard {
            host: self.clone(),
            running: Some(running),
            armed: true,
        };
        if let Some(mut liveness) = transition_guard.running_mut().liveness.take() {
            // Closing the write side is the primary graceful shutdown signal.
            let _ = liveness.shutdown().await;
            drop(liveness);
        }
        let status = {
            let running = transition_guard.running_mut();
            match tokio::time::timeout(self.options.shutdown_timeout, running.child.wait()).await {
                Ok(result) => result.map_err(|error| EmbeddedDriverError::Lifecycle {
                    reason: format!("wait for embedded daemon: {error}"),
                }),
                Err(_) => {
                    match running.child.start_kill() {
                        Ok(()) => running.child.wait().await.map_err(|error| {
                            EmbeddedDriverError::Lifecycle {
                                reason: format!("reap force-killed embedded daemon: {error}"),
                            }
                        }),
                        Err(error) => Err(EmbeddedDriverError::Lifecycle {
                            reason: format!("force-kill embedded daemon: {error}"),
                        }),
                    }
                }
            }
        };
        let running = transition_guard.disarm();
        cleanup_owned_endpoint(&running.connection.socket_path, &running.endpoint_identity);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.phase = HostPhase::Stopped;
            if let Ok(status) = &status {
                inner.last_exit = Some(EmbeddedDriverExit {
                    generation: running.connection.generation,
                    code: status.code(),
                    success: status.success(),
                });
            }
        }
        self.changed.notify_waiters();
        status.map(|_| ())
    }

    fn serve_args(&self, socket_path: &str) -> Vec<String> {
        let mut args = vec![
            "serve".into(),
            "--embedded".into(),
            "--parent-liveness-stdio".into(),
            "--no-permissions-gate".into(),
            "--socket".into(),
            socket_path.into(),
            "--host-bundle-id".into(),
            self.options.host_bundle_id.clone(),
            "--permission-mode".into(),
            match self.options.permission_mode {
                EmbeddedPermissionMode::Standard => "standard",
                EmbeddedPermissionMode::Bounded => "bounded",
                EmbeddedPermissionMode::Unrestricted => "unrestricted",
            }
            .into(),
        ];
        if let Some(path) = &self.options.session_policy_path {
            args.extend(["--session-policy".into(), path.clone()]);
        }
        if self.options.approve_session_policy {
            args.push("--approve-session-policy".into());
        }
        if self.options.dangerously_bypass_approvals {
            args.push("--dangerously-bypass-approvals".into());
        }
        args
    }

    fn mcp_configuration(&self, socket_path: &str) -> EmbeddedMcpConfiguration {
        EmbeddedMcpConfiguration {
            command: self.options.binary_path.clone(),
            args: vec![
                "mcp".into(),
                "--embedded".into(),
                "--socket".into(),
                socket_path.into(),
                "--host-bundle-id".into(),
                self.options.host_bundle_id.clone(),
            ],
            // Arguments carry the complete embedded routing contract. Keeping
            // this empty avoids leaking daemon-only authorization state into
            // the generic MCP proxy process.
            environment: Vec::new(),
        }
    }

    fn refresh_exit_locked(&self, inner: &mut HostInner) {
        let phase = std::mem::replace(&mut inner.phase, HostPhase::Stopped);
        match phase {
            HostPhase::Ready(mut running) => match running.child.try_wait() {
                Ok(Some(status)) => {
                    cleanup_owned_endpoint(
                        &running.connection.socket_path,
                        &running.endpoint_identity,
                    );
                    inner.last_exit = Some(EmbeddedDriverExit {
                        generation: running.connection.generation,
                        code: status.code(),
                        success: status.success(),
                    });
                }
                Ok(None) | Err(_) => inner.phase = HostPhase::Ready(running),
            },
            other => inner.phase = other,
        }
    }
}

impl Drop for EmbeddedCuaDriverHost {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        let phase = std::mem::replace(&mut inner.phase, HostPhase::Stopped);
        match phase {
            HostPhase::Starting { cancel, .. } => cancel.store(true, Ordering::Release),
            HostPhase::Ready(mut running) => {
                drop(running.liveness.take());
                let _ = running.child.start_kill();
                cleanup_owned_endpoint(&running.connection.socket_path, &running.endpoint_identity);
            }
            HostPhase::Stopped | HostPhase::Stopping { .. } => {}
        }
    }
}

fn validate_options(
    options: EmbeddedDriverHostOptions,
) -> Result<ValidatedOptions, EmbeddedDriverError> {
    if options.binary_path.trim().is_empty() {
        return configuration_error("binary_path must not be empty");
    }
    if !std::path::Path::new(&options.binary_path).is_absolute() {
        return configuration_error("binary_path must be absolute");
    }
    if options.host_bundle_id.trim().is_empty()
        || options.host_bundle_id.len() > 255
        || options
            .host_bundle_id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return configuration_error(
            "host_bundle_id must be 1-255 non-whitespace, non-control characters",
        );
    }
    let startup_timeout_ms = options
        .startup_timeout_ms
        .unwrap_or(DEFAULT_STARTUP_TIMEOUT_MS);
    let shutdown_timeout_ms = options
        .shutdown_timeout_ms
        .unwrap_or(DEFAULT_SHUTDOWN_TIMEOUT_MS);
    if startup_timeout_ms == 0 || shutdown_timeout_ms == 0 {
        return configuration_error("startup and shutdown timeouts must be positive");
    }
    let permission_mode = options
        .permission_mode
        .unwrap_or(EmbeddedPermissionMode::Standard);
    match permission_mode {
        EmbeddedPermissionMode::Standard => {
            if options.session_policy_path.is_some() || options.approve_session_policy {
                return configuration_error(
                    "session policy options are valid only in bounded mode",
                );
            }
            if options.dangerously_bypass_approvals {
                return configuration_error(
                    "dangerously_bypass_approvals is valid only in unrestricted mode",
                );
            }
        }
        EmbeddedPermissionMode::Bounded => {
            if options
                .session_policy_path
                .as_deref()
                .is_none_or(|path| path.trim().is_empty())
            {
                return configuration_error("bounded mode requires session_policy_path");
            }
            if !options.approve_session_policy {
                return configuration_error("bounded mode requires approve_session_policy=true");
            }
            if options.dangerously_bypass_approvals {
                return configuration_error("bounded mode cannot bypass runtime approvals");
            }
        }
        EmbeddedPermissionMode::Unrestricted => {
            if !options.dangerously_bypass_approvals {
                return configuration_error(
                    "unrestricted mode requires dangerously_bypass_approvals=true",
                );
            }
            if options.session_policy_path.is_some() || options.approve_session_policy {
                return configuration_error(
                    "unrestricted mode cannot use a bounded session policy",
                );
            }
        }
    }
    for variable in &options.environment {
        if !allowed_environment_name(&variable.name)
            || variable.name.contains('=')
            || variable.name.contains('\0')
            || variable.value.contains('\0')
        {
            return configuration_error(format!(
                "environment variable {} is not in the embedded safe allowlist",
                variable.name
            ));
        }
    }
    if let Some(socket_path) = &options.socket_path {
        validate_socket_path(socket_path)?;
    }
    Ok(ValidatedOptions {
        binary_path: options.binary_path,
        host_bundle_id: options.host_bundle_id,
        socket_path: options.socket_path,
        startup_timeout: Duration::from_millis(startup_timeout_ms),
        shutdown_timeout: Duration::from_millis(shutdown_timeout_ms),
        permission_mode,
        session_policy_path: options.session_policy_path,
        approve_session_policy: options.approve_session_policy,
        dangerously_bypass_approvals: options.dangerously_bypass_approvals,
        environment: options.environment,
        inherit_stderr: options.inherit_stderr,
    })
}

fn configuration_error<T>(reason: impl Into<String>) -> Result<T, EmbeddedDriverError> {
    Err(EmbeddedDriverError::Configuration {
        reason: reason.into(),
    })
}

fn allowed_environment_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("LC_")
        || matches!(
            upper.as_str(),
            "PATH"
                | "HOME"
                | "USER"
                | "LOGNAME"
                | "SHELL"
                | "TMPDIR"
                | "TMP"
                | "TEMP"
                | "LANG"
                | "SYSTEMROOT"
                | "WINDIR"
                | "COMSPEC"
                | "PATHEXT"
                | "APPDATA"
                | "LOCALAPPDATA"
                | "PROGRAMDATA"
                | "DISPLAY"
                | "WAYLAND_DISPLAY"
                | "XDG_RUNTIME_DIR"
                | "XDG_SESSION_TYPE"
                | "DBUS_SESSION_BUS_ADDRESS"
                | "XAUTHORITY"
                | "CUA_LOG"
        )
}

fn safe_environment(overrides: &[EmbeddedEnvironmentVariable]) -> Vec<EmbeddedEnvironmentVariable> {
    let mut values = BTreeMap::new();
    for (name, value) in std::env::vars() {
        if allowed_environment_name(&name) {
            values.insert(name, value);
        }
    }
    for variable in overrides {
        values.insert(variable.name.clone(), variable.value.clone());
    }
    values
        .into_iter()
        .map(|(name, value)| EmbeddedEnvironmentVariable { name, value })
        .collect()
}

fn validate_metadata(
    metadata: &DaemonMetadata,
    expected_pid: u32,
    host_bundle_id: &str,
) -> Result<(), EmbeddedDriverError> {
    let mismatch = if metadata.pid != expected_pid {
        Some(format!(
            "endpoint pid {} does not match spawned pid {expected_pid}",
            metadata.pid
        ))
    } else if !metadata.embedded {
        Some("daemon did not confirm embedded mode".into())
    } else if metadata.host_bundle_id.as_deref() != Some(host_bundle_id) {
        Some(format!(
            "daemon host bundle id {:?} does not match {host_bundle_id}",
            metadata.host_bundle_id
        ))
    } else if metadata.contract_version != CONTRACT_VERSION {
        Some(format!(
            "contract version {} does not match SDK {CONTRACT_VERSION}",
            metadata.contract_version
        ))
    } else if metadata.tools_list_schema_version != TOOLS_LIST_SCHEMA_VERSION {
        Some(format!(
            "tools-list schema version {} does not match SDK {TOOLS_LIST_SCHEMA_VERSION}",
            metadata.tools_list_schema_version
        ))
    } else if metadata.capability_version != CAPABILITY_VERSION {
        Some(format!(
            "capability version {} does not match SDK {CAPABILITY_VERSION}",
            metadata.capability_version
        ))
    } else if metadata.mcp_protocol_version != MCP_PROTOCOL_VERSION {
        Some(format!(
            "MCP protocol version {} does not match SDK {MCP_PROTOCOL_VERSION}",
            metadata.mcp_protocol_version
        ))
    } else {
        None
    };
    mismatch.map_or(Ok(()), |reason| {
        Err(EmbeddedDriverError::IncompatibleDaemon { reason })
    })
}

fn default_socket_path(generation: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        return format!(r"\\.\pipe\cua-{}-{}", std::process::id(), &generation[..8]);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let filename = format!("cua-{}-{}.sock", std::process::id(), &generation[..8]);
        let preferred = std::env::temp_dir().join(&filename);
        if preferred.as_os_str().to_string_lossy().len() < 100 {
            preferred.to_string_lossy().into_owned()
        } else {
            format!("/tmp/{filename}")
        }
    }
}

fn validate_socket_path(socket_path: &str) -> Result<(), EmbeddedDriverError> {
    if socket_path.trim().is_empty() {
        return configuration_error("socket_path must not be empty");
    }
    #[cfg(unix)]
    {
        if !std::path::Path::new(socket_path).is_absolute() {
            return configuration_error("Unix socket_path must be absolute");
        }
        if socket_path.len() >= 104 {
            return configuration_error(
                "socket_path exceeds the portable Unix socket length limit",
            );
        }
    }
    #[cfg(target_os = "windows")]
    if !socket_path.starts_with(r"\\.\pipe\") {
        return configuration_error("Windows socket_path must be a named pipe path");
    }
    Ok(())
}

fn prepare_endpoint(socket_path: &str) -> Result<(), EmbeddedDriverError> {
    validate_socket_path(socket_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
        match std::fs::symlink_metadata(socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
                    return Err(EmbeddedDriverError::EndpointConflict {
                        path: socket_path.into(),
                        reason: "a process is already accepting connections".into(),
                    });
                }
                let current = std::fs::symlink_metadata(socket_path).map_err(|error| {
                    EmbeddedDriverError::EndpointConflict {
                        path: socket_path.into(),
                        reason: format!("stale socket changed during preflight: {error}"),
                    }
                })?;
                if !current.file_type().is_socket()
                    || current.dev() != metadata.dev()
                    || current.ino() != metadata.ino()
                {
                    return Err(EmbeddedDriverError::EndpointConflict {
                        path: socket_path.into(),
                        reason: "stale socket identity changed during preflight".into(),
                    });
                }
                std::fs::remove_file(socket_path).map_err(|error| {
                    EmbeddedDriverError::EndpointConflict {
                        path: socket_path.into(),
                        reason: format!("cannot remove stale socket: {error}"),
                    }
                })?;
            }
            Ok(_) => {
                return Err(EmbeddedDriverError::EndpointConflict {
                    path: socket_path.into(),
                    reason: "refusing to replace a non-socket path or symlink".into(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(EmbeddedDriverError::EndpointConflict {
                    path: socket_path.into(),
                    reason: error.to_string(),
                });
            }
        }
    }
    #[cfg(target_os = "windows")]
    if cua_driver_core::daemon::is_daemon_listening(socket_path) {
        return Err(EmbeddedDriverError::EndpointConflict {
            path: socket_path.into(),
            reason: "a named-pipe server is already listening".into(),
        });
    }
    Ok(())
}

fn endpoint_may_be_ready(socket_path: &str) -> bool {
    #[cfg(unix)]
    {
        std::fs::symlink_metadata(socket_path)
            .ok()
            .is_some_and(|metadata| {
                use std::os::unix::fs::FileTypeExt as _;
                metadata.file_type().is_socket()
            })
    }
    #[cfg(target_os = "windows")]
    {
        cua_driver_core::daemon::is_daemon_listening(socket_path)
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = socket_path;
        false
    }
}

fn capture_endpoint_identity(socket_path: &str) -> Result<EndpointIdentity, EmbeddedDriverError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
        let metadata = std::fs::symlink_metadata(socket_path).map_err(|error| {
            EmbeddedDriverError::Lifecycle {
                reason: format!("inspect embedded socket after readiness: {error}"),
            }
        })?;
        if !metadata.file_type().is_socket() || metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(EmbeddedDriverError::Lifecycle {
                reason: "embedded endpoint is not a private 0600 Unix socket".into(),
            });
        }
        Ok(EndpointIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = socket_path;
        Ok(EndpointIdentity)
    }
}

fn cleanup_owned_endpoint(socket_path: &str, identity: &EndpointIdentity) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if let Ok(metadata) = std::fs::symlink_metadata(socket_path) {
            if metadata.dev() == identity.device && metadata.ino() == identity.inode {
                let _ = std::fs::remove_file(socket_path);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (socket_path, identity);
    }
}

async fn terminate_startup_child(child: &mut Child, liveness: &mut ChildStdin) {
    let _ = liveness.shutdown().await;
    if tokio::time::timeout(Duration::from_millis(250), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(mode: EmbeddedPermissionMode) -> EmbeddedDriverHostOptions {
        EmbeddedDriverHostOptions {
            binary_path: "/example/cua-driver".into(),
            host_bundle_id: "com.example.host".into(),
            socket_path: None,
            startup_timeout_ms: None,
            shutdown_timeout_ms: None,
            permission_mode: Some(mode),
            session_policy_path: None,
            approve_session_policy: false,
            dangerously_bypass_approvals: false,
            environment: Vec::new(),
            inherit_stderr: false,
        }
    }

    #[test]
    fn authorization_modes_require_explicit_acknowledgements() {
        assert!(validate_options(options(EmbeddedPermissionMode::Standard)).is_ok());

        let bounded = options(EmbeddedPermissionMode::Bounded);
        assert!(validate_options(bounded).is_err());
        let mut bounded = options(EmbeddedPermissionMode::Bounded);
        bounded.session_policy_path = Some("policy.json".into());
        bounded.approve_session_policy = true;
        assert!(validate_options(bounded).is_ok());

        let unrestricted = options(EmbeddedPermissionMode::Unrestricted);
        assert!(validate_options(unrestricted).is_err());
        let mut unrestricted = options(EmbeddedPermissionMode::Unrestricted);
        unrestricted.dangerously_bypass_approvals = true;
        assert!(validate_options(unrestricted).is_ok());
    }

    #[test]
    fn environment_is_allowlisted_and_driver_controls_are_reserved() {
        assert!(allowed_environment_name("PATH"));
        assert!(allowed_environment_name("WAYLAND_DISPLAY"));
        assert!(!allowed_environment_name("CUA_DRIVER_PERMISSION_MODE"));
        assert!(!allowed_environment_name("LD_PRELOAD"));
        assert!(!allowed_environment_name("NODE_OPTIONS"));
    }

    #[test]
    fn metadata_validation_requires_same_process_and_contract() {
        let metadata = DaemonMetadata {
            driver_version: env!("CARGO_PKG_VERSION").into(),
            contract_version: CONTRACT_VERSION.into(),
            tools_list_schema_version: TOOLS_LIST_SCHEMA_VERSION.into(),
            capability_version: CAPABILITY_VERSION.into(),
            mcp_protocol_version: MCP_PROTOCOL_VERSION.into(),
            pid: 42,
            embedded: true,
            host_bundle_id: Some("com.example.host".into()),
        };
        assert!(validate_metadata(&metadata, 42, "com.example.host").is_ok());
        assert!(validate_metadata(&metadata, 43, "com.example.host").is_err());
        assert!(validate_metadata(&metadata, 42, "com.other").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn endpoint_preflight_refuses_non_socket_paths() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("important.txt");
        std::fs::write(&file, "keep").unwrap();
        let error = prepare_endpoint(file.to_str().unwrap()).unwrap_err();
        assert!(matches!(
            error,
            EmbeddedDriverError::EndpointConflict { .. }
        ));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "keep");
    }

    #[test]
    fn generated_default_endpoint_is_short_and_unique() {
        let first = default_socket_path("01234567-89ab-cdef-0123-456789abcdef");
        let second = default_socket_path("fedcba98-7654-3210-fedc-ba9876543210");
        assert_ne!(first, second);
        #[cfg(unix)]
        assert!(first.len() < 104);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_stop_resets_state_and_cleans_its_endpoint() {
        let host = EmbeddedCuaDriverHost::new(
            "/example/cua-driver".into(),
            "com.example.cancel-stop".into(),
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("driver.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        std::fs::set_permissions(
            &socket_path,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        let endpoint_identity = capture_endpoint_identity(socket_path.to_str().unwrap()).unwrap();

        let mut child = Command::new("/bin/sh");
        child
            .args(["-c", "while :; do sleep 1; done"])
            .stdin(Stdio::piped())
            .kill_on_drop(true);
        let mut child = child.spawn().unwrap();
        let liveness = child.stdin.take();
        let generation = "cancelled-stop-generation".to_owned();
        let running = RunningProcess {
            connection: EmbeddedDriverConnection {
                socket_path: socket_path.to_string_lossy().into_owned(),
                pid: child.id().unwrap(),
                generation: generation.clone(),
                driver_version: env!("CARGO_PKG_VERSION").into(),
                contract_version: CONTRACT_VERSION.into(),
                mcp_protocol_version: MCP_PROTOCOL_VERSION.into(),
                mcp: EmbeddedMcpConfiguration {
                    command: "/example/cua-driver".into(),
                    args: Vec::new(),
                    environment: Vec::new(),
                },
            },
            child,
            liveness,
            endpoint_identity,
        };
        host.inner.lock().unwrap().phase = HostPhase::Stopping { generation };

        drop(StopTransitionGuard {
            host: host.clone(),
            running: Some(running),
            armed: true,
        });
        drop(listener);

        assert_eq!(host.state(), EmbeddedDriverHostState::Stopped);
        assert!(!socket_path.exists());
    }
}
