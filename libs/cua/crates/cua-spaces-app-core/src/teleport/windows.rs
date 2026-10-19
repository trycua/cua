// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Window lists: this machine's open windows (teleported into a Space) and a
//! Space's remote windows (streamed here), their filters and the picker's
//! primary button.

use serde::{Deserialize, Serialize};

/// One open window on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWindow {
    /// CoreGraphics window number.
    pub window_id: u32,
    /// App id.
    pub app_id: String,
    /// App name.
    pub app_name: String,
    /// Window title.
    pub window_title: String,
    /// Teleport can bring the app up in a Space.
    pub supported: bool,
    /// The owning app's bundle (or `.desktop` entry), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_path: Option<String>,
}

/// One app with open windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenApp {
    /// App id.
    pub app_id: String,
    /// App name.
    pub app_name: String,
    /// Supported.
    pub supported: bool,
}

/// One of a Space's windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWindow {
    /// Stable key.
    pub id: String,
    /// App name.
    pub app_name: String,
    /// Title.
    pub title: String,
    /// Visible.
    pub visible: bool,
    /// App id.
    pub app_id: String,
    /// Window handle epoch.
    pub target_epoch: u64,
    /// Width, px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_px: Option<u32>,
    /// Height, px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_px: Option<u32>,
    /// The owning process (what the SDK's `Space.app_icon` looks a macOS
    /// app up by), when the window list reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// One app's remote windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWindowGroup {
    /// App id.
    pub app_id: String,
    /// App name.
    pub app_name: String,
    /// Windows, arrival order.
    pub windows: Vec<RemoteWindow>,
}

fn matches(needle: &str, fields: [&str; 2]) -> bool {
    fields.iter().any(|f| f.to_lowercase().contains(needle))
}

/// Open windows whose app name or title contains `query` (case-insensitive).
pub fn filter_windows(windows: &[OpenWindow], query: &str) -> Vec<OpenWindow> {
    let needle = query.trim().to_lowercase();
    windows
        .iter()
        .filter(|w| needle.is_empty() || matches(&needle, [&w.app_name, &w.window_title]))
        .cloned()
        .collect()
}

/// Distinct apps, first-seen order.
pub fn apps_from_windows(windows: &[OpenWindow]) -> Vec<OpenApp> {
    let mut out: Vec<OpenApp> = Vec::new();
    for w in windows {
        if out.iter().any(|a| a.app_id == w.app_id) {
            continue;
        }
        out.push(OpenApp {
            app_id: w.app_id.clone(),
            app_name: w.app_name.clone(),
            supported: w.supported,
        });
    }
    out
}

/// "Teleport" is live only for a selected, supported window.
pub fn teleport_button_enabled(selected: Option<&OpenWindow>) -> bool {
    selected.is_some_and(|w| w.supported)
}

/// Which side of the window picker is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PickerTab {
    /// This machine's windows, teleported into the Space.
    Space,
    /// The Space's windows, streamed here.
    ThisMac,
}

/// What the primary button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PickerAction {
    /// Teleport the app.
    Teleport,
    /// Stream the remote window here.
    StreamRemote,
    /// Streaming a local window is not built yet: an honest notice.
    StreamLocalSoon,
}

/// The primary button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerPrimary {
    /// "Teleport" / "Stream".
    pub label: String,
    /// Action.
    pub action: PickerAction,
    /// Enabled.
    pub enabled: bool,
}

/// The primary button for a tab and selection (`None`: nothing selected;
/// `Some(supported)`).
pub fn picker_primary(tab: PickerTab, selected_supported: Option<bool>) -> PickerPrimary {
    let p = |label: &str, action, enabled| PickerPrimary {
        label: label.into(),
        action,
        enabled,
    };
    match (tab, selected_supported) {
        (PickerTab::ThisMac, sel) => p("Stream", PickerAction::StreamRemote, sel.is_some()),
        (PickerTab::Space, None) => p("Teleport", PickerAction::Teleport, false),
        (PickerTab::Space, Some(true)) => p("Teleport", PickerAction::Teleport, true),
        (PickerTab::Space, Some(false)) => p("Stream", PickerAction::StreamLocalSoon, true),
    }
}

/// Remote windows matching `query` (app or title).
pub fn filter_remote_windows(windows: &[RemoteWindow], query: &str) -> Vec<RemoteWindow> {
    let needle = query.trim().to_lowercase();
    windows
        .iter()
        .filter(|w| needle.is_empty() || matches(&needle, [&w.app_name, &w.title]))
        .cloned()
        .collect()
}

/// Remote windows grouped by app, first-seen order, z-order kept.
pub fn group_windows_by_app(windows: &[RemoteWindow]) -> Vec<RemoteWindowGroup> {
    let mut groups: Vec<RemoteWindowGroup> = Vec::new();
    for w in windows {
        match groups.iter_mut().find(|g| g.app_id == w.app_id) {
            Some(g) => g.windows.push(w.clone()),
            None => groups.push(RemoteWindowGroup {
                app_id: w.app_id.clone(),
                app_name: w.app_name.clone(),
                windows: vec![w.clone()],
            }),
        }
    }
    groups
}

/// The guest's whole screen (the driver's capture target), not an app window.
pub fn is_screen_target(w: &RemoteWindow) -> bool {
    crate::contains_word(&w.app_name.to_lowercase(), "cua driver")
}

/// "Desktop (1024×768)" when the size is known.
pub fn screen_label(screen: Option<&RemoteWindow>) -> String {
    let size = screen.and_then(|s| Some((s.width_px?, s.height_px?)));
    crate::spaces::stream::desktop_label(size.and_then(|(w, h)| resolution_text(w, h)).as_deref())
}

/// "1280×800", or `None` for an unknown (zero) size.
pub fn resolution_text(width: u32, height: u32) -> Option<String> {
    (width > 0 && height > 0).then(|| format!("{width}\u{d7}{height}"))
}
