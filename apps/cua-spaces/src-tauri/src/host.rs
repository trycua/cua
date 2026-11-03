// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "This machine" as a host, and the first-run onboarding choice.
//!
//! The commands are thin: [`HostCommands`] holds a [`HostBackend`] (the
//! `cua-host` library in the app, a fake in tests) plus the onboarding store.
//! Host setup installs cua-spacesd as a per-OS service that joins the cua.ai
//! relay (default) or listens on a direct `ip:port`; the account token comes
//! from the app's own sign-in session.
//!
//! Installer mode: `--mode host|client` on the app's command line (the
//! installers pass it on first launch) or the MDM file
//! `<cua home>/spaces-install-mode` preselects the onboarding choice. The user
//! still confirms in the app; nothing is installed without that.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// MDM / installer file preselecting the onboarding choice (`host`|`client`).
pub const INSTALL_MODE_FILE: &str = "spaces-install-mode";

/// What the webview sends to `host_setup`, and its validation: the app
/// core's, shared with the SwiftUI app.
pub use cua_spaces_app_core::host::{validate_setup, HostSetupRequest};

/// `ServiceState` of the contract.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServiceStateView {
    pub installed: bool,
    pub running: bool,
    pub kind: String,
    pub detail: String,
}

/// A client connected to this machine through the relay (presence).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectedClientView {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub streams: u32,
    pub since: u64,
}

/// An OS permission the user must grant (macOS TCC panes).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionHintView {
    pub id: String,
    /// `title` in cua-host.
    #[serde(alias = "title")]
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted: Option<bool>,
}

/// One access to this machine (the driver's access log).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessRecordView {
    pub at_ms: u64,
    pub via: String,
    pub who: String,
    pub what: String,
}

/// A Space this machine provides to one of your devices.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProvidedSpaceView {
    pub relay_machine: String,
    pub local_space: String,
    pub name: String,
    pub image: String,
    pub os: String,
    pub kind: String,
    pub created_by: String,
    pub created_at_ms: u64,
}

/// One line of this machine's Spaces audit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpacesAuditView {
    pub at_ms: u64,
    pub action: String,
    pub who: String,
    pub space: String,
    pub detail: String,
}

/// A change to this machine's two settings (`None` keeps a value): the app
/// core's `host.settingChange` for a toggle.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostSettingChange {
    pub share_desktop: Option<bool>,
    pub provide_spaces: Option<bool>,
}

/// `HostStatus` of the contract, as the webview sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostStatusView {
    pub configured: bool,
    pub mode: Option<String>,
    pub relay_url: Option<String>,
    pub direct_url: Option<String>,
    pub machine_id: Option<String>,
    pub name: Option<String>,
    pub sharing: bool,
    pub service: ServiceStateView,
    pub online: Option<bool>,
    pub clients: Vec<ConnectedClientView>,
    pub permissions: Vec<PermissionHintView>,
    pub error: Option<String>,
    pub recent_access: Vec<AccessRecordView>,
    pub access_log_error: Option<String>,
    pub share_desktop: bool,
    pub provide_spaces: bool,
    pub max_spaces: u32,
    pub max_macos_vms: u32,
    pub provided_spaces: Vec<ProvidedSpaceView>,
    pub spaces_audit: Vec<SpacesAuditView>,
    pub spaces_audit_error: Option<String>,
}

impl HostStatusView {
    /// Not configured, with an optional reason.
    pub fn unconfigured(error: Option<String>) -> Self {
        Self {
            error,
            ..Self::default()
        }
    }

    /// Converts any serializable status with the contract's camelCase shape
    /// (the `cua-host` library's `HostStatus`).
    pub fn from_serializable(status: &impl Serialize) -> Result<Self, String> {
        serde_json::to_value(status)
            .and_then(serde_json::from_value)
            .map_err(|e| format!("host status: {e}"))
    }
}

/// Host operations behind the commands.
#[async_trait]
pub trait HostBackend: Send + Sync {
    async fn status(&self) -> Result<HostStatusView, String>;
    async fn setup(&self, request: HostSetupRequest) -> Result<HostStatusView, String>;
    async fn stop_sharing(&self) -> Result<HostStatusView, String>;
    async fn start_sharing(&self) -> Result<HostStatusView, String>;
    async fn remove(&self) -> Result<(), String>;
    /// Changes what this machine shares: its desktop, and Spaces for your
    /// other devices.
    async fn configure(&self, change: HostSettingChange) -> Result<HostStatusView, String>;
}

/// First-run choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnboardingMode {
    Client,
    Host,
}

impl OnboardingMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "client" | "access" => Some(Self::Client),
            "host" | "unattended" => Some(Self::Host),
            _ => None,
        }
    }
}

/// What `onboarding_state` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingState {
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<OnboardingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installer_mode: Option<OnboardingMode>,
}

#[derive(Default, Serialize, Deserialize)]
struct StoredOnboarding {
    completed: bool,
    #[serde(default)]
    mode: Option<OnboardingMode>,
}

/// Onboarding state persisted in the app's config directory.
#[derive(Clone, Debug)]
pub struct OnboardingStore {
    path: PathBuf,
}

impl OnboardingStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `<config dir>/onboarding.json`.
    pub fn in_dir(dir: &Path) -> Self {
        Self::new(dir.join("onboarding.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The first run finished.
    pub fn completed(&self) -> bool {
        self.load().completed
    }

    fn load(&self) -> StoredOnboarding {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn complete(&self, mode: OnboardingMode) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let body = serde_json::to_vec_pretty(&StoredOnboarding {
            completed: true,
            mode: Some(mode),
        })
        .map_err(|e| e.to_string())?;
        std::fs::write(&self.path, body).map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// `--mode host|client` / `--mode=host` in the process arguments.
pub fn installer_mode_from_args<I, S>(args: I) -> Option<OnboardingMode>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg.as_ref();
        if let Some(value) = arg.strip_prefix("--mode=") {
            return OnboardingMode::parse(value);
        }
        if arg == "--mode" {
            return args.next().and_then(|v| OnboardingMode::parse(v.as_ref()));
        }
    }
    None
}

/// Machine-wide install-mode files an MDM / system installer may write
/// (the per-user `<cua home>/spaces-install-mode` wins over these).
pub fn system_install_mode_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    return vec![PathBuf::from("/Library/Application Support/Cua").join(INSTALL_MODE_FILE)];
    #[cfg(target_os = "windows")]
    return vec![std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("Cua")
        .join(INSTALL_MODE_FILE)];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return vec![PathBuf::from("/etc/cua").join(INSTALL_MODE_FILE)];
}

/// Parses an install-mode file: a bare `host` / `client`, or the MSI's INI
/// form (`[spaces]` / `mode=host`).
pub fn parse_install_mode(content: &str) -> Option<OnboardingMode> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty() && !l.starts_with('[') && !l.starts_with('#') && !l.starts_with(';')
        })
        .find_map(|line| {
            let value = match line.split_once('=') {
                Some((key, value)) if key.trim().eq_ignore_ascii_case("mode") => value,
                Some(_) => return None,
                None => line,
            };
            OnboardingMode::parse(value)
        })
}

/// The installer-preselected mode from the per-user file
/// (`<cua home>/spaces-install-mode`), then `system` files.
pub fn installer_mode_from_files(cua_home: &Path, system: &[PathBuf]) -> Option<OnboardingMode> {
    std::iter::once(cua_home.join(INSTALL_MODE_FILE))
        .chain(system.iter().cloned())
        .find_map(|path| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|s| parse_install_mode(&s))
        })
}

/// [`installer_mode_from_files`] with this OS's system locations.
pub fn installer_mode_from_file(cua_home: &Path) -> Option<OnboardingMode> {
    installer_mode_from_files(cua_home, &system_install_mode_paths())
}

/// Settings panes the webview may ask to open (permission hints, and the
/// Cua Volume file system extension's approval in Login Items &
/// Extensions). Anything else goes through `open_external` (http(s) only).
pub fn allowed_settings_url(url: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "x-apple.systempreferences:com.apple.preference.security?Privacy_",
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_",
        "x-apple.systempreferences:com.apple.LoginItems-Settings.extension",
        "ms-settings:privacy",
    ];
    PREFIXES.iter().any(|p| url.starts_with(p))
        && url
            .bytes()
            // No `&`: Windows opens the pane through `cmd /C start`, and
            // cmd would read it as a command separator.
            .all(|b| b.is_ascii_alphanumeric() || b"._-:?=".contains(&b))
}

/// Everything the host/onboarding commands need.
pub struct HostCommands {
    backend: Arc<dyn HostBackend>,
    onboarding: OnboardingStore,
    installer_mode: Option<OnboardingMode>,
}

impl HostCommands {
    pub fn new(
        backend: Arc<dyn HostBackend>,
        onboarding: OnboardingStore,
        installer_mode: Option<OnboardingMode>,
    ) -> Self {
        Self {
            backend,
            onboarding,
            installer_mode,
        }
    }

    pub fn onboarding_state(&self) -> OnboardingState {
        let stored = self.onboarding.load();
        OnboardingState {
            completed: stored.completed,
            mode: stored.mode,
            installer_mode: self.installer_mode,
        }
    }

    pub fn complete_onboarding(&self, mode: &str) -> Result<(), String> {
        let mode = OnboardingMode::parse(mode)
            .ok_or_else(|| format!("unknown onboarding mode {mode:?} (host or client)"))?;
        self.onboarding.complete(mode)
    }

    /// Never fails: an unreachable backend reads as "not configured" with the
    /// reason, so the roster entry can still offer "Set up for access".
    pub async fn status(&self) -> HostStatusView {
        match self.backend.status().await {
            Ok(status) => status,
            Err(error) => HostStatusView::unconfigured(Some(error)),
        }
    }

    pub async fn setup(&self, request: HostSetupRequest) -> Result<HostStatusView, String> {
        let request = validate_setup(request)?;
        self.backend.setup(request).await
    }

    pub async fn stop_sharing(&self) -> Result<HostStatusView, String> {
        self.backend.stop_sharing().await
    }

    pub async fn start_sharing(&self) -> Result<HostStatusView, String> {
        self.backend.start_sharing().await
    }

    pub async fn remove(&self) -> Result<(), String> {
        self.backend.remove().await
    }

    pub async fn configure(&self, change: HostSettingChange) -> Result<HostStatusView, String> {
        if change.share_desktop.is_none() && change.provide_spaces.is_none() {
            return Err("nothing to change".into());
        }
        self.backend.configure(change).await
    }
}

/// Backend used when this build has no host support wired in.
pub struct UnavailableHost(pub String);

#[async_trait]
impl HostBackend for UnavailableHost {
    async fn status(&self) -> Result<HostStatusView, String> {
        Ok(HostStatusView::unconfigured(None))
    }
    async fn setup(&self, _: HostSetupRequest) -> Result<HostStatusView, String> {
        Err(self.0.clone())
    }
    async fn stop_sharing(&self) -> Result<HostStatusView, String> {
        Err(self.0.clone())
    }
    async fn start_sharing(&self) -> Result<HostStatusView, String> {
        Err(self.0.clone())
    }
    async fn remove(&self) -> Result<(), String> {
        Err(self.0.clone())
    }
    async fn configure(&self, _: HostSettingChange) -> Result<HostStatusView, String> {
        Err(self.0.clone())
    }
}

/// Opens an allow-listed OS settings pane (only ever on a user click).
pub fn open_settings_url(url: &str) -> Result<(), String> {
    if !allowed_settings_url(url) {
        return Err("not a permission settings pane".into());
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_mode_parsing() {
        assert_eq!(
            installer_mode_from_args(["app", "--mode", "host"]),
            Some(OnboardingMode::Host)
        );
        assert_eq!(
            installer_mode_from_args(["app", "--mode=client"]),
            Some(OnboardingMode::Client)
        );
        assert_eq!(installer_mode_from_args(["app", "--mode", "bogus"]), None);
        assert_eq!(installer_mode_from_args(["app"]), None);
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("system-mode");
        let sys = [system.clone()];
        assert_eq!(installer_mode_from_files(dir.path(), &sys), None);
        // MSI INI form in the machine-wide file.
        std::fs::write(&system, "[spaces]\r\nmode=client\r\n").unwrap();
        assert_eq!(
            installer_mode_from_files(dir.path(), &sys),
            Some(OnboardingMode::Client)
        );
        // The per-user file (NSIS / MDM) wins.
        std::fs::write(dir.path().join(INSTALL_MODE_FILE), "HOST\n").unwrap();
        assert_eq!(
            installer_mode_from_files(dir.path(), &sys),
            Some(OnboardingMode::Host)
        );
        assert_eq!(parse_install_mode("other=host"), None);
        assert_eq!(
            parse_install_mode("# comment\nclient"),
            Some(OnboardingMode::Client)
        );
    }

    #[test]
    fn settings_urls_are_allowlisted() {
        assert!(allowed_settings_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        ));
        assert!(allowed_settings_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        ));
        assert!(!allowed_settings_url("file:///etc/passwd"));
        assert!(!allowed_settings_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_A; rm"
        ));
        assert!(!allowed_settings_url("https://example.com"));
        assert!(allowed_settings_url("ms-settings:privacy-webcam"));
        // Cua Volume's FSKit extension (volume_mount_status.settings_url).
        assert!(allowed_settings_url(
            "x-apple.systempreferences:com.apple.LoginItems-Settings.extension"
        ));
        assert!(allowed_settings_url(
            "x-apple.systempreferences:com.apple.LoginItems-Settings.extension?ExtensionItems"
        ));
        assert!(!allowed_settings_url(
            "x-apple.systempreferences:com.apple.LoginItems-Settings.extension; open -a Terminal"
        ));
        assert!(!allowed_settings_url(
            "x-apple.systempreferences:com.apple.LoginItems-Settings.extensio"
        ));
        for shell in [
            "ms-settings:privacy&calc",
            "ms-settings:privacy|x",
            "ms-settings:privacy^&x",
        ] {
            assert!(!allowed_settings_url(shell), "{shell}");
        }
    }
}
