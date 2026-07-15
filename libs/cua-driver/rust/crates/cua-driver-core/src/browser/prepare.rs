//! Acting `browser_prepare` implementation for driver-owned Chromium profiles.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::approval::{consume_prepare_approval, validate_profile};
use super::platform::{
    PrepareAction, PrepareAuthorization, PrepareOutcome, PrepareProfile, PrepareProfileMode,
    PrepareRequest, PrepareSideEffects,
};
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::types::{
    BrowserEngineFamily, EndpointOwnershipMethod, EndpointOwnershipProof, OwnedEndpoint,
};
use super::BrowserEngine;

const PROFILE_MARKER: &str = ".cua-driver-owned-profile.json";
const PROFILE_SCHEMA: &str = "cua-driver-browser-profile-v1";

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
    #[cfg(target_os = "linux")]
    command.arg("--password-store=basic");
    command
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
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
    loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not inspect the isolated browser process: {error}"),
            )
        })? {
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("isolated browser exited before exposing DevTools ({status})"),
            ));
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
                "isolated browser did not expose a loopback DevTools endpoint before timeout",
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
                        owner_pid: child_pid,
                        detail: Some(
                            "driver-owned profile port file plus live loopback socket owner"
                                .to_owned(),
                        ),
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
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "isolated automatic setup is currently supported only for Chromium browsers",
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        #[cfg(target_os = "linux")]
        assert!(args.iter().any(|arg| arg == "--password-store=basic"));
    }
}
