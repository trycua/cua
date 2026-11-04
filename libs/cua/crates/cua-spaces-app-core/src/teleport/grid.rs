// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The teleport picker's grid, the same in both apps: one tile per app
//! (Apps), per window of this machine (Open windows) or per window of the
//! Space (From <Space>). A tile is the thumbnail of a live window (the
//! app's frontmost one for an app tile), the app's icon and one line of
//! name; what the app can take is the tooltip, and an app that cannot move
//! is dimmed, as the Tauri app always showed it. The shells load the
//! thumbnails and icons the tile names, lazily, through the SDK's caches.

use super::flow::{Capability, PickerState, sections};
use super::windows::{
    OpenWindow, RemoteWindow, filter_remote_windows, filter_windows, is_screen_target,
};
use crate::spaces::stream::window_label;
use serde::{Deserialize, Serialize};

/// Where a tile's icon comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PickerTileIcon {
    /// This machine's app at `path` (the SDK's `Teleport.appIconPng`).
    Host {
        /// The app bundle or `.desktop` entry.
        path: String,
    },
    /// A Space's app (the SDK's `Space.appIcons`).
    #[serde(rename_all = "camelCase")]
    Guest {
        /// App name.
        app_name: String,
        /// App id.
        app_id: String,
        /// Owning process, 0 when unknown.
        pid: u32,
    },
    /// No icon to show.
    None,
}

/// Where a tile's live preview comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PickerTileThumbnail {
    /// One of this machine's windows (the window-drag preview source, the
    /// SDK's `Teleport.captureWindowThumbnail`).
    #[serde(rename_all = "camelCase")]
    HostWindow {
        /// CoreGraphics window number.
        window_id: u32,
    },
    /// One of the Space's windows (the SDK's `Space.windowThumbnail`).
    #[serde(rename_all = "camelCase")]
    GuestWindow {
        /// Window handle.
        window_id: String,
        /// Its epoch.
        epoch: u64,
    },
    /// No window to preview (the tile shows the icon large).
    None,
}

/// One tile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerTile {
    /// The catalog entry id (Apps) or the window id.
    pub id: String,
    /// One line: the app's name, or the window's title.
    pub title: String,
    /// The tooltip: what the app can take, why not, or the full title.
    pub help: String,
    /// Dimmed and not choosable.
    pub disabled: bool,
    /// Highlighted (keyboard or click).
    pub selected: bool,
    /// The icon.
    pub icon: PickerTileIcon,
    /// The preview.
    pub thumbnail: PickerTileThumbnail,
}

/// A titled run of tiles ("Recent", "Apps", "Not available"); the window
/// tabs have one untitled section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerTileSection {
    /// Title, empty for none.
    pub title: String,
    /// Tiles, in order.
    pub tiles: Vec<PickerTile>,
}

/// A tab's grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerGrid {
    /// Sections, in order.
    pub sections: Vec<PickerTileSection>,
    /// When there are no tiles: "No apps match." and the like.
    pub empty_text: Option<String>,
}

/// The tabs, in order, with their labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PickerGridTab {
    /// The app catalog.
    Apps,
    /// This machine's open windows.
    Windows,
    /// The Space's windows.
    Space,
}

/// One tab of the strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerGridTabItem {
    /// Which tab.
    pub tab: PickerGridTab,
    /// Its label.
    pub label: String,
}

/// The tab strip, in order ("From <Space>" names the Space).
pub fn grid_tabs(space_name: &str) -> Vec<PickerGridTabItem> {
    [
        (PickerGridTab::Apps, "Apps".to_string()),
        (PickerGridTab::Windows, "Open windows".to_string()),
        (PickerGridTab::Space, format!("From {space_name}")),
    ]
    .into_iter()
    .map(|(tab, label)| PickerGridTabItem { tab, label })
    .collect()
}

/// The grid's primary button: "Continue" on Apps, "Teleport to <Space>" on
/// Open windows, "Stream to This Mac" on From <Space>; live while a
/// choosable tile is selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerGridPrimary {
    /// The label.
    pub label: String,
    /// Enabled.
    pub enabled: bool,
}

/// The primary button for `tab`'s `grid`.
pub fn grid_primary(tab: PickerGridTab, space_name: &str, grid: &PickerGrid) -> PickerGridPrimary {
    use super::windows::{PickerTab, picker_primary};
    let label = match tab {
        PickerGridTab::Apps => "Continue".to_string(),
        PickerGridTab::Windows => format!(
            "{} to {space_name}",
            picker_primary(PickerTab::Space, Some(true)).label
        ),
        PickerGridTab::Space => format!(
            "{} to This Mac",
            picker_primary(PickerTab::ThisMac, Some(true)).label
        ),
    };
    let enabled = grid
        .sections
        .iter()
        .flat_map(|s| s.tiles.iter())
        .any(|t| t.selected && !t.disabled);
    PickerGridPrimary { label, enabled }
}

/// The frontmost of this machine's windows that belongs to `entry`'s app:
/// the same bundle, else the same catalog or bundle id (never a bare name:
/// two apps can share one). `windows` is front to back, as the window list
/// reports it.
fn frontmost<'a>(
    entry: &super::flow::CatalogEntry,
    windows: &'a [OpenWindow],
) -> Option<&'a OpenWindow> {
    let same = |a: &str, b: &str| !a.is_empty() && a.eq_ignore_ascii_case(b);
    windows
        .iter()
        .find(|w| matches!((&w.bundle_path, &entry.host_path), (Some(b), Some(h)) if b == h))
        .or_else(|| {
            windows.iter().find(|w| {
                same(&w.app_id, &entry.id)
                    || entry
                        .host_app_id
                        .as_deref()
                        .is_some_and(|id| same(&w.app_id, id))
            })
        })
}

/// The Apps tab: the catalog's sections (Recent, Apps, Not available) as
/// tiles, each with its app's frontmost window as the preview.
pub fn app_grid(state: &PickerState, windows: &[OpenWindow]) -> PickerGrid {
    let sections: Vec<PickerTileSection> = sections(state)
        .into_iter()
        .map(|sec| PickerTileSection {
            title: sec.title,
            tiles: sec
                .entries
                .iter()
                .map(|e| {
                    let disabled = e.capability == Capability::Unsupported;
                    PickerTile {
                        id: e.id.clone(),
                        title: e.name.clone(),
                        help: if disabled {
                            e.reason
                                .clone()
                                .unwrap_or_else(|| e.capability.label().into())
                        } else {
                            e.capability.label().into()
                        },
                        disabled,
                        selected: state.selected_id.as_deref() == Some(e.id.as_str()),
                        icon: e
                            .host_path
                            .clone()
                            .map_or(PickerTileIcon::None, |path| PickerTileIcon::Host { path }),
                        thumbnail: frontmost(e, windows).map_or(PickerTileThumbnail::None, |w| {
                            PickerTileThumbnail::HostWindow {
                                window_id: w.window_id,
                            }
                        }),
                    }
                })
                .collect(),
        })
        .collect();
    let empty_text = sections.is_empty().then(|| "No apps match.".to_string());
    PickerGrid {
        sections,
        empty_text,
    }
}

/// The Open windows tab: this machine's windows matching `query`.
pub fn window_grid(windows: &[OpenWindow], query: &str, selected: Option<&str>) -> PickerGrid {
    let tiles: Vec<PickerTile> = filter_windows(windows, query)
        .into_iter()
        .map(|w| {
            let title = [w.window_title.trim(), w.app_name.trim()]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or_default()
                .to_string();
            let id = w.window_id.to_string();
            PickerTile {
                selected: selected == Some(id.as_str()),
                id,
                help: if w.window_title.trim().is_empty() {
                    w.app_name.clone()
                } else {
                    format!("{} \u{b7} {}", w.app_name, w.window_title)
                },
                title,
                disabled: false,
                icon: w
                    .bundle_path
                    .clone()
                    .map_or(PickerTileIcon::None, |path| PickerTileIcon::Host { path }),
                thumbnail: PickerTileThumbnail::HostWindow {
                    window_id: w.window_id,
                },
            }
        })
        .collect();
    grid_of(
        tiles,
        windows.is_empty(),
        "No open windows.",
        "No windows match.",
    )
}

/// The From <Space> tab: the Space's windows matching `query` (not the
/// guest's screen target).
pub fn remote_grid(windows: &[RemoteWindow], query: &str, selected: Option<&str>) -> PickerGrid {
    let apps: Vec<RemoteWindow> = windows
        .iter()
        .filter(|w| !is_screen_target(w))
        .cloned()
        .collect();
    let tiles: Vec<PickerTile> = filter_remote_windows(&apps, query)
        .into_iter()
        .map(|w| {
            let title = window_label(&w);
            PickerTile {
                selected: selected == Some(w.id.as_str()),
                id: w.id.clone(),
                help: if w.title.trim().is_empty() {
                    w.app_name.clone()
                } else {
                    format!("{} \u{b7} {}", w.app_name, w.title)
                },
                title,
                disabled: false,
                icon: PickerTileIcon::Guest {
                    app_name: w.app_name.clone(),
                    app_id: w.app_id.clone(),
                    pid: w.pid.unwrap_or(0),
                },
                thumbnail: PickerTileThumbnail::GuestWindow {
                    window_id: w.id.clone(),
                    epoch: w.target_epoch,
                },
            }
        })
        .collect();
    grid_of(
        tiles,
        apps.is_empty(),
        "No open windows in this Space yet.",
        "No windows match.",
    )
}

fn grid_of(tiles: Vec<PickerTile>, none_at_all: bool, empty: &str, no_match: &str) -> PickerGrid {
    let empty_text = tiles
        .is_empty()
        .then(|| if none_at_all { empty } else { no_match }.to_string());
    PickerGrid {
        sections: if tiles.is_empty() {
            vec![]
        } else {
            vec![PickerTileSection {
                title: String::new(),
                tiles,
            }]
        },
        empty_text,
    }
}

/// Keyboard navigation: the tile `delta` places from `selected` among the
/// choosable tiles, in reading order (left/right are +-1, up/down are +-
/// the shell's column count), clamped at the ends; the first tile when
/// nothing is selected yet.
pub fn grid_step(grid: &PickerGrid, selected: Option<&str>, delta: i32) -> Option<String> {
    let ids: Vec<&str> = grid
        .sections
        .iter()
        .flat_map(|s| s.tiles.iter())
        .filter(|t| !t.disabled)
        .map(|t| t.id.as_str())
        .collect();
    if ids.is_empty() {
        return None;
    }
    let Some(at) = selected.and_then(|s| ids.iter().position(|id| *id == s)) else {
        return Some(ids[0].to_string());
    };
    let to = (at as i64 + delta as i64).clamp(0, ids.len() as i64 - 1) as usize;
    Some(ids[to].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teleport::flow::{CatalogEntry, Move, PickerEvent, initial, reduce};

    fn entry(id: &str, name: &str, cap: Capability, path: Option<&str>) -> CatalogEntry {
        CatalogEntry {
            id: id.into(),
            name: name.into(),
            host_path: path.map(Into::into),
            host_app_id: None,
            version: None,
            capability: cap,
            reason: (cap == Capability::Unsupported).then(|| "No provider yet".into()),
            moves: vec![Move::AppOnly],
            provider_id: None,
            sensitive_groups: vec![],
            install_source: None,
            install_id: None,
            install_version: None,
            launch_bin: None,
            last_used_ms: None,
            json: "{}".into(),
        }
    }

    fn window(id: u32, app_id: &str, app: &str, title: &str, bundle: Option<&str>) -> OpenWindow {
        OpenWindow {
            window_id: id,
            app_id: app_id.into(),
            app_name: app.into(),
            window_title: title.into(),
            supported: true,
            bundle_path: bundle.map(Into::into),
        }
    }

    #[test]
    fn app_tiles_preview_the_frontmost_window_and_dim_what_cannot_move() {
        let s = reduce(
            &initial("Aurora"),
            &PickerEvent::Loaded {
                entries: vec![
                    entry(
                        "slack",
                        "Slack",
                        Capability::Full,
                        Some("/Applications/Slack.app"),
                    ),
                    entry("code", "Visual Studio Code", Capability::InstallOnly, None),
                    entry("steam", "Steam", Capability::Unsupported, None),
                ],
            },
        );
        let windows = [
            window(
                9,
                "slack",
                "Slack",
                "general",
                Some("/Applications/Slack.app"),
            ),
            window(
                7,
                "slack",
                "Slack",
                "random",
                Some("/Applications/Slack.app"),
            ),
            window(5, "code", "Code", "main.rs", None),
        ];
        let g = app_grid(&s, &windows);
        let tiles: Vec<&PickerTile> = g.sections.iter().flat_map(|s| s.tiles.iter()).collect();
        assert_eq!(
            tiles[0].thumbnail,
            PickerTileThumbnail::HostWindow { window_id: 9 },
            "the frontmost Slack window"
        );
        assert_eq!(tiles[0].help, "App and signed-in state");
        assert_eq!(
            tiles[1].thumbnail,
            PickerTileThumbnail::HostWindow { window_id: 5 }
        );
        let steam = tiles.iter().find(|t| t.id == "steam").unwrap();
        assert!(steam.disabled && steam.help == "No provider yet");
        assert_eq!(steam.thumbnail, PickerTileThumbnail::None);
        // Arrow keys skip the dimmed tile and clamp at the ends.
        assert_eq!(grid_step(&g, None, 1).as_deref(), Some("slack"));
        assert_eq!(grid_step(&g, Some("slack"), 1).as_deref(), Some("code"));
        assert_eq!(grid_step(&g, Some("code"), 3).as_deref(), Some("code"));
        assert_eq!(grid_step(&g, Some("code"), -4).as_deref(), Some("slack"));
    }

    #[test]
    fn window_tabs_filter_and_say_when_empty() {
        let windows = [window(9, "slack", "Slack", "general", None)];
        let g = window_grid(&windows, "", Some("9"));
        assert!(g.sections[0].tiles[0].selected);
        assert_eq!(g.sections[0].tiles[0].help, "Slack \u{b7} general");
        assert_eq!(
            window_grid(&windows, "zzz", None).empty_text.as_deref(),
            Some("No windows match.")
        );
        assert_eq!(
            window_grid(&[], "", None).empty_text.as_deref(),
            Some("No open windows.")
        );
        let screen = RemoteWindow {
            id: "s".into(),
            app_name: "cua driver".into(),
            title: String::new(),
            visible: true,
            app_id: "cua-driver".into(),
            target_epoch: 1,
            width_px: None,
            height_px: None,
            pid: None,
        };
        assert_eq!(
            remote_grid(&[screen], "", None).empty_text.as_deref(),
            Some("No open windows in this Space yet.")
        );
        assert_eq!(grid_tabs("Aurora")[2].label, "From Aurora");
    }

    #[test]
    fn primary_names_the_destination_and_needs_a_selection() {
        let windows = [window(9, "slack", "Slack", "general", None)];
        let p = grid_primary(
            PickerGridTab::Windows,
            "Aurora",
            &window_grid(&windows, "", None),
        );
        assert_eq!(p.label, "Teleport to Aurora");
        assert!(!p.enabled);
        let p = grid_primary(
            PickerGridTab::Windows,
            "Aurora",
            &window_grid(&windows, "", Some("9")),
        );
        assert!(p.enabled);
        let p = grid_primary(PickerGridTab::Space, "Aurora", &remote_grid(&[], "", None));
        assert_eq!((p.label.as_str(), p.enabled), ("Stream to This Mac", false));
        let p = grid_primary(PickerGridTab::Apps, "Aurora", &window_grid(&[], "", None));
        assert_eq!(p.label, "Continue");
    }
}
