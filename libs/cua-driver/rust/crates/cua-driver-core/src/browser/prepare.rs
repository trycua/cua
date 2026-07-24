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

use super::approval::{
    consume_existing_profile_approval, consume_prepare_approval, validate_profile,
    ExistingProfileApprovalScope,
};
use super::engine::unsupported_engine_refusal;
use super::platform::{
    BrowserConsentOutcome, BrowserConsentRequest, ExistingProfileSetupOutcome,
    ExistingProfileSetupRequest, PrepareAction, PrepareAttachment, PrepareAttachmentKind,
    PrepareAuthorization, PrepareOutcome, PrepareProfile, PrepareProfileMode, PrepareRequest,
    PrepareSideEffects, PrepareStrategy,
};
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::types::{
    BrowserEngineFamily, EndpointOwnershipMethod, EndpointOwnershipProof, OwnedEndpoint,
};
use super::BrowserEngine;

const PROFILE_MARKER: &str = ".cua-driver-owned-profile.json";
const PROFILE_SCHEMA: &str = "cua-driver-browser-profile-v1";

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

fn with_setup_side_effects(
    mut error: BrowserRefusal,
    setup: &ExistingProfileSetupOutcome,
) -> BrowserRefusal {
    if !setup.opened_setup_page
        && !setup.closed_setup_page
        && !setup.enabled_remote_debugging
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
    #[cfg(target_os = "windows")]
    owned_pid: i64,
    profile: PathBuf,
    delete_profile: bool,
    marker: ProfileMarker,
    owner_sessions: Vec<String>,
}

impl Drop for ManagedBrowser {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            // The isolated browser is spawned as its own process group. Chrome
            // fans out into renderer/utility descendants, so killing only the
            // root Child can leave profile writers alive after cleanup.
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(target_os = "windows")]
        if self.owned_pid != i64::from(self.child.id()) {
            // Edge on Windows ARM may use a short-lived launcher process and
            // transfer the browser role to a descendant. The listener owner
            // was attested inside that driver-spawned process tree, so reap
            // that exact process tree when its owning session ends.
            let _ = Command::new("taskkill.exe")
                .args(["/PID", &self.owned_pid.to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.delete_profile && profile_matches_marker(&self.profile, &self.marker) {
            let _ = fs::remove_dir_all(&self.profile);
        }
    }
}

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

fn isolated_browser_command(executable: &str, profile: &Path) -> Command {
    let mut command = Command::new(executable);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .arg("--remote-debugging-port=0")
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
    command
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
    child: &mut Child,
    profile: &Path,
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
                            ownership: EndpointOwnershipProof {
                                method: EndpointOwnershipMethod::SpawnedByDriver,
                                owner_pid: i64::from(child.id()),
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
) -> Result<OwnedEndpoint, BrowserRefusal> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(live) = engine.platform.discover_owned_endpoint(child_pid).await? {
            if live.http_port == profile_endpoint.http_port
                && live.ws_url == profile_endpoint.ws_url
            {
                return Ok(OwnedEndpoint {
                    ws_url: live.ws_url,
                    http_port: live.http_port,
                    ownership: EndpointOwnershipProof {
                        method: EndpointOwnershipMethod::SpawnedByDriver,
                        owner_pid: live.ownership.owner_pid,
                        detail: Some(if live.ownership.owner_pid == child_pid {
                            "driver-owned profile port file plus live loopback socket owner"
                                .to_owned()
                        } else {
                            "driver-owned profile port file plus live loopback socket owner promoted from a short-lived launcher process"
                                    .to_owned()
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

impl BrowserEngine {
    pub(crate) fn cleanup_prepared_session(&self, session: &str) {
        self.managed_browsers
            .lock()
            .unwrap()
            .retain(|browser| !browser.owner_sessions.iter().any(|owner| owner == session));
    }

    pub(crate) async fn prepare_browser(
        &self,
        request: PrepareRequest,
    ) -> Result<PrepareOutcome, BrowserRefusal> {
        if request.strategy == Some(PrepareStrategy::ExistingProfile) {
            return self.attach_existing_profile(request).await;
        }
        if request.strategy.is_some() {
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "the requested browser preparation strategy is unsupported",
            ));
        }

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
        match request.authorization.as_ref() {
            Some(PrepareAuthorization::McpHost) => {}
            Some(PrepareAuthorization::ApprovalArtifact(token)) => {
                consume_prepare_approval(token, request.pid, profile_request)?;
            }
            None => {
                return Err(refusal(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "browser preparation needs MCP host approval or a fresh browser-approve token",
                ))
            }
        }

        let classification = self.platform.classify_browser(request.pid).await?;
        if !classification.supports_cdp || classification.engine != BrowserEngineFamily::Chromium {
            return Err(unsupported_engine_refusal(
                &classification,
                "prepare_isolated_profile",
            ));
        }
        let fingerprint = self.platform.process_fingerprint(request.pid).await?;
        let executable = fingerprint.executable.ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "could not prove the requested browser executable for isolated launch",
            )
        })?;
        let prepared_profile = prepare_profile(profile_request)?;
        let mut command = isolated_browser_command(&executable, &prepared_profile.path);
        let mut child = command.spawn().map_err(|error| {
            cleanup_created_profile(&prepared_profile);
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not launch an isolated browser process: {error}"),
            )
        })?;
        let endpoint = match wait_for_spawned_endpoint(&mut child, &prepared_profile.path).await {
            Ok(endpoint) => {
                match attest_spawned_endpoint(self, i64::from(child.id()), endpoint).await {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        cleanup_created_profile(&prepared_profile);
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                cleanup_created_profile(&prepared_profile);
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
        self.managed_browsers.lock().unwrap().push(ManagedBrowser {
            child,
            #[cfg(target_os = "windows")]
            owned_pid: prepared_pid,
            profile: prepared_profile.path,
            delete_profile: prepared_profile.delete_on_cleanup,
            marker: prepared_profile.marker,
            owner_sessions,
        });
        Ok(PrepareOutcome {
            action: PrepareAction::LaunchedIsolatedBrowser,
            endpoint: Some(endpoint),
            message: "Launched a separate driver-owned isolated Chromium process; the requested browser process was not modified or terminated.".to_owned(),
            prepared_pid: Some(prepared_pid),
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
            LegacyArtifact,
            Unrestricted,
        }

        let window_id = request.window_id.ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "strategy=existing_profile requires an exact window_id approval anchor",
            )
        })?;
        let scope = ExistingProfileApprovalScope {
            pid: request.pid,
            window_id,
            session: request.session.clone(),
        };
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
        } else if self.approval_broker.provider_id().is_some() {
            ConsentPath::Protected
        } else if crate::authorization::legacy_existing_profile_approval_enabled() {
            match request.authorization.as_ref() {
                Some(PrepareAuthorization::ApprovalArtifact(token)) => {
                    consume_existing_profile_approval(token, &scope)?;
                    ConsentPath::LegacyArtifact
                }
                // The ordinary MCP destructive-tool marker proves transport
                // provenance, not a person's approval of their authenticated
                // profile. It is deliberately insufficient here.
                Some(PrepareAuthorization::McpHost) | None => {
                    return Err(refusal(
                        BrowserRefusalCode::BrowserConsentRequired,
                        "legacy existing-profile compatibility requires a fresh operation-bound browser-approve artifact",
                    )
                    .with_detail(serde_json::json!({
                        "approval_request_id": uuid::Uuid::new_v4().to_string(),
                        "approval_command": "cua-driver browser-approve --strategy existing_profile --pid <pid> --window-id <window_id> --session <session>",
                        "legacy_approval_enabled": true,
                    })));
                }
            }
        } else {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "existing-profile attachment requires a certified trusted-consent provider in standard/bounded mode; the legacy file-backed artifact is disabled",
            )
            .with_detail(serde_json::json!({
                "permission_mode": mode.as_str(),
                "trusted_consent_required": true,
                "legacy_approval_enabled": false,
                "protected_consent_collector": self.approval_broker.provider_id(),
            })));
        };
        if request.profile.is_some() || request.allow_launch {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "strategy=existing_profile cannot be combined with profile or allow_launch",
            ));
        }

        let classification = self.platform.classify_browser(request.pid).await?;
        if !classification.supports_cdp || classification.engine != BrowserEngineFamily::Chromium {
            return Err(unsupported_engine_refusal(
                &classification,
                "attach_existing_profile",
            ));
        }
        self.native_window_checked(request.pid, window_id).await?;
        let fingerprint = self.platform.process_fingerprint(request.pid).await?;
        let mut setup = ExistingProfileSetupOutcome::default();
        let setup_request = ExistingProfileSetupRequest {
            pid: request.pid,
            window_id,
            browser: classification.product_kind,
        };
        let mut setup_pending = false;
        let mut setup_guard = None;
        let mut endpoint = self
            .platform
            .discover_existing_profile_endpoint(request.pid)
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
            if let Err(error) = self.native_window_checked(request.pid, window_id).await {
                return Err(setup_guard
                    .as_mut()
                    .expect("setup guard exists while setup is pending")
                    .abort(with_setup_side_effects(error, &setup))
                    .await);
            }
            let current_fingerprint = match self.platform.process_fingerprint(request.pid).await {
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
                endpoint = match self
                    .platform
                    .discover_existing_profile_endpoint(request.pid)
                    .await
                {
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
        if endpoint.ownership.owner_pid != request.pid {
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

        // Consent is collected only after the exact process, native window,
        // browser product, and endpoint owner have all been proven. This keeps
        // the human-visible request exact and avoids activating an indicator
        // for a target that cannot be attached.
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
                    "pid": request.pid,
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
                if mode == crate::authorization::PermissionMode::Bounded {
                    self.approval_broker
                        .activate_preapproved(&approval_request)
                        .await
                } else {
                    self.approval_broker.approve(&approval_request).await
                }
                .map_err(|error| {
                    refusal(
                        BrowserRefusalCode::BrowserConsentRequired,
                        format!("protected existing-profile consent was not granted: {error}"),
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
            .existing_profile_grant(
                &request.session,
                request.transport_session.as_deref(),
                request.pid,
            )
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
        let grant = self.existing_profile_grants.mint(
            &request.session,
            request.transport_session.as_deref(),
            request.pid,
            window_id,
            fingerprint,
            "chromium".to_owned(),
            endpoint.ws_url.clone(),
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
                            pid: request.pid,
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
                            request.pid,
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
        if let Err(_error) = claimed {
            self.revoke_existing_profile_grant(
                &request.session,
                request.transport_session.as_deref(),
                request.pid,
            )
            .await;
            let error = with_prepare_side_effects(
                refusal(
                    BrowserRefusalCode::BrowserReconnectExhausted,
                    "the approved browser socket could not be claimed",
                ),
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
                        request.pid,
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
            .invalidate_endpoint_generation(request.pid, previous_generation);

        Ok(PrepareOutcome {
            action: PrepareAction::AttachedExistingProfile,
            endpoint: Some(endpoint),
            message: "Attached to the approved existing Chromium profile. Bind the native window again before using browser capabilities.".to_owned(),
            prepared_pid: Some(request.pid),
            side_effects: PrepareSideEffects {
                displayed_consent_prompt,
                changed_preferences: setup.enabled_remote_debugging,
                opened_setup_page: setup.opened_setup_page,
                closed_setup_page: setup.closed_setup_page,
                enabled_remote_debugging: setup.enabled_remote_debugging,
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
        let command = isolated_browser_command("chromium-under-test", profile);
        let args = command
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
