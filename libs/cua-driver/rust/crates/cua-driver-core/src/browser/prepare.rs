//! Acting `browser_prepare` implementation for driver-owned Chromium profiles.

use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::engine::unsupported_engine_refusal;
use super::platform::{
    BrowserConsentOutcome, BrowserConsentRequest, ExistingProfileSetupOutcome,
    ExistingProfileSetupRequest, PrepareAction, PrepareAttachment, PrepareAttachmentKind,
    PrepareLaunchPosture, PrepareOutcome, PrepareProfile, PrepareProfileMode, PrepareRequest,
    PrepareSideEffects, PrepareStrategy,
};
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::types::{
    BrowserEngineFamily, EndpointOwnershipMethod, EndpointOwnershipProof, OwnedEndpoint,
};
use super::BrowserEngine;

const PROFILE_MARKER: &str = ".cua-driver-owned-profile.json";
const PROFILE_SCHEMA: &str = "cua-driver-browser-profile-v1";

fn validate_profile(profile: &PrepareProfile) -> Result<(), BrowserRefusal> {
    match profile.mode {
        PrepareProfileMode::IsolatedNew => {
            if profile.name.is_some() {
                return Err(refusal(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "profile.name is not valid with mode=isolated_new",
                ));
            }
        }
        PrepareProfileMode::IsolatedNamed => {
            let Some(name) = profile.name.as_deref() else {
                return Err(refusal(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "profile.name is required with mode=isolated_named",
                ));
            };
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(refusal(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "profile.name must be 1-64 ASCII letters, digits, '-' or '_'",
                ));
            }
        }
    }
    Ok(())
}

async fn claim_with_optional_consent<T, Claim, Consent>(
    claim: &mut Pin<Box<Claim>>,
    consent: Consent,
) -> Result<(anyhow::Result<T>, bool), BrowserRefusal>
where
    Claim: Future<Output = anyhow::Result<T>>,
    Consent: Future<Output = Result<BrowserConsentOutcome, BrowserRefusal>>,
{
    let mut consent = Box::pin(consent);
    tokio::select! {
        result = claim.as_mut() => Ok((result, false)),
        outcome = consent.as_mut() => match outcome? {
            BrowserConsentOutcome::Accepted => Ok((claim.as_mut().await, true)),
            BrowserConsentOutcome::NotPresent => Ok((claim.as_mut().await, false)),
        },
    }
}

async fn retry_claim_after_accepted_consent<T, Retry>(
    initial: anyhow::Result<T>,
    accepted_consent: bool,
    retry: Retry,
) -> (anyhow::Result<T>, Option<anyhow::Error>)
where
    Retry: Future<Output = anyhow::Result<T>>,
{
    match initial {
        Ok(value) => (Ok(value), None),
        Err(initial_error) if accepted_consent => (retry.await, Some(initial_error)),
        Err(error) => (Err(error), None),
    }
}

fn with_setup_side_effects(
    mut error: BrowserRefusal,
    setup: &ExistingProfileSetupOutcome,
) -> BrowserRefusal {
    if !setup.opened_setup_page
        && !setup.closed_setup_page
        && !setup.enabled_remote_debugging
        && !setup.used_bounded_pixel_fallback
        && !setup.focused_setup_address_field
        && !setup.foregrounded_window
        && !setup.injected_global_input
    {
        return error;
    }
    let cause = error.detail.take();
    error.detail = Some(serde_json::json!({
        "setup_side_effects": {
            "opened_setup_page": setup.opened_setup_page,
            "closed_setup_page": setup.closed_setup_page,
            "focused_setup_address_field": setup.focused_setup_address_field,
            "enabled_remote_debugging": setup.enabled_remote_debugging,
            "used_bounded_pixel_fallback": setup.used_bounded_pixel_fallback,
            "foregrounded_window": setup.foregrounded_window,
            "injected_global_input": setup.injected_global_input,
        },
        "cause": cause,
    }));
    error
}

fn with_prepare_side_effects(
    mut error: BrowserRefusal,
    setup: &ExistingProfileSetupOutcome,
    displayed_consent_prompt: bool,
) -> BrowserRefusal {
    error = with_setup_side_effects(error, setup);
    if !displayed_consent_prompt {
        return error;
    }
    let mut detail = match error.detail.take() {
        Some(serde_json::Value::Object(detail)) => detail,
        Some(cause) => {
            let mut detail = serde_json::Map::new();
            detail.insert("cause".to_owned(), cause);
            detail
        }
        None => serde_json::Map::new(),
    };
    detail.insert(
        "displayed_consent_prompt".to_owned(),
        serde_json::Value::Bool(true),
    );
    error.detail = Some(serde_json::Value::Object(detail));
    error
}

struct PendingExistingProfileSetup {
    platform: Arc<dyn super::platform::BrowserPlatform>,
    request: Option<ExistingProfileSetupRequest>,
}

impl PendingExistingProfileSetup {
    fn new(
        platform: Arc<dyn super::platform::BrowserPlatform>,
        request: ExistingProfileSetupRequest,
    ) -> Self {
        Self {
            platform,
            request: Some(request),
        }
    }

    async fn abort(&mut self, error: BrowserRefusal) -> BrowserRefusal {
        let Some(request) = self.request.clone() else {
            return error;
        };
        let result = self
            .platform
            .abort_existing_profile_setup(request, error)
            .await;
        self.request = None;
        result
    }

    async fn commit(&mut self) -> Result<bool, BrowserRefusal> {
        let request = self.request.clone().ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                "the pending browser setup was already finalized",
            )
        })?;
        let result = self.platform.commit_existing_profile_setup(request).await;
        if result.is_ok() {
            self.request = None;
        }
        result
    }
}

impl Drop for PendingExistingProfileSetup {
    fn drop(&mut self) {
        let Some(request) = self.request.take() else {
            return;
        };
        let platform = self.platform.clone();
        let error = refusal(
            BrowserRefusalCode::BrowserBindingStale,
            "the existing-profile setup request ended before commit; rolling back its exact pending setup",
        );
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = platform.abort_existing_profile_setup(request, error).await;
            });
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ProfileMarker {
    schema: String,
    mode: PrepareProfileMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

struct PreparedProfile {
    path: PathBuf,
    created: bool,
    delete_on_cleanup: bool,
    marker: ProfileMarker,
}

pub(crate) struct ManagedBrowser {
    child: Child,
    process_job: BrowserProcessJob,
    owned_pid: i64,
    profile: PathBuf,
    delete_profile: bool,
    marker: ProfileMarker,
    owner_sessions: Vec<String>,
}

impl Drop for ManagedBrowser {
    fn drop(&mut self) {
        let process_tree_stopped = terminate_browser_process_tree(
            &mut self.child,
            Some(self.owned_pid),
            &mut self.process_job,
        );
        if process_tree_stopped
            && self.delete_profile
            && profile_matches_marker(&self.profile, &self.marker)
        {
            let _ = fs::remove_dir_all(&self.profile);
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsBrowserJob {
    handle: std::os::windows::io::OwnedHandle,
}

#[cfg(target_os = "windows")]
impl WindowsBrowserJob {
    fn new() -> Result<Self, BrowserRefusal> {
        use core::ffi::c_void;
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        unsafe {
            let handle =
                CreateJobObjectW(None, windows::core::PCWSTR::null()).map_err(|error| {
                    refusal(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        format!("could not create the isolated-browser Windows job: {error}"),
                    )
                })?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if let Err(error) = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
                return Err(refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not configure the isolated-browser Windows job: {error}"),
                ));
            }
            Ok(Self {
                handle: OwnedHandle::from_raw_handle(handle.0 as *mut c_void),
            })
        }
    }

    fn raw(&self) -> windows::Win32::Foundation::HANDLE {
        use core::ffi::c_void;
        use std::os::windows::io::AsRawHandle;

        windows::Win32::Foundation::HANDLE(self.handle.as_raw_handle() as *mut c_void)
    }

    fn assign(&self, child: &Child) -> Result<(), BrowserRefusal> {
        use core::ffi::c_void;
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;

        let process = HANDLE(child.as_raw_handle() as *mut c_void);
        unsafe { AssignProcessToJobObject(self.raw(), process) }.map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!(
                    "could not assign isolated browser {} to its Windows lifecycle job: {error}",
                    child.id()
                ),
            )
        })
    }

    fn resume_suspended_child(&self, child: &Child) -> Result<(), BrowserRefusal> {
        use core::ffi::c_void;
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        };
        use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not enumerate the suspended isolated-browser thread: {error}"),
                )
            })?;
            let _snapshot = OwnedHandle::from_raw_handle(snapshot.0 as *mut c_void);
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..THREADENTRY32::default()
            };
            let mut thread_ids = Vec::new();
            if Thread32First(snapshot, &mut entry).is_ok() {
                loop {
                    if entry.th32OwnerProcessID == child.id() {
                        thread_ids.push(entry.th32ThreadID);
                    }
                    if Thread32Next(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            if thread_ids.len() != 1 {
                return Err(refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    "the isolated browser did not expose exactly one suspended primary thread",
                ));
            }
            let thread =
                OpenThread(THREAD_SUSPEND_RESUME, false, thread_ids[0]).map_err(|error| {
                    refusal(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        format!("could not open the suspended isolated-browser thread: {error}"),
                    )
                })?;
            let _thread = OwnedHandle::from_raw_handle(thread.0 as *mut c_void);
            if ResumeThread(thread) != 1 {
                return Err(refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    "the isolated-browser thread did not have the expected suspended state",
                ));
            }
        }
        Ok(())
    }

    fn terminate_and_wait(&self) -> bool {
        use core::ffi::c_void;
        use windows::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject, TerminateJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };

        // This remains effective after a short-lived launcher exits because
        // every non-breakaway descendant stays in the job.
        if unsafe { TerminateJobObject(self.raw(), 1) }.is_err() {
            return false;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            let queried = unsafe {
                QueryInformationJobObject(
                    self.raw(),
                    JobObjectBasicAccountingInformation,
                    &mut accounting as *mut _ as *mut c_void,
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    None,
                )
            };
            if queried.is_ok() && accounting.ActiveProcesses == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(target_os = "windows")]
type BrowserProcessJob = Option<WindowsBrowserJob>;

#[cfg(not(target_os = "windows"))]
type BrowserProcessJob = ();

pub(crate) type ManagedBrowsers = Mutex<Vec<ManagedBrowser>>;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn profile_root() -> Result<PathBuf, BrowserRefusal> {
    if let Some(root) = std::env::var_os("CUA_DRIVER_BROWSER_PROFILE_ROOT") {
        return Ok(PathBuf::from(root));
    }
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("CuaDriver").join("BrowserProfiles"));
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME").map(PathBuf::from).map(|path| {
        path.join("Library")
            .join("Application Support")
            .join("CuaDriver")
            .join("BrowserProfiles")
    });
    #[cfg(all(unix, not(target_os = "macos")))]
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".local").join("state"))
        })
        .map(|path| path.join("cua-driver").join("browser-profiles"));
    root.ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "could not resolve the current user's driver-owned profile directory",
        )
    })
}

fn read_profile_marker(path: &Path) -> Option<ProfileMarker> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return None;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    let Ok(bytes) = fs::read(path.join(PROFILE_MARKER)) else {
        return None;
    };
    serde_json::from_slice::<ProfileMarker>(&bytes)
        .ok()
        .filter(|marker| marker.schema == PROFILE_SCHEMA)
}

fn profile_matches_marker(path: &Path, expected: &ProfileMarker) -> bool {
    read_profile_marker(path).is_some_and(|marker| marker == *expected)
}

fn cleanup_created_profile(profile: &PreparedProfile) {
    if profile.delete_on_cleanup && profile_matches_marker(&profile.path, &profile.marker) {
        let _ = fs::remove_dir_all(&profile.path);
    }
}

fn terminate_browser_process_tree(
    child: &mut Child,
    owned_pid: Option<i64>,
    process_job: &mut BrowserProcessJob,
) -> bool {
    #[cfg(not(target_os = "windows"))]
    let _ = (owned_pid, process_job);
    #[cfg(unix)]
    unsafe {
        // The isolated browser is spawned as its own process group. Chrome
        // fans out into renderer/utility descendants, so killing only the
        // root Child can leave profile writers alive after cleanup.
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(target_os = "windows")]
    {
        let had_job = process_job.is_some();
        let job_drained = process_job
            .as_ref()
            .map(WindowsBrowserJob::terminate_and_wait);
        if !had_job {
            // Standard posture predates the Job Object route and retains its
            // exact launcher/runtime tree cleanup. The fixed-port route never
            // falls back to raw PIDs after its job has drained.
            let child_pid = i64::from(child.id());
            for pid in [Some(child_pid), owned_pid.filter(|pid| *pid != child_pid)]
                .into_iter()
                .flatten()
            {
                let _ = Command::new("taskkill.exe")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        let _ = child.kill();
        let child_reaped = child.wait().is_ok();
        if job_drained == Some(true) {
            let _ = process_job.take();
        }
        job_drained.unwrap_or(child_reaped)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = child.kill();
        let child_reaped = child.wait().is_ok();
        child_reaped
    }
}

fn spawn_isolated_browser_process(
    command: &mut Command,
    launch_posture: PrepareLaunchPosture,
    prepared_profile: &PreparedProfile,
) -> Result<(Child, BrowserProcessJob), BrowserRefusal> {
    #[cfg(target_os = "windows")]
    let process_job = match launch_posture {
        PrepareLaunchPosture::DriverSelectedPort => match WindowsBrowserJob::new() {
            Ok(job) => Some(job),
            Err(error) => {
                cleanup_created_profile(prepared_profile);
                return Err(error);
            }
        },
        PrepareLaunchPosture::Standard => None,
    };
    #[cfg(not(target_os = "windows"))]
    let process_job = ();
    #[cfg(not(target_os = "windows"))]
    let _ = launch_posture;

    #[allow(unused_mut)] // mutable only on Windows assignment/resume failures
    let mut child = command.spawn().map_err(|error| {
        cleanup_created_profile(prepared_profile);
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not launch an isolated browser process: {error}"),
        )
    })?;

    #[cfg(target_os = "windows")]
    if let Some(job) = process_job.as_ref() {
        if let Err(error) = job
            .assign(&child)
            .and_then(|()| job.resume_suspended_child(&child))
        {
            // The browser is still suspended until ownership is installed and
            // its only primary thread is resumed, so no handoff descendant can
            // escape this failure path.
            let process_tree_stopped = job.terminate_and_wait();
            let _ = child.kill();
            let child_reaped = child.wait().is_ok();
            if process_tree_stopped && child_reaped {
                cleanup_created_profile(prepared_profile);
            }
            return Err(error);
        }
    }

    Ok((child, process_job))
}

#[cfg(target_os = "linux")]
fn configure_linux_isolated_browser_command(
    command: &mut Command,
    native_wayland: bool,
    no_sandbox: bool,
) {
    command.arg("--password-store=basic");
    if native_wayland {
        command.arg("--ozone-platform=wayland");
    }
    if no_sandbox {
        command.arg("--no-sandbox");
    }
}

fn reserve_driver_selected_debugging_port() -> Result<u16, BrowserRefusal> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not reserve a driver-selected loopback DevTools port: {error}"),
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not inspect the reserved loopback DevTools port: {error}"),
            )
        })?
        .port();
    drop(listener);
    if port == 0 {
        return Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "the reserved loopback DevTools port was not usable",
        ));
    }
    Ok(port)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnedEndpointHint {
    ProfilePortFile,
    FixedLoopbackPort(u16),
}

struct IsolatedBrowserCommand {
    command: Command,
    endpoint_hint: SpawnedEndpointHint,
}

fn isolated_browser_command(
    executable: &str,
    profile: &Path,
    launch_posture: PrepareLaunchPosture,
) -> Result<IsolatedBrowserCommand, BrowserRefusal> {
    let mut command = Command::new(executable);
    #[cfg(target_os = "windows")]
    if launch_posture == PrepareLaunchPosture::DriverSelectedPort {
        use std::os::windows::process::CommandExt;
        use windows::Win32::System::Threading::CREATE_SUSPENDED;

        // The fixed-port posture needs a per-browser Job Object. Start the
        // launcher suspended so it cannot hand off descendants before core
        // assigns it to that lifecycle owner.
        command.creation_flags(CREATE_SUSPENDED.0);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let (remote_debugging_port, endpoint_hint) = match launch_posture {
        PrepareLaunchPosture::Standard => (
            "--remote-debugging-port=0".to_owned(),
            SpawnedEndpointHint::ProfilePortFile,
        ),
        PrepareLaunchPosture::DriverSelectedPort => {
            let port = reserve_driver_selected_debugging_port()?;
            (
                format!("--remote-debugging-port={port}"),
                SpawnedEndpointHint::FixedLoopbackPort(port),
            )
        }
    };
    command
        .arg(remote_debugging_port)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--disable-default-apps")
        .arg("--disable-extensions");
    #[cfg(target_os = "windows")]
    command
        .arg("--window-position=40,40")
        .arg("--window-size=900,640");
    #[cfg(target_os = "linux")]
    {
        let native_wayland = std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|session| session.eq_ignore_ascii_case("wayland"))
            && std::env::var_os("WAYLAND_DISPLAY").is_some()
            && std::env::var_os("DISPLAY").is_none();
        let no_sandbox = std::env::var("CUA_E2E_BROWSER_NO_SANDBOX").as_deref() == Ok("1");
        configure_linux_isolated_browser_command(&mut command, native_wayland, no_sandbox);
    }
    let stderr = if std::env::var_os("CUA_E2E_BROWSER_STDERR").is_some() {
        Stdio::inherit()
    } else {
        Stdio::null()
    };
    command
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    Ok(IsolatedBrowserCommand {
        command,
        endpoint_hint,
    })
}

fn clean_spawn_exit_can_be_launcher_handoff(status: &ExitStatus) -> bool {
    #[cfg(target_os = "windows")]
    {
        status.success()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = status;
        false
    }
}

fn write_profile_marker(path: &Path, marker: &ProfileMarker) -> Result<(), BrowserRefusal> {
    let marker_path = path.join(PROFILE_MARKER);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&marker_path).map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not create the isolated profile ownership marker: {error}"),
        )
    })?;
    let bytes = serde_json::to_vec(marker).expect("serialize profile marker");
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not persist the isolated profile ownership marker: {error}"),
            )
        })
}

fn prepare_profile(profile: &PrepareProfile) -> Result<PreparedProfile, BrowserRefusal> {
    validate_profile(profile)?;
    let root = profile_root()?;
    fs::create_dir_all(&root).map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not create the driver-owned profile root: {error}"),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not restrict the driver-owned profile root: {error}"),
            )
        })?;
    }
    let root_metadata = fs::symlink_metadata(&root).map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not inspect the driver-owned profile root: {error}"),
        )
    })?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "the driver-owned profile root is not a real directory",
        ));
    }
    let leaf = match profile.mode {
        PrepareProfileMode::IsolatedNew => format!("isolated-{}", uuid::Uuid::new_v4()),
        PrepareProfileMode::IsolatedNamed => profile.name.clone().expect("validated profile name"),
    };
    let path = root.join(leaf);
    let marker = ProfileMarker {
        schema: PROFILE_SCHEMA.to_owned(),
        mode: profile.mode,
        name: profile.name.clone(),
    };
    let created = if path.exists() {
        if profile.mode != PrepareProfileMode::IsolatedNamed
            || !profile_matches_marker(&path, &marker)
        {
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "the requested profile directory exists but is not proven driver-owned",
            ));
        }
        false
    } else {
        fs::create_dir(&path).map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not create an isolated browser profile: {error}"),
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o700)) {
                let _ = fs::remove_dir(&path);
                return Err(refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not restrict the isolated browser profile: {error}"),
                ));
            }
        }
        let is_empty = fs::read_dir(&path)
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not verify the isolated browser profile: {error}"),
                )
            })?
            .next()
            .is_none();
        if !is_empty {
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "new isolated browser profile was not empty",
            ));
        }
        if let Err(error) = write_profile_marker(&path, &marker) {
            let _ = fs::remove_dir(&path);
            return Err(error);
        }
        true
    };
    Ok(PreparedProfile {
        path,
        created,
        delete_on_cleanup: profile.mode == PrepareProfileMode::IsolatedNew,
        marker,
    })
}

async fn wait_for_spawned_endpoint(
    engine: &BrowserEngine,
    child: &mut Child,
    profile: &Path,
    endpoint_hint: SpawnedEndpointHint,
) -> Result<OwnedEndpoint, BrowserRefusal> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let port_file = profile.join("DevToolsActivePort");
    let mut observed_clean_launcher_exit = false;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not inspect the isolated browser process: {error}"),
            )
        })? {
            if clean_spawn_exit_can_be_launcher_handoff(&status) {
                // Edge on Windows ARM can transfer the browser role to a
                // descendant and let its launcher exit successfully. Keep
                // waiting for the driver-owned profile's port file; the live
                // listener and its descendant ownership are attested below.
                observed_clean_launcher_exit = true;
            } else {
                return Err(refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("isolated browser exited before exposing DevTools ({status})"),
                ));
            }
        }
        match endpoint_hint {
            SpawnedEndpointHint::ProfilePortFile => {
                if let Ok(text) = fs::read_to_string(&port_file) {
                    let mut lines = text.lines();
                    if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                        if let Ok(port) = port.parse::<u16>() {
                            if path.starts_with("/devtools/browser/")
                                && tokio::net::TcpStream::connect(("127.0.0.1", port))
                                    .await
                                    .is_ok()
                            {
                                return Ok(OwnedEndpoint {
                                    ws_url: format!("ws://127.0.0.1:{port}{path}"),
                                    http_port: Some(port),
                                    transport: super::types::EndpointTransport::SpawnedExact,
                                    ownership: EndpointOwnershipProof {
                                        method: EndpointOwnershipMethod::SpawnedByDriver,
                                        owner_pid: i64::from(child.id()),
                                        listener_pid: None,
                                        detail: Some(
                                            "driver-spawned process and private profile port file"
                                                .to_owned(),
                                        ),
                                    },
                                });
                            }
                        }
                    }
                }
            }
            SpawnedEndpointHint::FixedLoopbackPort(port) => {
                if let Some(endpoint) = engine
                    .platform
                    .discover_spawned_endpoint_on_port(i64::from(child.id()), port)
                    .await?
                {
                    return Ok(endpoint);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                if observed_clean_launcher_exit {
                    "isolated browser launcher exited cleanly, but its process tree did not expose a loopback DevTools endpoint before timeout"
                } else {
                    "isolated browser did not expose a loopback DevTools endpoint before timeout"
                },
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn attest_spawned_endpoint(
    engine: &BrowserEngine,
    child_pid: i64,
    profile_endpoint: OwnedEndpoint,
    endpoint_hint: SpawnedEndpointHint,
) -> Result<OwnedEndpoint, BrowserRefusal> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let live = match endpoint_hint {
            SpawnedEndpointHint::ProfilePortFile => {
                engine
                    .platform
                    .discover_spawned_endpoint(child_pid, &profile_endpoint.ws_url)
                    .await?
            }
            SpawnedEndpointHint::FixedLoopbackPort(port) => {
                engine
                    .platform
                    .discover_spawned_endpoint_on_port(child_pid, port)
                    .await?
            }
        };
        if let Some(live) = live {
            if live.http_port == profile_endpoint.http_port
                && live.ws_url == profile_endpoint.ws_url
            {
                let runtime_pid = spawned_runtime_pid(&live.ownership);
                let endpoint_source = profile_endpoint
                    .ownership
                    .detail
                    .as_deref()
                    .unwrap_or("driver-spawned browser endpoint");
                return Ok(OwnedEndpoint {
                    ws_url: live.ws_url,
                    http_port: live.http_port,
                    transport: super::types::EndpointTransport::SpawnedExact,
                    ownership: EndpointOwnershipProof {
                        method: EndpointOwnershipMethod::SpawnedByDriver,
                        // The platform already proved the exact listener is in
                        // child_pid's process tree. Promote that live process
                        // to the prepared-browser identity so ARM64 launcher
                        // handoffs remain bindable and reapable. Later Windows
                        // reproof normalizes ownership to this stable pid while
                        // retaining any new exact listener separately.
                        owner_pid: runtime_pid,
                        listener_pid: live.ownership.listener_pid,
                        detail: Some(if runtime_pid == child_pid {
                            format!("{endpoint_source} plus live loopback socket owner")
                        } else {
                            format!(
                                "{endpoint_source} plus live loopback socket owner promoted from a short-lived launcher process"
                            )
                        }),
                    },
                });
            }
        }
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                "the isolated browser profile endpoint did not match live loopback socket ownership",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn spawned_runtime_pid(ownership: &EndpointOwnershipProof) -> i64 {
    ownership.listener_pid.unwrap_or(ownership.owner_pid)
}

impl BrowserEngine {
    pub(crate) fn is_driver_owned_pid_for_session(&self, session: &str, pid: i64) -> bool {
        self.managed_browsers.lock().unwrap().iter().any(|browser| {
            browser.owned_pid == pid && browser.owner_sessions.iter().any(|owner| owner == session)
        })
    }

    pub(crate) fn cleanup_prepared_session(&self, session: &str) {
        self.protected_resource_ownership.remove_session(session);
        self.managed_browsers
            .lock()
            .unwrap()
            .retain(|browser| !browser.owner_sessions.iter().any(|owner| owner == session));
    }

    pub(crate) async fn prepare_browser(
        &self,
        request: PrepareRequest,
    ) -> Result<PrepareOutcome, BrowserRefusal> {
        if request.launch_posture != PrepareLaunchPosture::Standard
            && !(request.pid.is_none()
                && request.strategy.is_none()
                && request.allow_launch
                && request.profile.is_some())
        {
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "launch_posture=driver_selected_port is standalone-only: omit pid and pass allow_launch=true with an isolated profile; it cannot attach to an existing profile or prepare an existing browser process",
            ));
        }
        if request.strategy == Some(PrepareStrategy::ExistingProfile) {
            return self.attach_existing_profile(request).await;
        }
        if request.strategy.is_some() {
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "the requested browser preparation strategy is unsupported",
            ));
        }

        if request.pid.is_some() {
            match self.platform.prepare_endpoint(request.clone()).await {
                Ok(mut outcome) => {
                    if outcome.prepared_pid.is_none() {
                        outcome.prepared_pid = outcome
                            .endpoint
                            .as_ref()
                            .map(|endpoint| endpoint.ownership.owner_pid);
                    }
                    return Ok(outcome);
                }
                Err(error) if error.code != BrowserRefusalCode::BrowserRequiresSetup => {
                    return Err(error)
                }
                Err(_) => {}
            }
        }

        if !request.allow_launch {
            return Err(refusal(
                BrowserRefusalCode::BrowserRequiresSetup,
                "no owned endpoint is available; pass allow_launch=true with an isolated profile and verified approval",
            ));
        }
        let profile_request = request.profile.as_ref().ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "acting setup requires profile.mode=isolated_new or isolated_named",
            )
        })?;
        validate_profile(profile_request)?;
        let executable = if let Some(pid) = request.pid {
            let classification = self.platform.classify_browser(pid).await?;
            if !classification.supports_cdp
                || classification.engine != BrowserEngineFamily::Chromium
            {
                return Err(unsupported_engine_refusal(
                    &classification,
                    "prepare_isolated_profile",
                ));
            }
            self.platform
                .process_fingerprint(pid)
                .await?
                .executable
                .ok_or_else(|| {
                    refusal(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        "could not prove the requested browser executable for isolated launch",
                    )
                })?
        } else {
            self.platform.isolated_browser_executable()?
        };
        let prepared_profile = prepare_profile(profile_request)?;
        let mut launch = match isolated_browser_command(
            &executable,
            &prepared_profile.path,
            request.launch_posture,
        ) {
            Ok(launch) => launch,
            Err(error) => {
                cleanup_created_profile(&prepared_profile);
                return Err(error);
            }
        };
        let (mut child, mut process_job) = match spawn_isolated_browser_process(
            &mut launch.command,
            request.launch_posture,
            &prepared_profile,
        ) {
            Ok(spawned) => spawned,
            Err(error) => return Err(error),
        };
        let endpoint = match wait_for_spawned_endpoint(
            self,
            &mut child,
            &prepared_profile.path,
            launch.endpoint_hint,
        )
        .await
        {
            Ok(endpoint) => {
                let endpoint_owner = spawned_runtime_pid(&endpoint.ownership);
                match attest_spawned_endpoint(
                    self,
                    i64::from(child.id()),
                    endpoint,
                    launch.endpoint_hint,
                )
                .await
                {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        let process_tree_stopped = terminate_browser_process_tree(
                            &mut child,
                            Some(endpoint_owner),
                            &mut process_job,
                        );
                        if process_tree_stopped {
                            cleanup_created_profile(&prepared_profile);
                        }
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                let process_tree_stopped =
                    terminate_browser_process_tree(&mut child, None, &mut process_job);
                if process_tree_stopped {
                    cleanup_created_profile(&prepared_profile);
                }
                return Err(error);
            }
        };
        let prepared_pid = endpoint.ownership.owner_pid;
        let mut owner_sessions = vec![request.session];
        if let Some(transport_session) = request.transport_session {
            if !owner_sessions.contains(&transport_session) {
                owner_sessions.push(transport_session);
            }
        }
        if let Ok(fingerprint) = self.platform.process_fingerprint(prepared_pid).await {
            for owner in &owner_sessions {
                self.protected_resource_ownership
                    .mark_driver_owned_process(owner, fingerprint.clone());
            }
        }
        self.managed_browsers.lock().unwrap().push(ManagedBrowser {
            child,
            process_job,
            owned_pid: prepared_pid,
            profile: prepared_profile.path,
            delete_profile: prepared_profile.delete_on_cleanup,
            marker: prepared_profile.marker,
            owner_sessions,
        });
        Ok(PrepareOutcome {
            action: PrepareAction::LaunchedIsolatedBrowser,
            endpoint: Some(endpoint),
            message: match request.launch_posture {
                PrepareLaunchPosture::Standard => "Launched a separate driver-owned isolated Chromium process; no existing browser process was modified or terminated.".to_owned(),
                PrepareLaunchPosture::DriverSelectedPort => "Launched a separate driver-owned isolated Chromium process with a driver-selected nonzero DevTools port. navigator.webdriver is browser-version-dependent and was not measured; verify it in-page if it matters. No other browser identity or fingerprint overrides are applied, and detector bypass is not guaranteed.".to_owned(),
            },
            prepared_pid: Some(prepared_pid),
            launch_posture: Some(request.launch_posture),
            side_effects: PrepareSideEffects {
                launched_browser: true,
                created_profile: prepared_profile.created,
                reused_driver_profile: !prepared_profile.created,
                ..PrepareSideEffects::default()
            },
            attachment: None,
        })
    }

    async fn attach_existing_profile(
        &self,
        request: PrepareRequest,
    ) -> Result<PrepareOutcome, BrowserRefusal> {
        enum ConsentPath {
            Protected,
            BoundedManifest,
            LaunchGrant,
            Unrestricted,
        }

        let pid = request.pid.ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "strategy=existing_profile requires an exact pid approval anchor",
            )
        })?;
        let window_id = request.window_id.ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "strategy=existing_profile requires an exact window_id approval anchor",
            )
        })?;
        let mode = crate::tool::current_dispatch_authorization_context()
            .map(|context| context.mode())
            .map(Ok)
            .unwrap_or_else(crate::authorization::configured_permission_mode)
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserConsentRequired,
                    format!("permission mode is unavailable: {error}"),
                )
            })?;
        let consent_path = if mode == crate::authorization::PermissionMode::Unrestricted {
            ConsentPath::Unrestricted
        } else if mode == crate::authorization::PermissionMode::Bounded {
            let context = crate::tool::current_dispatch_authorization_context().ok_or_else(|| {
                refusal(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "bounded existing-profile attachment requires a live session authorization context",
                )
            })?;
            let manifest = context.capability_manifest().ok_or_else(|| {
                refusal(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "bounded existing-profile attachment requires an approved capability manifest",
                )
            })?;
            manifest
                .authorize_call(
                    "browser_prepare",
                    &serde_json::json!({
                        "pid": pid,
                        "window_id": window_id,
                        "strategy": {"kind": "existing_profile"},
                    }),
                )
                .map_err(|message| refusal(BrowserRefusalCode::BrowserConsentRequired, message))?;
            ConsentPath::BoundedManifest
        } else if crate::authorization::launch_grant_enabled("existing_profile") {
            ConsentPath::LaunchGrant
        } else if self.approval_broker.provider_id().is_some() {
            ConsentPath::Protected
        } else {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "existing-profile attachment in standard mode requires --grant existing-profile or an embedding authorization host; bounded mode requires a matching manifest",
            )
            .with_detail(serde_json::json!({
                "permission_mode": mode.as_str(),
                "authorization_required": true,
                "authorization_host": self.approval_broker.provider_id(),
            })));
        };
        if request.profile.is_some() || request.allow_launch {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "strategy=existing_profile cannot be combined with profile or allow_launch",
            ));
        }

        let classification = self.platform.classify_browser(pid).await?;
        if !classification.supports_cdp || classification.engine != BrowserEngineFamily::Chromium {
            return Err(unsupported_engine_refusal(
                &classification,
                "attach_existing_profile",
            ));
        }
        self.native_window_checked(pid, window_id).await?;
        let fingerprint = self.platform.process_fingerprint(pid).await?;
        let mut setup = ExistingProfileSetupOutcome::default();
        let setup_request = ExistingProfileSetupRequest {
            pid,
            window_id,
            browser: classification.product_kind,
        };
        let mut setup_pending = false;
        let mut setup_guard = None;
        let mut endpoint = self
            .platform
            .discover_existing_profile_endpoint(pid)
            .await?;
        if endpoint.is_none() {
            setup = self
                .platform
                .setup_existing_profile_endpoint(setup_request.clone())
                .await?;
            setup_pending = true;
            setup_guard = Some(PendingExistingProfileSetup::new(
                self.platform.clone(),
                setup_request.clone(),
            ));

            // Setup is allowed to interact only with the already-approved
            // process/window generation. Re-prove both before accepting the
            // newly exposed listener.
            if let Err(error) = self.native_window_checked(pid, window_id).await {
                return Err(setup_guard
                    .as_mut()
                    .expect("setup guard exists while setup is pending")
                    .abort(with_setup_side_effects(error, &setup))
                    .await);
            }
            let current_fingerprint = match self.platform.process_fingerprint(pid).await {
                Ok(fingerprint) => fingerprint,
                Err(error) => {
                    return Err(setup_guard
                        .as_mut()
                        .expect("setup guard exists while setup is pending")
                        .abort(with_setup_side_effects(error, &setup))
                        .await)
                }
            };
            if current_fingerprint != fingerprint {
                let error = with_setup_side_effects(
                    refusal(
                        BrowserRefusalCode::BrowserBindingStale,
                        "the approved browser process changed while remote debugging was being enabled",
                    ),
                    &setup,
                );
                return Err(setup_guard
                    .as_mut()
                    .expect("setup guard exists while setup is pending")
                    .abort(error)
                    .await);
            }
            endpoint = setup.endpoint.clone();
            if endpoint.is_none() {
                endpoint = match self.platform.discover_existing_profile_endpoint(pid).await {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        return Err(setup_guard
                            .as_mut()
                            .expect("setup guard exists while setup is pending")
                            .abort(with_setup_side_effects(error, &setup))
                            .await)
                    }
                };
            }
        }
        let endpoint = match endpoint {
            Some(endpoint) => endpoint,
            None => {
                let error = with_setup_side_effects(refusal(
                    BrowserRefusalCode::BrowserRequiresSetup,
                    "the approved browser still has no uniquely proven DevTools endpoint after bounded setup",
                ), &setup);
                if setup_pending {
                    return Err(setup_guard
                        .as_mut()
                        .expect("setup guard exists while setup is pending")
                        .abort(error)
                        .await);
                }
                return Err(error);
            }
        };
        if endpoint.ownership.owner_pid != pid {
            let error = with_setup_side_effects(
                refusal(
                    BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                    "the existing-profile endpoint is not owned by the approved browser process",
                ),
                &setup,
            );
            if setup_pending {
                return Err(setup_guard
                    .as_mut()
                    .expect("setup guard exists while setup is pending")
                    .abort(error)
                    .await);
            }
            return Err(error);
        }

        // Host authorization is requested only after the exact process,
        // native window, browser product, and endpoint owner have all been
        // proven. Bounded capability manifests, launch grants, and unrestricted mode
        // never enter this callback path.
        let protected_consent = if matches!(consent_path, ConsentPath::Protected) {
            let transport_session = request
                .transport_session
                .as_deref()
                .unwrap_or(request.session.as_str());
            let approval_request = self.approval_broker.request(
                mode,
                "browser_prepare.existing_profile",
                crate::authorization::RiskClass::R2,
                request.session.clone(),
                transport_session.to_owned(),
                serde_json::json!({
                    "pid": pid,
                    "window_id": window_id,
                    "process_fingerprint": fingerprint.clone(),
                    "browser_product": classification.product_kind,
                    "endpoint_owner_pid": endpoint.ownership.owner_pid,
                }),
                format!(
                    "Attach Cua Driver to {:?} window {} using its logged-in profile",
                    classification.product_kind, window_id
                ),
            );
            Some(
                self.approval_broker
                    .approve(&approval_request)
                    .await
                    .map_err(|error| {
                        refusal(
                            BrowserRefusalCode::BrowserConsentRequired,
                            format!("existing-profile authorization was not granted: {error}"),
                        )
                        .with_detail(serde_json::json!({
                            "permission_mode": mode.as_str(),
                            "trusted_consent_required": true,
                            "provider": self.approval_broker.provider_id(),
                            "approval_request_id": approval_request.nonce,
                        }))
                    })?,
            )
        } else {
            None
        };

        let previous_grant = self
            .existing_profile_grant(&request.session, request.transport_session.as_deref(), pid)
            .await;
        let previous_grant = match previous_grant {
            Ok(grant) => grant,
            Err(error) => {
                if let Some(protected) = protected_consent.as_ref() {
                    self.approval_broker.revoke(protected).await;
                }
                let error = with_setup_side_effects(error, &setup);
                if setup_pending {
                    return Err(setup_guard
                        .as_mut()
                        .expect("setup guard exists while setup is pending")
                        .abort(error)
                        .await);
                }
                return Err(error);
            }
        };
        let previous_generation = previous_grant.as_ref().map_or(0, |grant| grant.generation);
        let cleanup_remote_debugging = setup.enabled_remote_debugging
            || previous_grant
                .as_ref()
                .is_some_and(|grant| grant.cleanup_remote_debugging);
        let grant = self.existing_profile_grants.mint(
            &request.session,
            request.transport_session.as_deref(),
            pid,
            window_id,
            fingerprint,
            "chromium".to_owned(),
            classification.product_kind,
            endpoint.ws_url.clone(),
            cleanup_remote_debugging,
            protected_consent,
        );
        if let Some(previous) = previous_grant {
            self.pool
                .release_existing(&previous.endpoint_ws_url, previous.generation)
                .await;
            if let Some(protected) = previous.protected_consent.as_ref() {
                self.approval_broker.revoke(protected).await;
            }
            if previous.endpoint_ws_url != endpoint.ws_url {
                self.pool.release_claim_marker(&previous.endpoint_ws_url);
            }
        }
        let (claimed, displayed_consent_prompt) = {
            let ws_url = endpoint.ws_url.clone();
            let mut claim = Box::pin(self.pool.claim_existing(&ws_url, grant.generation));
            let initial = tokio::select! {
                result = &mut claim => Some(result),
                _ = tokio::time::sleep(Duration::from_millis(500)) => None,
            };
            if let Some(result) = initial {
                (result, false)
            } else {
                match claim_with_optional_consent(
                    &mut claim,
                    self.platform
                        .handle_existing_profile_consent(BrowserConsentRequest {
                            pid,
                            window_id,
                            attempt: 1,
                        }),
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        // The connection future may hold the pool mutex while
                        // awaiting its WebSocket handshake. Cancel it before
                        // revoking the grant, which also needs that mutex.
                        drop(claim);
                        self.revoke_existing_profile_grant(
                            &request.session,
                            request.transport_session.as_deref(),
                            pid,
                        )
                        .await;
                        let error = with_setup_side_effects(error, &setup);
                        if setup_pending {
                            return Err(setup_guard
                                .as_mut()
                                .expect("setup guard exists while setup is pending")
                                .abort(error)
                                .await);
                        }
                        return Err(error);
                    }
                }
            }
        };
        // Chrome on Windows can reject the WebSocket handshake that was
        // pending while its native remote-debugging consent prompt was open.
        // After an explicit acceptance, make one fresh, bounded dial to the
        // same attested endpoint under the same grant. The driver does not
        // request consent again or broaden the approved target; the browser
        // still owns any transport-level UI for the fresh connection.
        let (claimed, initial_claim_error) = retry_claim_after_accepted_consent(
            claimed,
            displayed_consent_prompt,
            self.pool.claim_existing(&endpoint.ws_url, grant.generation),
        )
        .await;
        if let Err(_final_claim_error) = claimed {
            self.revoke_existing_profile_grant(
                &request.session,
                request.transport_session.as_deref(),
                pid,
            )
            .await;
            let error = with_prepare_side_effects(
                refusal(
                    BrowserRefusalCode::BrowserReconnectExhausted,
                    "the approved browser socket could not be claimed",
                )
                .with_detail(serde_json::json!({
                    "retried_after_consent": initial_claim_error.is_some(),
                    "fresh_claim_failed": initial_claim_error.is_some(),
                })),
                &setup,
                displayed_consent_prompt,
            );
            if setup_pending {
                return Err(setup_guard
                    .as_mut()
                    .expect("setup guard exists while setup is pending")
                    .abort(error)
                    .await);
            }
            return Err(error);
        }
        if setup_pending {
            match setup_guard
                .as_mut()
                .expect("setup guard exists while setup is pending")
                .commit()
                .await
            {
                Ok(closed) => setup.closed_setup_page = closed,
                Err(error) => {
                    self.revoke_existing_profile_grant(
                        &request.session,
                        request.transport_session.as_deref(),
                        pid,
                    )
                    .await;
                    return Err(with_prepare_side_effects(
                        error,
                        &setup,
                        displayed_consent_prompt,
                    ));
                }
            }
        }
        self.store
            .invalidate_endpoint_generation(pid, previous_generation);

        Ok(PrepareOutcome {
            action: PrepareAction::AttachedExistingProfile,
            endpoint: Some(endpoint),
            message: "Attached to the approved existing Chromium profile. Bind the native window again before using browser capabilities.".to_owned(),
            prepared_pid: Some(pid),
            launch_posture: None,
            side_effects: PrepareSideEffects {
                displayed_consent_prompt,
                changed_preferences: setup.enabled_remote_debugging,
                opened_setup_page: setup.opened_setup_page,
                closed_setup_page: setup.closed_setup_page,
                enabled_remote_debugging: setup.enabled_remote_debugging,
                used_bounded_pixel_fallback: setup.used_bounded_pixel_fallback,
                focused_setup_address_field: setup.focused_setup_address_field,
                foregrounded_window: setup.foregrounded_window,
                injected_global_input: setup.injected_global_input,
                ..PrepareSideEffects::default()
            },
            attachment: Some(PrepareAttachment {
                kind: PrepareAttachmentKind::ExistingProfile,
                browser: "chromium".to_owned(),
                capabilities_invalidated: true,
                next_action: "get_browser_state".to_owned(),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    const WINDOWS_JOB_HANDOFF_HELPER_ENV: &str = "CUA_TEST_WINDOWS_JOB_HANDOFF_HELPER";

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_job_handoff_helper() {
        let Some(pid_path) = std::env::var_os(WINDOWS_JOB_HANDOFF_HELPER_ENV) else {
            return;
        };
        let child = Command::new("cmd.exe")
            .args(["/D", "/S", "/C", "ping -n 120 127.0.0.1 >NUL"])
            .spawn()
            .expect("spawn long-lived handoff descendant");
        fs::write(pid_path, child.id().to_string()).expect("record handoff descendant pid");
        drop(child);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_job_reaps_descendant_after_clean_launcher_handoff() {
        use core::ffi::c_void;
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use std::os::windows::process::CommandExt;
        use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows::Win32::System::JobObjects::IsProcessInJob;
        use windows::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, CREATE_SUSPENDED, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let pid_path = temp.path().join("descendant.pid");
        let mut command = Command::new(std::env::current_exe().expect("current test executable"));
        command
            .arg("windows_job_handoff_helper")
            .arg("--nocapture")
            .env(WINDOWS_JOB_HANDOFF_HELPER_ENV, &pid_path)
            .creation_flags(CREATE_SUSPENDED.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let job = WindowsBrowserJob::new().expect("create browser lifecycle job");
        let mut launcher = command.spawn().expect("spawn suspended launcher helper");
        job.assign(&launcher).expect("assign suspended launcher");
        job.resume_suspended_child(&launcher)
            .expect("resume owned launcher");
        assert!(launcher.wait().expect("wait for launcher").success());

        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_path.is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let descendant_pid = fs::read_to_string(&pid_path)
            .expect("handoff descendant pid file")
            .trim()
            .parse::<u32>()
            .expect("numeric handoff descendant pid");
        let descendant = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                descendant_pid,
            )
        }
        .expect("open handoff descendant");
        let descendant = unsafe { OwnedHandle::from_raw_handle(descendant.0 as *mut c_void) };
        let descendant_raw = windows::Win32::Foundation::HANDLE(
            std::os::windows::io::AsRawHandle::as_raw_handle(&descendant) as *mut c_void,
        );
        let mut in_job = windows::Win32::Foundation::BOOL::default();
        unsafe { IsProcessInJob(descendant_raw, job.raw(), &mut in_job) }
            .expect("query descendant job ownership");
        assert!(in_job.as_bool());
        assert_eq!(
            unsafe { WaitForSingleObject(descendant_raw, 0) },
            WAIT_TIMEOUT
        );

        assert!(job.terminate_and_wait());
        assert_eq!(
            unsafe { WaitForSingleObject(descendant_raw, 5_000) },
            WAIT_OBJECT_0
        );
    }

    #[tokio::test]
    async fn completed_claim_wins_while_optional_consent_is_absent() {
        let mut claim = Box::pin(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<_, anyhow::Error>(7_u8)
        });
        let consent = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "no consent surface",
            ))
        };
        let (result, displayed) = claim_with_optional_consent(&mut claim, consent)
            .await
            .expect("the completed claim should win the race");
        assert_eq!(result.unwrap(), 7);
        assert!(!displayed);
    }

    #[tokio::test]
    async fn accepted_consent_waits_for_the_same_claim() {
        let mut claim = Box::pin(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<_, anyhow::Error>(9_u8)
        });
        let (result, displayed) =
            claim_with_optional_consent(&mut claim, async { Ok(BrowserConsentOutcome::Accepted) })
                .await
                .expect("accepted consent should resume the existing claim");
        assert_eq!(result.unwrap(), 9);
        assert!(displayed);
    }

    #[tokio::test]
    async fn accepted_consent_retries_one_failed_claim_with_a_fresh_dial() {
        let (result, initial_error) = retry_claim_after_accepted_consent(
            Err(anyhow::anyhow!("pre-consent handshake rejected")),
            true,
            async { Ok::<_, anyhow::Error>(11_u8) },
        )
        .await;
        assert_eq!(result.unwrap(), 11);
        assert_eq!(
            initial_error.expect("initial error").to_string(),
            "pre-consent handshake rejected"
        );
    }

    #[tokio::test]
    async fn claim_failure_without_accepted_consent_is_not_retried() {
        let retry_polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let retry_marker = retry_polled.clone();
        let (result, initial_error) = retry_claim_after_accepted_consent(
            Err::<u8, _>(anyhow::anyhow!("connection refused")),
            false,
            async move {
                retry_marker.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, anyhow::Error>(12_u8)
            },
        )
        .await;
        assert_eq!(result.unwrap_err().to_string(), "connection refused");
        assert!(initial_error.is_none());
        assert!(!retry_polled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn successful_claim_does_not_retry_after_accepted_consent() {
        let retry_polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let retry_marker = retry_polled.clone();
        let (result, initial_error) =
            retry_claim_after_accepted_consent(Ok::<_, anyhow::Error>(13_u8), true, async move {
                retry_marker.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, anyhow::Error>(14_u8)
            })
            .await;
        assert_eq!(result.unwrap(), 13);
        assert!(initial_error.is_none());
        assert!(!retry_polled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn accepted_consent_limits_a_failed_fresh_dial_to_one_retry() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempt_counter = attempts.clone();
        let (result, initial_error) = retry_claim_after_accepted_consent(
            Err::<u8, _>(anyhow::anyhow!("pre-consent handshake rejected")),
            true,
            async move {
                attempt_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err::<u8, _>(anyhow::anyhow!("fresh handshake rejected"))
            },
        )
        .await;
        assert_eq!(result.unwrap_err().to_string(), "fresh handshake rejected");
        assert_eq!(
            initial_error.expect("initial error").to_string(),
            "pre-consent handshake rejected"
        );
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn setup_side_effects_are_preserved_on_refusal() {
        let error = with_setup_side_effects(
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                "fixture binding changed",
            )
            .with_detail(serde_json::json!({"original": true})),
            &ExistingProfileSetupOutcome {
                opened_setup_page: true,
                closed_setup_page: true,
                enabled_remote_debugging: true,
                used_bounded_pixel_fallback: true,
                focused_setup_address_field: true,
                foregrounded_window: true,
                injected_global_input: true,
                endpoint: None,
            },
        );
        let detail = error.detail.expect("setup detail");
        assert_eq!(detail["setup_side_effects"]["opened_setup_page"], true);
        assert_eq!(
            detail["setup_side_effects"]["enabled_remote_debugging"],
            true
        );
        assert_eq!(
            detail["setup_side_effects"]["used_bounded_pixel_fallback"],
            true
        );
        assert_eq!(detail["cause"]["original"], true);
    }

    #[test]
    fn profile_marker_must_match_mode_and_name_exactly() {
        let path = std::env::temp_dir().join(format!(
            "cua-driver-profile-marker-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&path).unwrap();
        let expected = ProfileMarker {
            schema: PROFILE_SCHEMA.to_owned(),
            mode: PrepareProfileMode::IsolatedNamed,
            name: Some("research".to_owned()),
        };
        write_profile_marker(&path, &expected).unwrap();
        assert!(profile_matches_marker(&path, &expected));
        assert!(!profile_matches_marker(
            &path,
            &ProfileMarker {
                name: Some("other".to_owned()),
                ..expected.clone()
            }
        ));
        assert!(!profile_matches_marker(
            &path,
            &ProfileMarker {
                mode: PrepareProfileMode::IsolatedNew,
                name: None,
                ..expected.clone()
            }
        ));
        assert!(write_profile_marker(&path, &expected).is_err());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn isolated_launch_uses_a_deterministic_clean_profile() {
        let profile = Path::new("profile-under-test");
        let launch = isolated_browser_command(
            "chromium-under-test",
            profile,
            PrepareLaunchPosture::Standard,
        )
        .expect("standard launch command");
        assert_eq!(launch.endpoint_hint, SpawnedEndpointHint::ProfilePortFile);
        let args = launch
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        for required in [
            "--remote-debugging-port=0",
            "--user-data-dir=profile-under-test",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-extensions",
            "about:blank",
        ] {
            assert!(args.iter().any(|arg| arg == required), "missing {required}");
        }
        #[cfg(target_os = "windows")]
        for required in ["--window-position=40,40", "--window-size=900,640"] {
            assert!(args.iter().any(|arg| arg == required), "missing {required}");
        }
        #[cfg(target_os = "linux")]
        assert!(args.iter().any(|arg| arg == "--password-store=basic"));
    }

    #[test]
    fn driver_selected_port_launch_uses_a_fixed_nonzero_port() {
        let profile = Path::new("profile-under-test");
        let launch = isolated_browser_command(
            "chromium-under-test",
            profile,
            PrepareLaunchPosture::DriverSelectedPort,
        )
        .expect("driver-selected-port launch command");
        let SpawnedEndpointHint::FixedLoopbackPort(hint_port) = launch.endpoint_hint else {
            panic!("driver-selected-port launch must use a fixed loopback port hint");
        };
        assert_ne!(hint_port, 0);
        let args = launch
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args
            .iter()
            .any(|arg| arg == &format!("--remote-debugging-port={hint_port}")));
        for required in [
            "--user-data-dir=profile-under-test",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-extensions",
            "about:blank",
        ] {
            assert!(args.iter().any(|arg| arg == required), "missing {required}");
        }
        for omitted in [
            "--disable-blink-features=AutomationControlled",
            "--enable-automation",
        ] {
            assert!(
                !args.iter().any(|arg| arg == omitted),
                "driver-selected-port posture must not add {omitted}"
            );
        }
        #[cfg(target_os = "linux")]
        assert!(args.iter().any(|arg| arg == "--password-store=basic"));
    }

    #[test]
    fn clean_launcher_exit_is_only_deferred_on_windows() {
        let mut command = if cfg!(target_os = "windows") {
            let mut command = Command::new("cmd.exe");
            command.args(["/C", "exit", "0"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            command
        };
        let status = command.status().unwrap();
        assert_eq!(
            clean_spawn_exit_can_be_launcher_handoff(&status),
            cfg!(target_os = "windows")
        );
    }

    #[test]
    fn spawned_runtime_promotes_a_proven_listener_without_losing_the_root() {
        let proof = EndpointOwnershipProof {
            method: EndpointOwnershipMethod::ListeningSocketPid,
            owner_pid: 42,
            listener_pid: Some(43),
            detail: Some("listener 43 proven inside process tree 42".to_owned()),
        };
        assert_eq!(spawned_runtime_pid(&proof), 43);
        assert_eq!(proof.owner_pid, 42);

        let root_owned = EndpointOwnershipProof {
            listener_pid: None,
            ..proof
        };
        assert_eq!(spawned_runtime_pid(&root_owned), 42);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn isolated_launch_can_select_native_wayland_and_test_vm_sandbox_mode() {
        let mut command = Command::new("chromium-under-test");
        configure_linux_isolated_browser_command(&mut command, true, true);
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.iter().any(|arg| arg == "--password-store=basic"));
        assert!(args.iter().any(|arg| arg == "--ozone-platform=wayland"));
        assert!(args.iter().any(|arg| arg == "--no-sandbox"));
    }
}
