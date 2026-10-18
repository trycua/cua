// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use serde::{Deserialize, Serialize};

/// Source or destination operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    MacOS,
    Linux,
    Windows,
}

impl Platform {
    /// The platform this code is running on.
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOS
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Linux
        }
    }
}

/// A host-side check proving the application whose session a provider transfers
/// is installed on this machine. Consent UIs use it to decide whether to offer
/// an app for teleport (and whether its policy toggles are enabled) without the
/// UI having to hard-code a per-app install path — it reads this from the
/// provider registry via `cua teleport providers`. On the source (export)
/// machine, `probe` is interpreted for the *current* host platform. Data only:
/// the sender evaluates it (`cua_teleport::is_installed`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallProbe {
    /// A filesystem path to check for existence (e.g. `/Applications/Slack.app`)
    /// or, when `on_path` is set, a bare binary name to resolve on `PATH`.
    pub probe: String,
    /// True when `probe` is a `PATH` binary name rather than a filesystem path.
    pub on_path: bool,
}

impl InstallProbe {
    /// A probe for an installed application bundle at the given absolute path.
    pub fn path(probe: impl Into<String>) -> Self {
        Self {
            probe: probe.into(),
            on_path: false,
        }
    }

    /// A probe for a binary resolvable on `PATH`.
    pub fn on_path(probe: impl Into<String>) -> Self {
        Self {
            probe: probe.into(),
            on_path: true,
        }
    }
}

/// A reference to an application on the source machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRef {
    /// Platform application identifier: a macOS bundle identifier, a Linux
    /// desktop/application id, or a Windows AUMID/executable name.
    pub app_id: String,
    /// Human-readable name for UI display only; never used for matching
    /// authorization decisions.
    pub display_name: String,
    /// The platform the application is running on.
    pub platform: Platform,
}

/// A reference to one window of an application, when a transfer is scoped to
/// a single window (for example one browser window's tabs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRef {
    pub title: String,
    /// Opaque platform window identifier, when known.
    pub native_id: Option<String>,
}

/// How much of the app session to move. Providers may support a subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferScope {
    /// Only the open tabs / open documents.
    TabsOnly,
    /// The full user profile (may include cookies, logins, history).
    FullProfile,
}

impl TransferScope {
    /// Parse the CLI spelling (`tabs` or `full`).
    pub fn from_cli(value: &str) -> Option<Self> {
        match value {
            "tabs" | "tabs-only" | "tabs_only" => Some(Self::TabsOnly),
            "full" | "full-profile" | "full_profile" => Some(Self::FullProfile),
            _ => None,
        }
    }
}

/// One item a transfer would move, as rendered by a consent UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestItem {
    /// Human-readable label ("Cookies and logins", "Open tabs").
    pub label: String,
    /// Bundle-relative path this item will occupy.
    pub rel_path: String,
    /// Estimated size in bytes (0 when unknown).
    pub est_bytes: u64,
    /// Count of concrete things this item holds (tabs, bookmarks, cookies,
    /// logins, history entries), when it can be determined cheaply. A consent
    /// UI prefers "{count} {count_noun}" over the raw byte size; `None` (the
    /// database is locked, the file is missing, or the item is a directory)
    /// falls back to `est_bytes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Plural noun for `count` ("tabs", "bookmarks", "cookies", "logins",
    /// "history entries"); `None` whenever `count` is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_noun: Option<String>,
    /// Whether the item carries credentials or other sensitive data.
    pub sensitive: bool,
    /// Whether a consent UI should pre-check this item. The primary session
    /// state and low-risk configuration default to checked (even when
    /// `sensitive`); bulky or especially private items — a full conversation
    /// history — default to unchecked so they are strictly opt-in. Independent
    /// of `sensitive`, which only drives the warning styling. Defaults to
    /// `true` so existing manifests keep their behavior.
    #[serde(default = "default_true")]
    pub default_checked: bool,
}

/// Serde default for [`ManifestItem::default_checked`].
fn default_true() -> bool {
    true
}

/// What a transfer would move. This is what a consent UI renders
/// ("this will transfer cookies and logins…").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferManifest {
    pub provider_id: String,
    pub app_display_name: String,
    pub scope: TransferScope,
    pub items: Vec<ManifestItem>,
    pub total_est_bytes: u64,
    /// Free-form caveats ("closing Chrome yields a cleaner capture").
    pub notes: Vec<String>,
}

/// A window the destination should restore after launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRestore {
    pub title: Option<String>,
    /// URLs (or documents) the restored window should open.
    pub urls: Vec<String>,
}

/// How to relaunch the imported application on the destination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<String>,
    pub restore_windows: Vec<WindowRestore>,
}
