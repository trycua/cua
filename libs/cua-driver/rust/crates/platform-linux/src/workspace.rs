//! Low-level ownership of an isolated, headless Wayland compositor.
//!
//! This module deliberately does not install its environment into the current
//! process. Callers apply [`NestedCompositorWorkspace::configure_command`] to
//! each application, capture helper, or driver process that belongs to the
//! workspace.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use cua_driver_core::workspace::{
    CreateWorkspaceRequest, CreatedWorkspace, WindowTarget, WorkspaceBackend,
    WorkspaceBackendDescriptor, WorkspaceCapabilities, WorkspaceError, WorkspaceKind,
};
use serde_json::json;

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const CONTROL_SOCKET_NAME: &str = "cua-inject.sock";
pub const CHROMIUM_PROFILE_DIR_NAME: &str = "chromium-profile";

static WORKSPACE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

type LaunchRequest = (Command, mpsc::Sender<std::io::Result<Child>>);

/// Fork compositors from one thread that lives for the lifetime of the daemon.
///
/// Linux delivers PR_SET_PDEATHSIG when the *forking thread* exits, not merely
/// when its process exits. Tokio blocking-pool threads retire while the daemon
/// is healthy, so they must never be the compositor's parent thread.
fn spawn_compositor(command: Command) -> std::io::Result<Child> {
    static LAUNCHER: OnceLock<std::result::Result<mpsc::Sender<LaunchRequest>, String>> =
        OnceLock::new();
    let launcher = LAUNCHER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<LaunchRequest>();
        std::thread::Builder::new()
            .name("cua-compositor-launcher".into())
            .spawn(move || {
                while let Ok((mut command, reply)) = receiver.recv() {
                    let _ = reply.send(command.spawn());
                }
            })
            .map(|_| sender)
            .map_err(|error| error.to_string())
    });
    let launcher = launcher.as_ref().map_err(|error| {
        std::io::Error::other(format!(
            "could not start compositor launcher thread: {error}"
        ))
    })?;
    let (reply, result) = mpsc::channel();
    launcher.send((command, reply)).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "compositor launcher thread exited",
        )
    })?;
    result.recv().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "compositor launcher dropped its result",
        )
    })?
}

/// Static properties of the low-level Linux workspace backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NestedCompositorCapabilities {
    pub visual_separation: bool,
    pub separate_display_server: bool,
    pub host_focus_isolation: bool,
    /// A nested display is a lifecycle boundary, not a security sandbox.
    pub filesystem_isolation: bool,
    pub network_isolation: bool,
    pub process_isolation: bool,
    pub launch: bool,
    pub capture: bool,
    pub input: bool,
    /// The compositor provides cua's private, target-addressable input socket.
    pub direct_surface_input: bool,
    /// Existing host applications cannot be adopted into the nested session.
    pub move_existing: bool,
    /// Applications already running outside the workspace must be relaunched.
    pub relaunch_required: bool,
}

/// Current state of the compositor child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NestedCompositorStatus {
    Running,
    Exited(Option<i32>),
    Stopped,
}

/// Configuration for a nested compositor workspace.
#[derive(Clone, Debug)]
pub struct NestedCompositorConfig {
    compositor: OsString,
    runtime_root: PathBuf,
    startup_timeout: Duration,
    shutdown_timeout: Duration,
}

impl NestedCompositorConfig {
    /// Use `cua-compositor` and the current user's Wayland runtime root.
    pub fn cua_compositor() -> Self {
        Self::new("cua-compositor")
    }

    /// Use an explicitly selected wlroots compositor executable.
    pub fn new(compositor: impl Into<OsString>) -> Self {
        Self {
            compositor: compositor.into(),
            runtime_root: default_runtime_root(),
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }

    pub fn runtime_root(mut self, runtime_root: impl Into<PathBuf>) -> Self {
        self.runtime_root = runtime_root.into();
        self
    }

    pub fn startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }
}

impl Default for NestedCompositorConfig {
    fn default() -> Self {
        Self::cua_compositor()
    }
}

/// A running, independently owned headless Wayland session.
///
/// Dropping this value terminates the compositor and attempts to reap it without
/// waiting beyond the configured shutdown bound. It never changes
/// `WAYLAND_DISPLAY`, `DISPLAY`, or any other environment variable of the
/// owning process.
pub struct NestedCompositorWorkspace {
    child: Option<Child>,
    compositor: OsString,
    runtime_dir: PathBuf,
    wayland_socket: PathBuf,
    control_socket: Option<PathBuf>,
    shutdown_timeout: Duration,
    status: NestedCompositorStatus,
}

impl NestedCompositorWorkspace {
    /// Launch the default `cua-compositor` backend.
    pub fn launch() -> Result<Self> {
        Self::launch_with_config(NestedCompositorConfig::default())
    }

    /// Launch a specified compositor using the standard headless wlroots env.
    pub fn launch_with_compositor(compositor: impl Into<OsString>) -> Result<Self> {
        Self::launch_with_config(NestedCompositorConfig::new(compositor))
    }

    pub fn launch_with_config(config: NestedCompositorConfig) -> Result<Self> {
        let runtime_dir = create_runtime_dir(&config.runtime_root)?;
        let inject_capable = is_cua_compositor(&config.compositor);
        let control_socket = inject_capable.then(|| runtime_dir.join(CONTROL_SOCKET_NAME));

        let mut command = Command::new(&config.compositor);
        command
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "pixman")
            .env("WLR_RENDERER_ALLOW_SOFTWARE", "1")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("WLR_HEADLESS_OUTPUTS", "1")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(path) = control_socket.as_ref() {
            command.env("CUA_INJECT_SOCKET", path);
        } else {
            command.env_remove("CUA_INJECT_SOCKET");
        }
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Close the fork→prctl race: if the daemon already died,
                // refuse to exec an orphan compositor.
                if libc::getppid() == 1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "cua-driver parent exited during compositor startup",
                    ));
                }
                Ok(())
            });
        }

        let child = match spawn_compositor(command) {
            Ok(child) => child,
            Err(error) => {
                cleanup_runtime_dir(&runtime_dir, None, control_socket.as_deref());
                return Err(error).with_context(|| {
                    format!("failed to spawn nested compositor {:?}", config.compositor)
                });
            }
        };

        let mut workspace = Self {
            child: Some(child),
            compositor: config.compositor,
            runtime_dir,
            wayland_socket: PathBuf::new(),
            control_socket,
            shutdown_timeout: config.shutdown_timeout,
            status: NestedCompositorStatus::Running,
        };

        match workspace.wait_until_ready(config.startup_timeout) {
            Ok(socket) => {
                workspace.wayland_socket = socket;
                Ok(workspace)
            }
            Err(error) => {
                let _ = workspace.shutdown();
                Err(error)
            }
        }
    }

    fn wait_until_ready(&mut self, timeout: Duration) -> Result<PathBuf> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait_child()? {
                bail!(
                    "nested compositor {:?} exited before becoming ready ({})",
                    self.compositor,
                    display_exit_status(status)
                );
            }

            let sockets = wayland_sockets(&self.runtime_dir);
            if let Some(socket) = select_wayland_socket(&HashSet::new(), &sockets) {
                let control_ready = self
                    .control_socket
                    .as_ref()
                    .is_none_or(|path| is_unix_socket(path));
                if control_ready {
                    return Ok(self.runtime_dir.join(socket));
                }
            }

            if Instant::now() >= deadline {
                if self.control_socket.is_some() && !sockets.is_empty() {
                    bail!(
                        "nested compositor {:?} did not expose its control socket within {:?}",
                        self.compositor,
                        timeout
                    );
                }
                bail!(
                    "nested compositor {:?} did not expose a Wayland socket within {:?}",
                    self.compositor,
                    timeout
                );
            }
            std::thread::sleep(POLL_INTERVAL.min(timeout));
        }
    }

    /// Environment entries needed by applications, capture helpers, and driver
    /// processes that should connect to this workspace.
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut environment = vec![
            (
                OsString::from("XDG_RUNTIME_DIR"),
                self.runtime_dir.as_os_str().to_owned(),
            ),
            (
                OsString::from("WAYLAND_DISPLAY"),
                self.wayland_socket.as_os_str().to_owned(),
            ),
            (
                OsString::from("CUA_DRIVER_RS_ENABLE_WAYLAND"),
                OsString::from("1"),
            ),
        ];
        if let Some(path) = self.control_socket.as_ref() {
            environment.push((
                OsString::from("CUA_INJECT_SOCKET"),
                path.as_os_str().to_owned(),
            ));
        }
        environment
    }

    /// Apply the workspace environment to one child without mutating the
    /// process-global environment. `DISPLAY` is removed to prevent accidental
    /// fallback to the host X server.
    pub fn configure_command(&self, command: &mut Command) {
        command.env_remove("DISPLAY");
        for (key, value) in self.environment() {
            command.env(key, value);
        }
    }

    pub fn wayland_socket(&self) -> &Path {
        &self.wayland_socket
    }

    pub fn control_socket(&self) -> Option<&Path> {
        self.control_socket.as_deref()
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn compositor(&self) -> &OsStr {
        &self.compositor
    }

    pub fn compositor_pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    pub fn capabilities(&self) -> NestedCompositorCapabilities {
        capabilities_for(self.control_socket.is_some())
    }

    pub fn status(&mut self) -> Result<NestedCompositorStatus> {
        if self.child.is_none() {
            return Ok(self.status);
        }
        match self.try_wait_child()? {
            Some(status) => Ok(NestedCompositorStatus::Exited(status.code())),
            None => Ok(NestedCompositorStatus::Running),
        }
    }

    /// Terminate and reap the compositor without waiting beyond the configured
    /// shutdown bound.
    pub fn shutdown(&mut self) -> Result<()> {
        let Some(mut child) = self.child.take() else {
            self.status = NestedCompositorStatus::Stopped;
            self.cleanup();
            return Ok(());
        };

        match child.try_wait() {
            Ok(Some(_)) => {
                self.status = NestedCompositorStatus::Stopped;
                self.cleanup();
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                self.child = Some(child);
                return Err(error).context("query nested compositor status");
            }
        }

        // Give the compositor a bounded graceful-exit window first so it can
        // release Wayland clients and sockets. Escalate to SIGKILL only after
        // that window expires.
        let term_result = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        if term_result != 0 {
            let signal_error = std::io::Error::last_os_error();
            if child.try_wait().ok().flatten().is_none() {
                self.child = Some(child);
                return Err(signal_error).context("signal nested compositor");
            }
        }
        let deadline = Instant::now() + self.shutdown_timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.status = NestedCompositorStatus::Stopped;
                    self.cleanup();
                    return Ok(());
                }
                Ok(None) => {}
                Err(error) => {
                    self.child = Some(child);
                    return Err(error).context("reap nested compositor");
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL_INTERVAL.min(self.shutdown_timeout));
        }

        child
            .kill()
            .context("force-terminate nested compositor after SIGTERM timeout")?;
        let kill_deadline = Instant::now() + self.shutdown_timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.status = NestedCompositorStatus::Stopped;
                    self.cleanup();
                    return Ok(());
                }
                Ok(None) => {}
                Err(error) => {
                    self.child = Some(child);
                    return Err(error).context("reap force-terminated nested compositor");
                }
            }
            if Instant::now() >= kill_deadline {
                self.child = Some(child);
                bail!(
                    "nested compositor did not exit within {:?} after SIGKILL",
                    self.shutdown_timeout
                );
            }
            std::thread::sleep(POLL_INTERVAL.min(self.shutdown_timeout));
        }
    }

    fn try_wait_child(&mut self) -> Result<Option<ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        let status = child.try_wait().context("query nested compositor status")?;
        if let Some(status) = status {
            self.status = NestedCompositorStatus::Exited(status.code());
            self.child = None;
        }
        Ok(status)
    }

    fn cleanup(&self) {
        cleanup_runtime_dir(
            &self.runtime_dir,
            (!self.wayland_socket.as_os_str().is_empty()).then_some(self.wayland_socket.as_path()),
            self.control_socket.as_deref(),
        );
    }
}

impl Drop for NestedCompositorWorkspace {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            tracing::warn!("failed to shut down nested compositor: {error:#}");
        }
    }
}

/// cua-driver adapter that owns one nested compositor per public workspace.
#[derive(Default)]
pub struct LinuxWorkspaceBackend {
    workspaces: Mutex<HashMap<String, NestedCompositorWorkspace>>,
}

impl LinuxWorkspaceBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn public_capabilities(_direct_surface_input: bool) -> WorkspaceCapabilities {
        WorkspaceCapabilities {
            create: true,
            attach: false,
            close: true,
            visual_separation: true,
            separate_display_server: true,
            host_focus_isolation: true,
            launch: true,
            move_existing_window: false,
            // Launch routing is workspace-scoped today. Capture and input
            // still consult daemon-global Wayland environment, so claiming
            // them here would risk operating on the host compositor.
            capture: false,
            input: false,
            filesystem_isolation: false,
            network_isolation: false,
            process_isolation: false,
            security_isolation: false,
        }
    }
}

#[async_trait]
impl WorkspaceBackend for LinuxWorkspaceBackend {
    fn platform(&self) -> &'static str {
        "linux"
    }

    fn descriptors(&self) -> Vec<WorkspaceBackendDescriptor> {
        vec![WorkspaceBackendDescriptor {
            kind: WorkspaceKind::NestedCompositor,
            available: executable_on_path(OsStr::new("cua-compositor")),
            experimental: true,
            capabilities: Self::public_capabilities(true),
            detail: Some(
                "Creates a per-workspace headless Wayland compositor; existing clients must be relaunched. This is not filesystem, network, or process isolation."
                    .into(),
            ),
        }]
    }

    async fn create(
        &self,
        workspace_id: &str,
        request: &CreateWorkspaceRequest,
    ) -> std::result::Result<CreatedWorkspace, WorkspaceError> {
        if !matches!(
            request.kind,
            WorkspaceKind::Auto | WorkspaceKind::NestedCompositor
        ) {
            return Err(WorkspaceError::Unsupported(
                "Linux currently implements nested_compositor workspaces".into(),
            ));
        }
        if request.native_id.is_some() {
            return Err(WorkspaceError::Unsupported(
                "nested_compositor cannot attach to a native_id".into(),
            ));
        }
        let compositor = request
            .options
            .get("compositor")
            .and_then(|value| value.as_str())
            .unwrap_or("cua-compositor");
        let compositor = compositor.to_owned();
        let workspace = tokio::task::spawn_blocking(move || {
            NestedCompositorWorkspace::launch_with_compositor(compositor)
        })
        .await
        .map_err(|error| WorkspaceError::Backend(format!("compositor task failed: {error}")))?
        .map_err(|error| WorkspaceError::Backend(format!("{error:#}")))?;
        let capabilities = Self::public_capabilities(workspace.control_socket().is_some());
        let detail = json!({
            "wayland_socket": workspace.wayland_socket(),
            "control_socket": workspace.control_socket(),
            "compositor_pid": workspace.compositor_pid(),
            "relaunch_required": true,
        });
        let mut workspaces = self.workspaces.lock().unwrap();
        match workspaces.entry(workspace_id.to_owned()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(workspace);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(WorkspaceError::Backend(format!(
                    "duplicate workspace id '{workspace_id}'"
                )));
            }
        }
        Ok(CreatedWorkspace {
            kind: WorkspaceKind::NestedCompositor,
            adopted: false,
            native_id: None,
            capabilities,
            detail,
        })
    }

    async fn close(
        &self,
        workspace_id: &str,
        launched_pids: &[u32],
    ) -> std::result::Result<(), WorkspaceError> {
        let Some(workspace) = self.workspaces.lock().unwrap().remove(workspace_id) else {
            return Err(WorkspaceError::NotFound(workspace_id.to_owned()));
        };
        let runtime_dir = workspace.runtime_dir().to_owned();
        let launched_pids = launched_pids.to_vec();
        let (workspace, shutdown_result) = tokio::task::spawn_blocking(move || {
            let mut workspace = workspace;
            let result = terminate_owned_processes(&runtime_dir, &launched_pids)
                .and_then(|()| workspace.shutdown());
            (workspace, result)
        })
        .await
        .map_err(|error| WorkspaceError::Backend(format!("shutdown task failed: {error}")))?;
        if let Err(error) = shutdown_result {
            self.workspaces
                .lock()
                .unwrap()
                .insert(workspace_id.to_owned(), workspace);
            return Err(WorkspaceError::Backend(format!("{error:#}")));
        }
        Ok(())
    }

    async fn move_window(
        &self,
        _workspace_id: &str,
        _target: &WindowTarget,
    ) -> std::result::Result<serde_json::Value, WorkspaceError> {
        Err(WorkspaceError::Unsupported(
            "Wayland clients cannot move between compositors; relaunch the application with workspace_id"
                .into(),
        ))
    }

    fn configure_command(
        &self,
        workspace_id: &str,
        command: &mut Command,
    ) -> std::result::Result<(), WorkspaceError> {
        let mut workspaces = self.workspaces.lock().unwrap();
        let workspace = workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| WorkspaceError::NotFound(workspace_id.to_owned()))?;
        match workspace
            .status()
            .map_err(|error| WorkspaceError::Backend(format!("{error:#}")))?
        {
            NestedCompositorStatus::Running => {}
            status => {
                return Err(WorkspaceError::Backend(format!(
                    "nested compositor is not running: {status:?}"
                )))
            }
        }
        workspace.configure_command(command);
        Ok(())
    }
}

fn terminate_owned_processes(runtime_dir: &Path, pids: &[u32]) -> Result<()> {
    let candidates: Vec<i32> = pids
        .iter()
        .filter_map(|pid| i32::try_from(*pid).ok())
        .collect();
    // Reap every recorded direct child before checking its environment. A
    // zombie has already lost /proc/<pid>/environ, so filtering first would
    // silently leave an exited workspace child in the daemon's process table.
    // waitpid(WNOHANG) is harmless for a live child or a pid we do not own.
    reap_children(&candidates);
    let owned: Vec<i32> = candidates
        .into_iter()
        .filter(|pid| process_uses_runtime(*pid, runtime_dir))
        .collect();
    for pid in &owned {
        unsafe {
            libc::kill(*pid, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + DEFAULT_SHUTDOWN_TIMEOUT;
    loop {
        reap_children(&owned);
        if owned.iter().all(|pid| !process_exists(*pid)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    for pid in &owned {
        if process_exists(*pid) {
            unsafe {
                libc::kill(*pid, libc::SIGKILL);
            }
        }
    }
    let kill_deadline = Instant::now() + DEFAULT_SHUTDOWN_TIMEOUT;
    loop {
        reap_children(&owned);
        if owned.iter().all(|pid| !process_exists(*pid)) {
            return Ok(());
        }
        if Instant::now() >= kill_deadline {
            let remaining: Vec<_> = owned
                .iter()
                .copied()
                .filter(|pid| process_exists(*pid))
                .collect();
            bail!("workspace processes did not exit after SIGKILL: {remaining:?}");
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn process_uses_runtime(pid: i32, runtime_dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(environment) = fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let prefix = b"XDG_RUNTIME_DIR=";
    environment.split(|byte| *byte == 0).any(|entry| {
        entry
            .strip_prefix(prefix)
            .is_some_and(|value| value == runtime_dir.as_os_str().as_bytes())
    })
}

fn process_exists(pid: i32) -> bool {
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn reap_children(pids: &[i32]) {
    for pid in pids {
        unsafe {
            libc::waitpid(*pid, std::ptr::null_mut(), libc::WNOHANG);
        }
    }
}

fn executable_on_path(executable: &OsStr) -> bool {
    if Path::new(executable).components().count() > 1 {
        return Path::new(executable).is_file();
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|directory| directory.join(executable).is_file())
}

fn default_runtime_root() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn create_runtime_dir(root: &Path) -> Result<PathBuf> {
    for _ in 0..32 {
        let sequence = WORKSPACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = root.join(format!(
            "cua-workspace-{}-{nanos:x}-{sequence:x}",
            std::process::id()
        ));
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create workspace runtime directory {path:?}"));
            }
        }
    }
    bail!("could not allocate a unique workspace runtime directory")
}

fn wayland_sockets(dir: &Path) -> HashSet<OsString> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            is_wayland_socket_name(&name).then_some(name)
        })
        .collect()
}

fn is_wayland_socket_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name.strip_prefix("wayland-").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn select_wayland_socket(
    before: &HashSet<OsString>,
    after: &HashSet<OsString>,
) -> Option<OsString> {
    after.difference(before).min().cloned()
}

fn is_cua_compositor(executable: &OsStr) -> bool {
    Path::new(executable)
        .file_name()
        .is_some_and(|name| name == "cua-compositor")
}

fn capabilities_for(direct_surface_input: bool) -> NestedCompositorCapabilities {
    NestedCompositorCapabilities {
        visual_separation: true,
        separate_display_server: true,
        host_focus_isolation: true,
        filesystem_isolation: false,
        network_isolation: false,
        process_isolation: false,
        launch: true,
        capture: true,
        input: true,
        direct_surface_input,
        move_existing: false,
        relaunch_required: true,
    }
}

fn is_unix_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
}

fn display_exit_status(status: ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("exit code {code}"))
        .unwrap_or_else(|| "terminated by signal".to_owned())
}

fn cleanup_runtime_dir(runtime_dir: &Path, wayland_socket: Option<&Path>, control: Option<&Path>) {
    if let Some(path) = control {
        let _ = fs::remove_file(path);
    }
    if let Some(path) = wayland_socket {
        let _ = fs::remove_file(path);
        let lock_path = path.with_file_name(format!(
            "{}.lock",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        let _ = fs::remove_file(lock_path);
    }
    let chromium_profile = runtime_dir.join(CHROMIUM_PROFILE_DIR_NAME);
    // This is the only recursive cleanup target. Construct it from the
    // already-owned runtime directory and verify the exact basename/parent so
    // a malformed environment or symlink value can never broaden the target.
    if chromium_profile.parent() == Some(runtime_dir)
        && chromium_profile.file_name() == Some(OsStr::new(CHROMIUM_PROFILE_DIR_NAME))
    {
        let _ = fs::remove_dir_all(chromium_profile);
    }
    if let Err(error) = fs::remove_dir(runtime_dir) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                "workspace runtime directory {:?} was not empty after scoped cleanup: {error}",
                runtime_dir
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(values: &[&str]) -> HashSet<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn accepts_only_numbered_wayland_socket_names() {
        assert!(is_wayland_socket_name(OsStr::new("wayland-0")));
        assert!(is_wayland_socket_name(OsStr::new("wayland-42")));
        assert!(!is_wayland_socket_name(OsStr::new("wayland-")));
        assert!(!is_wayland_socket_name(OsStr::new("wayland-a")));
        assert!(!is_wayland_socket_name(OsStr::new("wayland-1.lock")));
        assert!(!is_wayland_socket_name(OsStr::new("x11-0")));
    }

    #[test]
    fn socket_discovery_ignores_preexisting_names_and_is_deterministic() {
        let before = names(&["wayland-0", "wayland-4"]);
        let after = names(&["wayland-0", "wayland-2", "wayland-4", "wayland-9"]);
        assert_eq!(
            select_wayland_socket(&before, &after),
            Some(OsString::from("wayland-2"))
        );
    }

    #[test]
    fn recognizes_cua_compositor_by_executable_basename() {
        assert!(is_cua_compositor(OsStr::new("cua-compositor")));
        assert!(is_cua_compositor(OsStr::new(
            "/nix/store/example/bin/cua-compositor"
        )));
        assert!(!is_cua_compositor(OsStr::new("labwc")));
    }

    #[test]
    fn config_defaults_to_cua_compositor_and_bounded_timeouts() {
        let config = NestedCompositorConfig::default();
        assert_eq!(config.compositor, OsStr::new("cua-compositor"));
        assert_eq!(config.startup_timeout, DEFAULT_STARTUP_TIMEOUT);
        assert_eq!(config.shutdown_timeout, DEFAULT_SHUTDOWN_TIMEOUT);
    }

    #[test]
    fn capabilities_require_relaunch_and_never_move_existing_apps() {
        let capabilities = capabilities_for(true);
        assert!(capabilities.visual_separation);
        assert!(capabilities.separate_display_server);
        assert!(capabilities.host_focus_isolation);
        assert!(!capabilities.filesystem_isolation);
        assert!(!capabilities.network_isolation);
        assert!(!capabilities.process_isolation);
        assert!(capabilities.launch);
        assert!(capabilities.capture);
        assert!(capabilities.input);
        assert!(capabilities.direct_surface_input);
        assert!(!capabilities.move_existing);
        assert!(capabilities.relaunch_required);
    }

    #[test]
    fn terminate_owned_processes_reaps_an_already_exited_recorded_child() {
        let child = Command::new("/bin/true").spawn().unwrap();
        let pid = child.id();
        drop(child);

        let deadline = Instant::now() + Duration::from_secs(2);
        while process_exists(pid as i32) && Instant::now() < deadline {
            let state = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            if state.contains(") Z ") {
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        terminate_owned_processes(Path::new("/cua-test-runtime-does-not-exist"), &[pid]).unwrap();
        let waited = unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) };
        assert_eq!(waited, -1, "the exited child should already be reaped");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}
