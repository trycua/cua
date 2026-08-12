//! Immutable bounded-autonomy manifest loaded at trusted daemon startup.
//!
//! The agent may propose this file, but selecting and approving it is a
//! launcher responsibility. The manifest only narrows the built-in,
//! managed, and user policy layers; it cannot introduce unreviewed tools.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "yaml")]
use serde::Deserialize;
#[cfg(feature = "yaml")]
use sha2::{Digest, Sha256};

pub const SESSION_POLICY_FILE_ENV: &str = "CUA_DRIVER_SESSION_POLICY_FILE";
pub const SESSION_POLICY_APPROVED_ENV: &str = "CUA_DRIVER_SESSION_POLICY_APPROVED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestDecision {
    Allow,
    Deny,
    Ask,
    Undeclared,
}

#[derive(Debug, Clone)]
pub struct SessionManifest {
    version: u32,
    sha256: String,
    expires_unix_ms: u128,
    idle_timeout: Duration,
    allow: HashSet<String>,
    deny: HashSet<String>,
    ask: HashSet<String>,
    existing_profiles: HashSet<(i64, u64)>,
    existing_profile_kind: bool,
    browser_origins: HashSet<String>,
    applications: Vec<ApplicationGrant>,
    desktop_applications: HashSet<i64>,
    desktop_windows: HashSet<(i64, u64)>,
    desktop_display: bool,
    readable_paths: HashSet<String>,
    writable_paths: HashSet<String>,
    readable_roots: Vec<PathGrant>,
    writable_roots: Vec<PathGrant>,
    terminable_pids: HashSet<i64>,
    configuration_changes: Vec<(String, serde_json::Value)>,
    last_authorized_dispatch: Arc<Mutex<Instant>>,
    idle_expired: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct ApplicationGrant {
    bundle_id: Option<String>,
    executable: Option<String>,
    #[allow(dead_code)]
    launch: bool,
    all_windows: bool,
    terminate: TerminationGrant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminationGrant {
    Deny,
    DriverLaunched,
    Any,
}

#[derive(Debug, Clone)]
struct PathGrant {
    root: PathBuf,
    recursive: bool,
}

impl SessionManifest {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn expires_unix_ms(&self) -> u128 {
        self.expires_unix_ms
    }

    pub fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.allow.len(), self.deny.len(), self.ask.len())
    }

    pub fn decision(&self, tool: &str) -> ManifestDecision {
        let tool = canonical_tool_name(tool);
        if self.deny.contains(tool) {
            ManifestDecision::Deny
        } else if self.ask.contains(tool) {
            ManifestDecision::Ask
        } else if self.allow.contains(tool) {
            ManifestDecision::Allow
        } else {
            ManifestDecision::Undeclared
        }
    }

    /// Enforce resource fields for adapters that have completed migration.
    /// An allow-list entry for a tool never implies access to an undeclared
    /// browser process, native window, or origin.
    pub fn authorize_call(&self, tool: &str, args: &serde_json::Value) -> Result<(), String> {
        match tool {
            "browser_prepare"
                if args
                    .pointer("/strategy/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("existing_profile") =>
            {
                let pid = args
                    .get("pid")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| "bounded existing-profile access requires pid".to_owned())?;
                let window_id = args
                    .get("window_id")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| {
                        "bounded existing-profile access requires window_id".to_owned()
                    })?;
                if !self.existing_profile_kind
                    && !self.existing_profiles.contains(&(pid, window_id))
                {
                    return Err(format!(
                        "browser pid {pid} window {window_id} is outside the bounded session policy"
                    ));
                }
            }
            "browser_navigate" => {
                let url = args
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "bounded browser navigation requires url".to_owned())?;
                self.authorize_browser_url(url)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Validate a live top-level document after redirects and before input.
    pub fn authorize_browser_url(&self, raw: &str) -> Result<(), String> {
        let origin = canonical_origin(raw)?;
        if self.browser_origins.contains(&origin) {
            Ok(())
        } else {
            Err("the live browser origin is outside the bounded session policy".to_owned())
        }
    }

    /// Match the implementation-attested resource of an active adapter
    /// against the immutable bounded manifest. A tool allow-list entry alone
    /// never widens the resources approved at launch.
    pub fn authorize_protected_resource(
        &self,
        adapter_id: &str,
        resource: &serde_json::Value,
    ) -> Result<(), String> {
        let kind = resource
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("{adapter_id} did not attest a resource kind"))?;
        let refused = || {
            Err(format!(
                "{adapter_id} resource kind '{kind}' is outside the bounded session policy"
            ))
        };
        match adapter_id {
            "private_observation" => match kind {
                "window" => self.authorize_desktop_window(resource),
                "application" => self.authorize_desktop_application(resource),
                "display" => self.desktop_display.then_some(()).ok_or_else(|| {
                    "desktop display observation is outside the bounded session policy".to_owned()
                }),
                "browser_target" | "authenticated_browser_tab" => {
                    self.authorize_resource_origin(resource)
                }
                // `escalate_session` always expands a browser-scoped session
                // to desktop scope; it has no caller-selectable capture_scope
                // argument. Require the immutable manifest's display grant
                // directly instead of keying enforcement on a field the tool
                // can never send.
                "session_capture_scope" => self.desktop_display.then_some(()).ok_or_else(|| {
                    "desktop capture escalation is outside the bounded session policy".to_owned()
                }),
                "session_trajectory_recording" => Ok(()),
                _ => refused(),
            },
            "desktop_input" => match kind {
                "window_input" => self.authorize_desktop_window(resource),
                "application_input" => self.authorize_desktop_application(resource),
                "display_input" => self.desktop_display.then_some(()).ok_or_else(|| {
                    "desktop-wide input is outside the bounded session policy".to_owned()
                }),
                _ => refused(),
            },
            "file_transfer_and_output" => self.authorize_file_resource(kind, resource),
            "browser_bound_input" | "browser_consequential_action" => {
                self.authorize_resource_origin(resource)
            }
            "process_control" => {
                let pid = resource
                    .pointer("/fingerprint/pid")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| "process control did not attest a pid".to_owned())?;
                let driver_owned = resource
                    .get("driver_owned")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let app_allowed = self.applications.iter().any(|application| {
                    let identity_matches = application
                        .executable
                        .as_deref()
                        .zip(
                            resource
                                .pointer("/fingerprint/executable")
                                .and_then(serde_json::Value::as_str),
                        )
                        .is_some_and(|(allowed, actual)| allowed == actual);
                    identity_matches
                        && match application.terminate {
                            TerminationGrant::Deny => false,
                            TerminationGrant::DriverLaunched => driver_owned,
                            TerminationGrant::Any => true,
                        }
                });
                if self.terminable_pids.contains(&pid) || app_allowed {
                    Ok(())
                } else {
                    Err(format!(
                        "process pid {pid} is outside the bounded session policy"
                    ))
                }
            }
            "driver_configuration" => {
                let changes = resource
                    .get("exact_changes")
                    .and_then(serde_json::Value::as_object)
                    .ok_or_else(|| {
                        "driver configuration did not attest exact changes".to_owned()
                    })?;
                for (key, value) in changes {
                    if !self
                        .configuration_changes
                        .iter()
                        .any(|(allowed_key, allowed_value)| {
                            allowed_key == key && allowed_value == value
                        })
                    {
                        return Err(format!(
                            "driver configuration change '{key}' is outside the bounded session policy"
                        ));
                    }
                }
                Ok(())
            }
            _ => refused(),
        }
    }

    fn authorize_desktop_window(&self, resource: &serde_json::Value) -> Result<(), String> {
        let pid = resource
            .get("pid")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| "window resource did not attest a pid".to_owned())?;
        let window_id = resource
            .get("window_id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "window resource did not attest a window_id".to_owned())?;
        if self.desktop_windows.contains(&(pid, window_id))
            || self.existing_profiles.contains(&(pid, window_id))
            || self.resource_matches_application(resource, true)
        {
            Ok(())
        } else {
            Err(format!(
                "desktop pid {pid} window {window_id} is outside the bounded session policy"
            ))
        }
    }

    fn authorize_desktop_application(&self, resource: &serde_json::Value) -> Result<(), String> {
        let pid = resource
            .get("pid")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| "application resource did not attest a pid".to_owned())?;
        if self.desktop_applications.contains(&pid)
            || self
                .existing_profiles
                .iter()
                .any(|(profile_pid, _)| *profile_pid == pid)
            || self.resource_matches_application(resource, false)
        {
            Ok(())
        } else {
            Err(format!(
                "desktop application pid {pid} is outside the bounded session policy"
            ))
        }
    }

    fn resource_matches_application(
        &self,
        resource: &serde_json::Value,
        require_all_windows: bool,
    ) -> bool {
        let bundle_id = resource
            .get("bundle_id")
            .and_then(serde_json::Value::as_str);
        let executable = resource
            .pointer("/fingerprint/executable")
            .or_else(|| resource.get("executable"))
            .or_else(|| resource.get("launch_path"))
            .and_then(serde_json::Value::as_str);
        self.applications.iter().any(|application| {
            (!require_all_windows || application.all_windows)
                && (application
                    .bundle_id
                    .as_deref()
                    .zip(bundle_id)
                    .is_some_and(|(allowed, actual)| allowed == actual)
                    || application
                        .executable
                        .as_deref()
                        .zip(executable)
                        .is_some_and(|(allowed, actual)| allowed == actual))
        })
    }

    fn authorize_resource_origin(&self, resource: &serde_json::Value) -> Result<(), String> {
        let origin = resource
            .get("live_origin")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "browser resource did not attest a live origin".to_owned())?;
        if self.browser_origins.contains(origin) {
            Ok(())
        } else {
            Err("the live browser origin is outside the bounded session policy".to_owned())
        }
    }

    fn authorize_file_resource(
        &self,
        kind: &str,
        resource: &serde_json::Value,
    ) -> Result<(), String> {
        let all_allowed =
            |values: &serde_json::Value, allowed: &HashSet<String>, roots: &[PathGrant]| {
                values.as_array().is_some_and(|values| {
                    !values.is_empty()
                        && values.iter().all(|value| {
                            value
                                .as_str()
                                .is_some_and(|path| path_allowed(path, allowed, roots))
                        })
                })
            };
        match kind {
            "browser_upload" => {
                if all_allowed(
                    &resource["canonical_paths"],
                    &self.readable_paths,
                    &self.readable_roots,
                ) {
                    Ok(())
                } else {
                    Err(
                        "one or more upload paths are outside the bounded session policy"
                            .to_owned(),
                    )
                }
            }
            "trajectory_replay" => self.authorize_exact_path(
                resource,
                "canonical_source_directory",
                &self.readable_paths,
                &self.readable_roots,
            ),
            "browser_download" => self.authorize_exact_path(
                resource,
                "canonical_destination_root",
                &self.writable_paths,
                &self.writable_roots,
            ),
            "screenshot_output" => self.authorize_exact_path(
                resource,
                "canonical_output_path",
                &self.writable_paths,
                &self.writable_roots,
            ),
            "recording_output" | "recording_finalize" => self.authorize_exact_path(
                resource,
                "canonical_output_directory",
                &self.writable_paths,
                &self.writable_roots,
            ),
            // The exact dependency and confirmation bit are fixed by the
            // typed install_ffmpeg route; the manifest's tool allow is the
            // reviewed bounded decision.
            "dependency_install" => Ok(()),
            _ => Err(format!(
                "file resource kind '{kind}' is outside the bounded session policy"
            )),
        }
    }

    fn authorize_exact_path(
        &self,
        resource: &serde_json::Value,
        key: &str,
        allowed: &HashSet<String>,
        roots: &[PathGrant],
    ) -> Result<(), String> {
        let path = resource
            .get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("file resource did not attest {key}"))?;
        if path_allowed(path, allowed, roots) {
            Ok(())
        } else {
            Err("the exact path is outside the bounded session policy".to_owned())
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_unix_ms < now_unix_ms()
    }

    pub fn is_idle_expired(&self) -> bool {
        self.idle_expired.load(Ordering::Acquire)
    }

    /// Refresh the bounded-mode idle lease only after a call passed every
    /// manifest decision and resource check. Once the idle lease expires it is
    /// terminal for this daemon instance; a tool call cannot revive it.
    pub fn authorize_dispatch(&self) -> Result<(), String> {
        if self.is_idle_expired() {
            return Err("bounded session policy idle timeout exceeded".to_owned());
        }
        let now = Instant::now();
        let mut last = self.last_authorized_dispatch.lock().unwrap();
        if now.duration_since(*last) > self.idle_timeout {
            self.idle_expired.store(true, Ordering::Release);
            return Err("bounded session policy idle timeout exceeded".to_owned());
        }
        *last = now;
        Ok(())
    }
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    version: u32,
    mode: String,
    expires_after: String,
    idle_timeout: String,
    #[serde(default)]
    resources: RawResources,
    #[serde(default)]
    allow: RawToolSet,
    #[serde(default)]
    deny: RawToolSet,
    #[serde(default)]
    ask: RawToolSet,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResources {
    #[serde(default)]
    apps: Vec<RawApplicationResource>,
    #[serde(default)]
    browser: RawBrowserResources,
    #[serde(default)]
    desktop: RawDesktopResources,
    #[serde(default)]
    files: RawFileResources,
    #[serde(default)]
    processes: RawProcessResources,
    #[serde(default)]
    driver_configuration: RawDriverConfigurationResources,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBrowserResources {
    #[serde(default)]
    existing_profiles: Vec<RawExistingProfile>,
    #[serde(default)]
    profiles: Vec<RawBrowserProfile>,
    #[serde(default)]
    origins: Vec<String>,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBrowserProfile {
    kind: String,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawApplicationResource {
    #[serde(default)]
    bundle_id: Option<String>,
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    launch: bool,
    #[serde(default)]
    windows: Option<String>,
    #[serde(default)]
    terminate: Option<String>,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExistingProfile {
    pid: i64,
    window_id: u64,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDesktopResources {
    #[serde(default)]
    applications: Vec<i64>,
    #[serde(default)]
    windows: Vec<RawDesktopWindow>,
    #[serde(default)]
    display: bool,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDesktopWindow {
    pid: i64,
    window_id: u64,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFileResources {
    #[serde(default)]
    read: Vec<RawPathResource>,
    #[serde(default)]
    write: Vec<RawPathResource>,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawPathResource {
    Exact(String),
    Directory {
        dir: String,
        #[serde(default)]
        recursive: bool,
    },
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProcessResources {
    #[serde(default)]
    terminate: Vec<i64>,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDriverConfigurationResources {
    #[serde(default)]
    changes: Vec<RawConfigurationChange>,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfigurationChange {
    key: String,
    value: serde_json::Value,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToolSet {
    #[serde(default)]
    tools: Vec<String>,
}

static CONFIGURED_MANIFEST: OnceLock<Result<Option<SessionManifest>, String>> = OnceLock::new();

pub fn configured_session_manifest() -> Result<Option<&'static SessionManifest>, String> {
    match CONFIGURED_MANIFEST.get_or_init(load_configured_manifest) {
        Ok(manifest) => Ok(manifest.as_ref()),
        Err(error) => Err(error.clone()),
    }
}

fn load_configured_manifest() -> Result<Option<SessionManifest>, String> {
    let Some(path) = std::env::var_os(SESSION_POLICY_FILE_ENV) else {
        return Ok(None);
    };
    load_manifest(Path::new(&path)).map(Some)
}

/// Load and validate an immutable bounded-session manifest for a trusted
/// runtime/session constructor.
pub fn load_manifest(path: &Path) -> Result<SessionManifest, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("failed to read session policy {}: {error}", path.display()))?;
    if bytes.is_empty() {
        return Err(format!("session policy {} is empty", path.display()));
    }
    #[cfg(feature = "yaml")]
    let raw: RawManifest = serde_yaml_ng::from_slice(&bytes)
        .map_err(|error| format!("failed to parse session policy {}: {error}", path.display()))?;
    #[cfg(not(feature = "yaml"))]
    return Err("session policy support requires the yaml feature".to_owned());

    #[cfg(feature = "yaml")]
    {
        let RawManifest {
            version,
            mode,
            expires_after,
            idle_timeout,
            resources,
            allow,
            deny,
            ask,
        } = raw;
        let RawResources {
            apps: raw_applications,
            browser,
            desktop,
            files,
            processes,
            driver_configuration,
        } = resources;
        let RawBrowserResources {
            existing_profiles: raw_existing_profiles,
            profiles: raw_browser_profiles,
            origins: raw_browser_origins,
        } = browser;
        let RawDesktopResources {
            applications: raw_desktop_applications,
            windows: raw_desktop_windows,
            display: desktop_display,
        } = desktop;
        let RawFileResources {
            read: raw_readable_paths,
            write: raw_writable_paths,
        } = files;
        let RawProcessResources {
            terminate: raw_terminable_pids,
        } = processes;
        let RawDriverConfigurationResources {
            changes: raw_configuration_changes,
        } = driver_configuration;

        if !matches!(version, 1 | 2) {
            return Err(format!(
                "unsupported session policy version {}; expected 1 or 2",
                version
            ));
        }
        if !matches!(mode.as_str(), "bounded" | "autonomous") {
            return Err("session policy mode must be bounded".to_owned());
        }
        let expires_after = parse_duration(&expires_after)?;
        let idle_timeout = parse_duration(&idle_timeout)?;
        if expires_after.is_zero() || expires_after > Duration::from_secs(24 * 60 * 60) {
            return Err("expires_after must be greater than zero and no more than 24h".to_owned());
        }
        if idle_timeout.is_zero() || idle_timeout > expires_after {
            return Err(
                "idle_timeout must be greater than zero and no more than expires_after".to_owned(),
            );
        }
        let allow = validate_tools("allow", allow.tools)?;
        let deny = validate_tools("deny", deny.tools)?;
        let ask = validate_tools("ask", ask.tools)?;
        if allow.is_empty() {
            return Err("session policy allow.tools must not be empty".to_owned());
        }
        for tool in &allow {
            if deny.contains(tool) || ask.contains(tool) {
                return Err(format!(
                    "session policy tool '{tool}' appears in more than one decision set"
                ));
            }
        }
        for tool in &deny {
            if ask.contains(tool) {
                return Err(format!(
                    "session policy tool '{tool}' appears in more than one decision set"
                ));
            }
        }
        let mut existing_profiles = HashSet::new();
        for profile in raw_existing_profiles {
            if profile.pid <= 0 || profile.window_id == 0 {
                return Err(
                    "browser existing_profiles entries require positive pid and window_id"
                        .to_owned(),
                );
            }
            if !existing_profiles.insert((profile.pid, profile.window_id)) {
                return Err(format!(
                    "browser existing_profiles repeats pid {} window {}",
                    profile.pid, profile.window_id
                ));
            }
        }
        if version == 1 && !raw_browser_profiles.is_empty() {
            return Err("browser profiles require session policy version 2".to_owned());
        }
        let mut existing_profile_kind = false;
        for profile in raw_browser_profiles {
            match profile.kind.trim() {
                "isolated" | "isolated_new" => {}
                "existing_profile" => existing_profile_kind = true,
                other => {
                    return Err(format!(
                        "unsupported browser profile kind '{other}'; expected isolated or existing_profile"
                    ))
                }
            }
        }
        let mut browser_origins = HashSet::new();
        for raw_origin in raw_browser_origins {
            let origin = canonical_origin(&raw_origin)?;
            if origin != raw_origin.trim().trim_end_matches('/') {
                return Err(format!(
                    "browser origin '{raw_origin}' must be an exact canonical origin without a path"
                ));
            }
            if !browser_origins.insert(origin.clone()) {
                return Err(format!("browser origins repeats '{origin}'"));
            }
        }
        let desktop_applications =
            validate_positive_pids("desktop applications", raw_desktop_applications)?;
        if version == 1 && !raw_applications.is_empty() {
            return Err(
                "application identity resources require session policy version 2".to_owned(),
            );
        }
        let applications = validate_applications(raw_applications)?;
        let mut desktop_windows = HashSet::new();
        for window in raw_desktop_windows {
            if window.pid <= 0 || window.window_id == 0 {
                return Err("desktop windows entries require positive pid and window_id".to_owned());
            }
            if !desktop_windows.insert((window.pid, window.window_id)) {
                return Err(format!(
                    "desktop windows repeats pid {} window {}",
                    window.pid, window.window_id
                ));
            }
        }
        let (readable_paths, readable_roots) =
            validate_manifest_path_resources("files.read", raw_readable_paths, version)?;
        let (writable_paths, writable_roots) =
            validate_manifest_path_resources("files.write", raw_writable_paths, version)?;
        let terminable_pids = validate_positive_pids("processes.terminate", raw_terminable_pids)?;
        let mut configuration_changes = Vec::new();
        for change in raw_configuration_changes {
            let key = change.key.trim();
            if key.is_empty() || key == "capture_scope" {
                return Err(
                    "driver_configuration changes require a non-retired config key".to_owned(),
                );
            }
            if configuration_changes
                .iter()
                .any(|(existing_key, existing_value)| {
                    existing_key == key && existing_value == &change.value
                })
            {
                return Err(format!(
                    "driver_configuration changes repeats exact change '{key}'"
                ));
            }
            configuration_changes.push((key.to_owned(), change.value));
        }
        if !browser_origins.is_empty() {
            const ORIGIN_BYPASS_TOOLS: &[&str] = &[
                "page",
                "click",
                "double_click",
                "right_click",
                "drag",
                "scroll",
                "type_text",
                "press_key",
                "hotkey",
                "set_value",
                "mouse_button_down",
                "mouse_button_up",
                "mouse_drag",
                "parallel_mouse_drag",
                "get_accessibility_tree",
                "get_window_state",
                "verify_state",
                "get_desktop_state",
            ];
            if let Some(tool) = ORIGIN_BYPASS_TOOLS
                .iter()
                .find(|tool| allow.contains(**tool))
            {
                return Err(format!(
                    "origin-scoped bounded manifests cannot allow '{tool}' because it bypasses the typed browser origin adapter"
                ));
            }
        }

        let mut digest = Sha256::new();
        digest.update(format!("cua-driver-session-policy-v{version}\0").as_bytes());
        digest.update(&bytes);
        Ok(SessionManifest {
            version,
            sha256: format!("{:x}", digest.finalize()),
            expires_unix_ms: now_unix_ms() + expires_after.as_millis(),
            idle_timeout,
            allow,
            deny,
            ask,
            existing_profiles,
            existing_profile_kind,
            browser_origins,
            applications,
            desktop_applications,
            desktop_windows,
            desktop_display,
            readable_paths,
            writable_paths,
            readable_roots,
            writable_roots,
            terminable_pids,
            configuration_changes,
            last_authorized_dispatch: Arc::new(Mutex::new(Instant::now())),
            idle_expired: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[cfg(feature = "yaml")]
fn validate_tools(section: &str, tools: Vec<String>) -> Result<HashSet<String>, String> {
    let mut validated = HashSet::new();
    for tool in tools {
        let canonical = canonical_tool_name(tool.trim()).to_owned();
        if canonical.is_empty()
            || crate::authorization::advertised_risk_for(&canonical).class
                == crate::authorization::RiskClass::Unclassified
        {
            return Err(format!(
                "session policy {section}.tools contains unknown or unreviewed tool '{tool}'"
            ));
        }
        if !validated.insert(canonical.clone()) {
            return Err(format!(
                "session policy {section}.tools repeats '{canonical}'"
            ));
        }
    }
    Ok(validated)
}

fn canonical_tool_name(tool: &str) -> &str {
    match tool {
        "type_text_chars" => "type_text",
        other => other,
    }
}

#[cfg(feature = "yaml")]
fn validate_positive_pids(section: &str, pids: Vec<i64>) -> Result<HashSet<i64>, String> {
    let mut validated = HashSet::new();
    for pid in pids {
        if pid <= 0 {
            return Err(format!("{section} entries must be positive pids"));
        }
        if !validated.insert(pid) {
            return Err(format!("{section} repeats pid {pid}"));
        }
    }
    Ok(validated)
}

#[cfg(feature = "yaml")]
fn validate_applications(
    applications: Vec<RawApplicationResource>,
) -> Result<Vec<ApplicationGrant>, String> {
    let mut validated = Vec::new();
    for raw in applications {
        let bundle_id = raw
            .bundle_id
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let executable = raw
            .executable
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if bundle_id.is_some() == executable.is_some() {
            return Err(
                "each application resource must declare exactly one of bundle_id or executable"
                    .to_owned(),
            );
        }
        let executable = executable
            .map(|path| {
                let path = Path::new(&path);
                if !path.is_absolute() {
                    return Err(
                        "application executable identities must be canonical absolute paths"
                            .to_owned(),
                    );
                }
                canonical_manifest_path(path)
            })
            .transpose()?;
        let all_windows = match raw.windows.as_deref().unwrap_or("all") {
            "all" => true,
            other => {
                return Err(format!(
                    "unsupported application windows scope '{other}'; expected all"
                ))
            }
        };
        let terminate = match raw.terminate.as_deref().unwrap_or("deny") {
            "deny" => TerminationGrant::Deny,
            "driver_launched" => TerminationGrant::DriverLaunched,
            "any" => TerminationGrant::Any,
            other => {
                return Err(format!(
                    "unsupported application terminate scope '{other}'; expected deny, driver_launched, or any"
                ))
            }
        };
        if validated.iter().any(|application: &ApplicationGrant| {
            application.bundle_id == bundle_id && application.executable == executable
        }) {
            return Err("application resources repeat an identity".to_owned());
        }
        validated.push(ApplicationGrant {
            bundle_id,
            executable,
            launch: raw.launch,
            all_windows,
            terminate,
        });
    }
    Ok(validated)
}

#[cfg(feature = "yaml")]
fn validate_manifest_path_resources(
    section: &str,
    resources: Vec<RawPathResource>,
    version: u32,
) -> Result<(HashSet<String>, Vec<PathGrant>), String> {
    let mut exact = HashSet::new();
    let mut roots = Vec::new();
    for resource in resources {
        match resource {
            RawPathResource::Exact(path) => {
                exact.extend(validate_manifest_paths(section, vec![path])?);
            }
            RawPathResource::Directory { dir, recursive } => {
                if version < 2 {
                    return Err(format!(
                        "{section} directory roots require session policy version 2"
                    ));
                }
                let canonical = validate_manifest_paths(section, vec![dir])?
                    .into_iter()
                    .next()
                    .expect("one validated path");
                let root = PathBuf::from(&canonical);
                if root.exists() && !root.is_dir() {
                    return Err(format!("{section} directory root must name a directory"));
                }
                if roots
                    .iter()
                    .any(|grant: &PathGrant| grant.root == root && grant.recursive == recursive)
                {
                    return Err(format!("{section} repeats directory root '{canonical}'"));
                }
                roots.push(PathGrant { root, recursive });
            }
        }
    }
    Ok((exact, roots))
}

#[cfg(feature = "yaml")]
fn validate_manifest_paths(section: &str, paths: Vec<String>) -> Result<HashSet<String>, String> {
    let mut validated = HashSet::new();
    for raw in paths {
        let trimmed = raw.trim();
        let path = Path::new(trimmed);
        if !path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(format!(
                "{section} path '{raw}' must be absolute and contain no '..' components"
            ));
        }
        if path.parent().is_none() {
            return Err(format!(
                "{section} path '{raw}' must not name a filesystem root"
            ));
        }
        if trimmed.len()
            > path
                .components()
                .next()
                .map_or(0, |root| root.as_os_str().len())
            && (trimmed.ends_with('/') || trimmed.ends_with('\\'))
        {
            return Err(format!(
                "{section} path '{raw}' must not end with a directory separator"
            ));
        }
        let normalized = path.to_string_lossy().into_owned();
        if normalized != trimmed {
            return Err(format!(
                "{section} path '{raw}' must use its normalized spelling"
            ));
        }
        let canonical = canonical_manifest_path(path).map_err(|error| {
            format!("{section} path '{raw}' could not be resolved safely: {error}")
        })?;
        if !validated.insert(canonical.clone()) {
            return Err(format!("{section} repeats path '{canonical}'"));
        }
    }
    Ok(validated)
}

fn path_allowed(path: &str, exact: &HashSet<String>, roots: &[PathGrant]) -> bool {
    if exact.contains(path) {
        return true;
    }
    let candidate = Path::new(path);
    roots.iter().any(|grant| {
        candidate == grant.root
            || if grant.recursive {
                candidate.starts_with(&grant.root)
            } else {
                candidate.parent() == Some(grant.root.as_path())
            }
    })
}

#[cfg(feature = "yaml")]
fn canonical_manifest_path(path: &Path) -> Result<String, String> {
    if path.exists() {
        return std::fs::canonicalize(path)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| error.to_string());
    }

    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        match std::fs::symlink_metadata(existing) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("path contains a broken symbolic link".to_owned())
            }
            Ok(_) => return Err("path contains an unavailable filesystem entry".to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let name = existing
            .file_name()
            .ok_or_else(|| "path has no existing ancestor".to_owned())?;
        suffix.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| "path has no existing ancestor".to_owned())?;
    }
    let metadata = std::fs::symlink_metadata(existing).map_err(|error| error.to_string())?;
    if !metadata.is_dir() {
        return Err("existing ancestor is not a directory".to_owned());
    }
    let mut canonical = std::fs::canonicalize(existing).map_err(|error| error.to_string())?;
    for component in suffix.into_iter().rev() {
        let component_path = PathBuf::from(&component);
        if !matches!(
            component_path.components().next(),
            Some(Component::Normal(_))
        ) || component_path.components().count() != 1
        {
            return Err("path contains an unsafe component".to_owned());
        }
        canonical.push(component);
    }
    Ok(canonical.to_string_lossy().into_owned())
}

fn canonical_origin(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value == "about:blank" {
        return Ok(value.to_owned());
    }
    let parsed =
        url::Url::parse(value).map_err(|_| format!("invalid browser URL or origin '{raw}'"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "browser URL or origin '{raw}' must use http or https"
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("browser URL or origin '{raw}' has no host"))?;
    let mut origin = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    Ok(origin)
}

#[cfg(any(feature = "yaml", test))]
fn parse_duration(raw: &str) -> Result<Duration, String> {
    let value = raw.trim();
    if value.len() < 2 {
        return Err(format!(
            "invalid duration '{raw}'; use forms such as 30m or 8h"
        ));
    }
    let (number, unit) = value.split_at(value.len() - 1);
    let number = number
        .parse::<u64>()
        .map_err(|_| format!("invalid duration '{raw}'; use forms such as 30m or 8h"))?;
    let seconds = match unit {
        "s" => number,
        "m" => number.saturating_mul(60),
        "h" => number.saturating_mul(60 * 60),
        _ => {
            return Err(format!(
                "invalid duration unit in '{raw}'; expected s, m, or h"
            ))
        }
    };
    Ok(Duration::from_secs(seconds))
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "yaml")]
    fn manifest(source: &str) -> Result<SessionManifest, String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.yaml");
        std::fs::write(&path, source).unwrap();
        load_manifest(&path)
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn manifest_is_deny_by_default_and_preserves_ask() {
        let loaded = manifest(
            r#"
version: 1
mode: bounded
expires_after: 8h
idle_timeout: 30m
resources: {}
allow:
  tools: [start_session, get_browser_state]
deny:
  tools: [page]
ask:
  tools: [browser_download]
"#,
        )
        .unwrap();
        assert_eq!(loaded.decision("start_session"), ManifestDecision::Allow);
        assert_eq!(loaded.decision("page"), ManifestDecision::Deny);
        assert_eq!(loaded.decision("browser_download"), ManifestDecision::Ask);
        assert_eq!(loaded.decision("click"), ManifestDecision::Undeclared);
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn autonomous_manifest_mode_remains_a_compatibility_alias() {
        let loaded = manifest(
            "version: 1\nmode: autonomous\nexpires_after: 1h\nidle_timeout: 5m\nallow:\n  tools: [get_config]\n",
        )
        .unwrap();
        assert_eq!(loaded.decision("get_config"), ManifestDecision::Allow);
        assert_eq!(loaded.version(), 1);
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn version_two_supports_prelaunch_identities_profile_kinds_and_directory_roots() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input");
        let output = root.path().join("output");
        std::fs::create_dir_all(input.join("nested")).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        let executable = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let input = std::fs::canonicalize(input).unwrap();
        let output = std::fs::canonicalize(output).unwrap();
        let loaded = manifest(&format!(
            r#"
version: 2
mode: bounded
expires_after: 8h
idle_timeout: 30m
resources:
  apps:
    - executable: {executable:?}
      launch: true
      windows: all
      terminate: driver_launched
  browser:
    profiles:
      - kind: isolated
      - kind: existing_profile
  files:
    read:
      - dir: {input:?}
        recursive: true
    write:
      - dir: {output:?}
        recursive: false
allow:
  tools:
    - browser_prepare
    - get_window_state
    - kill_app
    - browser_set_input_files
    - get_desktop_state
"#,
            executable = executable.to_string_lossy(),
            input = input.to_string_lossy(),
            output = output.to_string_lossy(),
        ))
        .unwrap();

        assert_eq!(loaded.version(), 2);
        loaded
            .authorize_call(
                "browser_prepare",
                &serde_json::json!({
                    "pid": 42,
                    "window_id": 7,
                    "strategy": {"kind": "existing_profile"}
                }),
            )
            .unwrap();
        loaded
            .authorize_protected_resource(
                "private_observation",
                &serde_json::json!({
                    "kind": "window",
                    "pid": 42,
                    "window_id": 7,
                    "fingerprint": {
                        "pid": 42,
                        "start_time": 1,
                        "executable": executable
                    }
                }),
            )
            .unwrap();
        loaded
            .authorize_protected_resource(
                "process_control",
                &serde_json::json!({
                    "kind": "process_instance",
                    "driver_owned": true,
                    "fingerprint": {
                        "pid": 42,
                        "start_time": 1,
                        "executable": executable
                    }
                }),
            )
            .unwrap();
        assert!(loaded
            .authorize_protected_resource(
                "process_control",
                &serde_json::json!({
                    "kind": "process_instance",
                    "driver_owned": false,
                    "fingerprint": {
                        "pid": 42,
                        "start_time": 1,
                        "executable": executable
                    }
                }),
            )
            .is_err());
        loaded
            .authorize_protected_resource(
                "file_transfer_and_output",
                &serde_json::json!({
                    "kind": "browser_upload",
                    "canonical_paths": [input.join("nested").join("fixture.txt")]
                }),
            )
            .unwrap();
        loaded
            .authorize_protected_resource(
                "file_transfer_and_output",
                &serde_json::json!({
                    "kind": "screenshot_output",
                    "canonical_output_path": output.join("capture.png")
                }),
            )
            .unwrap();
        assert!(loaded
            .authorize_protected_resource(
                "file_transfer_and_output",
                &serde_json::json!({
                    "kind": "screenshot_output",
                    "canonical_output_path": output.join("nested").join("capture.png")
                }),
            )
            .is_err());
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn browser_resources_bind_existing_profile_and_origins() {
        let manifest = manifest(
            r#"
version: 1
mode: bounded
expires_after: 8h
idle_timeout: 30m
resources:
  browser:
    existing_profiles:
      - pid: 42
        window_id: 7
    origins:
      - https://app.example.com
allow:
  tools: [browser_prepare, browser_navigate]
"#,
        )
        .unwrap();

        manifest
            .authorize_call(
                "browser_prepare",
                &serde_json::json!({
                    "pid": 42,
                    "window_id": 7,
                    "strategy": {"kind": "existing_profile"}
                }),
            )
            .unwrap();
        assert!(manifest
            .authorize_call(
                "browser_prepare",
                &serde_json::json!({
                    "pid": 42,
                    "window_id": 8,
                    "strategy": {"kind": "existing_profile"}
                }),
            )
            .is_err());
        manifest
            .authorize_browser_url("https://app.example.com/work?q=1")
            .unwrap();
        manifest
            .authorize_protected_resource(
                "private_observation",
                &serde_json::json!({"kind": "window", "pid": 42, "window_id": 7}),
            )
            .unwrap();
        manifest
            .authorize_protected_resource(
                "desktop_input",
                &serde_json::json!({"kind": "application_input", "pid": 42}),
            )
            .unwrap();
        assert!(manifest
            .authorize_browser_url("https://evil.example/redirect")
            .is_err());
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn about_blank_uses_one_manifest_and_protected_resource_spelling() {
        let manifest = manifest(
            r#"
version: 1
mode: bounded
expires_after: 8h
idle_timeout: 30m
resources:
  browser:
    origins: [about:blank]
allow:
  tools: [browser_click]
"#,
        )
        .unwrap();

        manifest.authorize_browser_url("about:blank").unwrap();
        manifest
            .authorize_protected_resource(
                "browser_bound_input",
                &serde_json::json!({
                    "kind": "authenticated_browser_tab",
                    "live_origin": "about:blank"
                }),
            )
            .unwrap();
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn desktop_escalation_requires_the_manifest_display_grant() {
        let without_display = manifest(
            r#"
version: 1
mode: bounded
expires_after: 8h
idle_timeout: 30m
resources: {}
allow:
  tools: [escalate_session]
"#,
        )
        .unwrap();
        assert!(without_display
            .authorize_protected_resource(
                "private_observation",
                &serde_json::json!({
                    "kind": "session_capture_scope",
                    "capture_scope": "desktop"
                }),
            )
            .is_err());

        let with_display = manifest(
            r#"
version: 1
mode: bounded
expires_after: 8h
idle_timeout: 30m
resources:
  desktop:
    display: true
allow:
  tools: [escalate_session]
"#,
        )
        .unwrap();
        with_display
            .authorize_protected_resource(
                "private_observation",
                &serde_json::json!({
                    "kind": "session_capture_scope",
                    "capture_scope": "desktop"
                }),
            )
            .unwrap();
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn protected_resources_are_exactly_bounded_by_the_manifest() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("cua-input.txt");
        let output = root.path().join("cua-output.png");
        let outside = root.path().join("other.png");
        let input_yaml = serde_json::to_string(&input.to_string_lossy()).unwrap();
        let output_yaml = serde_json::to_string(&output.to_string_lossy()).unwrap();
        let canonical_input = canonical_manifest_path(&input).unwrap();
        let canonical_outside = canonical_manifest_path(&outside).unwrap();
        let loaded = manifest(&format!(
            r#"
version: 1
mode: bounded
expires_after: 8h
idle_timeout: 30m
resources:
  browser:
    origins: [https://app.example.com]
  desktop:
    applications: [42]
    windows:
      - pid: 42
        window_id: 7
    display: false
  files:
    read: [{}]
    write: [{}]
  processes:
    terminate: [42]
  driver_configuration:
    changes:
      - key: max_image_dimension
        value: 1200
allow:
  tools:
    - browser_click
    - browser_set_input_files
    - kill_app
    - set_config
"#,
            input_yaml, output_yaml
        ))
        .unwrap();

        loaded
            .authorize_protected_resource(
                "private_observation",
                &serde_json::json!({"kind": "window", "pid": 42, "window_id": 7}),
            )
            .unwrap();
        assert!(loaded
            .authorize_protected_resource(
                "private_observation",
                &serde_json::json!({"kind": "window", "pid": 42, "window_id": 8}),
            )
            .is_err());
        assert!(loaded
            .authorize_protected_resource(
                "private_observation",
                &serde_json::json!({"kind": "display"}),
            )
            .is_err());

        loaded
            .authorize_protected_resource(
                "browser_bound_input",
                &serde_json::json!({
                    "kind": "authenticated_browser_tab",
                    "live_origin": "https://app.example.com"
                }),
            )
            .unwrap();
        assert!(loaded
            .authorize_protected_resource(
                "browser_bound_input",
                &serde_json::json!({
                    "kind": "authenticated_browser_tab",
                    "live_origin": "https://other.example.com"
                }),
            )
            .is_err());

        loaded
            .authorize_protected_resource(
                "file_transfer_and_output",
                &serde_json::json!({
                    "kind": "browser_upload",
                    "canonical_paths": [canonical_input]
                }),
            )
            .unwrap();
        assert!(loaded
            .authorize_protected_resource(
                "file_transfer_and_output",
                &serde_json::json!({
                    "kind": "screenshot_output",
                    "canonical_output_path": canonical_outside
                }),
            )
            .is_err());

        loaded
            .authorize_protected_resource(
                "process_control",
                &serde_json::json!({
                    "kind": "process_instance",
                    "fingerprint": {"pid": 42}
                }),
            )
            .unwrap();
        assert!(loaded
            .authorize_protected_resource(
                "process_control",
                &serde_json::json!({
                    "kind": "process_instance",
                    "fingerprint": {"pid": 43}
                }),
            )
            .is_err());

        loaded
            .authorize_protected_resource(
                "driver_configuration",
                &serde_json::json!({
                    "kind": "driver_configuration",
                    "exact_changes": {"max_image_dimension": 1200}
                }),
            )
            .unwrap();
        assert!(loaded
            .authorize_protected_resource(
                "driver_configuration",
                &serde_json::json!({
                    "kind": "driver_configuration",
                    "exact_changes": {"max_image_dimension": 800}
                }),
            )
            .is_err());
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn manifest_file_resources_reject_filesystem_roots() {
        let root = std::path::Path::new(std::path::MAIN_SEPARATOR_STR)
            .to_string_lossy()
            .into_owned();
        let error = validate_manifest_paths("files.write", vec![root]).unwrap_err();
        assert!(error.contains("must not name a filesystem root"));
    }

    #[cfg(all(feature = "yaml", unix))]
    #[test]
    fn manifest_file_resources_reject_broken_symlink_leaves() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("broken-output");
        symlink(dir.path().join("missing-target"), &link).unwrap();
        let error =
            validate_manifest_paths("files.write", vec![link.to_string_lossy().into_owned()])
                .unwrap_err();
        assert!(error.contains("broken symbolic link"));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn idle_timeout_is_terminal_and_origin_bypass_tools_are_rejected() {
        let loaded = manifest(
            r#"
version: 1
mode: bounded
expires_after: 1h
idle_timeout: 1s
resources: {}
allow:
  tools: [get_config]
"#,
        )
        .unwrap();
        *loaded.last_authorized_dispatch.lock().unwrap() =
            Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
        assert!(loaded.authorize_dispatch().is_err());
        assert!(loaded.is_idle_expired());
        assert!(loaded.authorize_dispatch().is_err());

        for bypass_tool in ["page", "verify_state"] {
            let policy = format!(
                r#"
version: 1
mode: bounded
expires_after: 1h
idle_timeout: 10m
resources:
  browser:
    origins: [https://app.example.com]
allow:
  tools: [browser_navigate, {bypass_tool}]
"#
            );
            assert!(
                manifest(&policy)
                    .unwrap_err()
                    .contains("bypasses the typed browser origin adapter"),
                "{bypass_tool} must not bypass origin-scoped browser policy"
            );
        }
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn unknown_and_overlapping_tools_are_rejected() {
        let unknown = manifest(
            "version: 1\nmode: bounded\nexpires_after: 1h\nidle_timeout: 5m\nallow:\n  tools: [future_tool]\n",
        )
        .unwrap_err();
        assert!(unknown.contains("unknown or unreviewed"));

        let overlap = manifest(
            "version: 1\nmode: bounded\nexpires_after: 1h\nidle_timeout: 5m\nallow:\n  tools: [click]\ndeny:\n  tools: [click]\n",
        )
        .unwrap_err();
        assert!(overlap.contains("more than one decision set"));
    }

    #[test]
    fn duration_parser_is_bounded_and_explicit() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert!(parse_duration("30").is_err());
        assert!(parse_duration("1d").is_err());
    }
}
