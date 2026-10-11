// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A Space's Stream section: the Desktop row, then one row per window, each
//! one line (an icon, the label, a picture-in-picture button). Both shells
//! draw exactly these rows.
//!
//! - The Desktop row's icon is the Space's OS mark ([`crate::notch::os_icon`]):
//!   the label already says "Desktop", so a generic display glyph would say
//!   it twice, while the OS mark tells an Ubuntu desktop from a Windows one.
//!   Its label carries the resolution of the Space's primary display
//!   ("Desktop (1280×800)"), from the display list.
//! - A window row's label is the window's title (the app name only when the
//!   title is empty) and its icon is the app's own, from the SDK's
//!   `Space.app_icon`; a Space with no icon for the app shows none, never a
//!   placeholder. Shells truncate the label to one line; `help` is the full
//!   text for the tooltip.

use super::sidebar::detail_copy;
use crate::model::SpaceOs;
use crate::notch::os_icon;
use crate::teleport::windows::{
    RemoteWindow, filter_remote_windows, is_screen_target, resolution_text,
};
use serde::{Deserialize, Serialize};

/// The Desktop row's id (a window row's id is the window's handle).
pub const DESKTOP_ROW_ID: &str = "desktop";

/// A display's size in physical pixels (the primary display of the list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamDisplay {
    /// Width, px.
    pub width_px: u32,
    /// Height, px.
    pub height_px: u32,
}

/// What the Stream section is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSectionInput {
    /// The Space's windows, `None` while they load.
    #[serde(default)]
    pub windows: Option<Vec<RemoteWindow>>,
    /// The window list could not be read.
    #[serde(default)]
    pub failed: bool,
    /// The primary display, when the display list has been read.
    #[serde(default)]
    pub display: Option<StreamDisplay>,
    /// The Space's OS (the Desktop row's icon).
    pub os: SpaceOs,
    /// The OS name the Space reported ("Ubuntu 24.04"), if any.
    #[serde(default)]
    pub os_name: Option<String>,
    /// Rows whose picture in picture is open (row ids).
    #[serde(default)]
    pub open: Vec<String>,
    /// Filter text.
    #[serde(default)]
    pub query: String,
}

/// Which kind of row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamRowKind {
    /// The whole desktop.
    Desktop,
    /// One window.
    Window,
}

/// A row's icon: what the shell draws to the left of the label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StreamRowIcon {
    /// The Space's OS mark (`os-ubuntu`, `os-windows`, ...): the core's
    /// artwork (`notch.osIconSvg`) or system symbol.
    Os {
        /// The OS icon id.
        id: String,
    },
    /// The app's icon from the SDK's `Space.app_icon` (these three
    /// arguments); none when the Space has none.
    #[serde(rename_all = "camelCase")]
    App {
        /// App name.
        app_name: String,
        /// App id.
        app_id: String,
        /// Owning process, 0 when unknown.
        pid: u32,
    },
}

/// What a row button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamRowActionId {
    /// Picture in picture: the row's own stream in a floating panel.
    Pip,
}

/// A row's icon button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamRowAction {
    /// What it does.
    pub id: StreamRowActionId,
    /// SF Symbol (`pip.enter`, `pip.exit` while open).
    pub symbol: String,
    /// Tooltip and accessibility label.
    pub help: String,
    /// Its panel is open.
    pub active: bool,
}

/// One row of the Stream section: one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamRow {
    /// [`DESKTOP_ROW_ID`] or the window's handle.
    pub id: String,
    /// Desktop or window.
    pub kind: StreamRowKind,
    /// The one line ("Desktop (1280×800)", a window's title).
    pub label: String,
    /// The full label, for the tooltip when it is truncated.
    pub help: String,
    /// The display's resolution ("1280×800"), Desktop row only.
    pub resolution: Option<String>,
    /// The icon.
    pub icon: StreamRowIcon,
    /// Icon buttons, in order.
    pub actions: Vec<StreamRowAction>,
}

/// The Stream section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSection {
    /// The Desktop row, then one per window.
    pub rows: Vec<StreamRow>,
    /// Under the rows while there are no window rows: loading, none,
    /// failed or no match.
    pub status_text: Option<String>,
}

/// "Desktop" or "Desktop (1280×800)".
pub fn desktop_label(resolution: Option<&str>) -> String {
    match resolution {
        Some(r) => format!("Desktop ({r})"),
        None => "Desktop".into(),
    }
}

/// A window row's label: its title, else its app's name.
pub fn window_label(w: &RemoteWindow) -> String {
    [w.title.trim(), w.app_name.trim(), w.app_id.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn pip(open: bool) -> StreamRowAction {
    StreamRowAction {
        id: StreamRowActionId::Pip,
        symbol: if open { "pip.exit" } else { "pip.enter" }.into(),
        help: if open {
            "Close picture in picture"
        } else {
            "Picture in picture"
        }
        .into(),
        active: open,
    }
}

/// The Stream section for `input`.
pub fn stream_section(input: &StreamSectionInput) -> StreamSection {
    let is_open = |id: &str| input.open.iter().any(|o| o == id);
    let resolution = input
        .display
        .and_then(|d| resolution_text(d.width_px, d.height_px));
    let label = desktop_label(resolution.as_deref());
    let mut rows = vec![StreamRow {
        id: DESKTOP_ROW_ID.into(),
        kind: StreamRowKind::Desktop,
        help: label.clone(),
        label,
        resolution,
        icon: StreamRowIcon::Os {
            id: os_icon(input.os, input.os_name.as_deref()).into(),
        },
        actions: vec![pip(is_open(DESKTOP_ROW_ID))],
    }];
    let copy = detail_copy();
    let status_text = match &input.windows {
        None => Some(copy.stream_loading),
        Some(all) => {
            // The guest's own capture target is the whole screen: the
            // Desktop row already stands for it.
            let windows: Vec<RemoteWindow> = all
                .iter()
                .filter(|w| !is_screen_target(w))
                .cloned()
                .collect();
            let shown = filter_remote_windows(&windows, &input.query);
            rows.extend(shown.iter().map(|w| {
                let label = window_label(w);
                StreamRow {
                    id: w.id.clone(),
                    kind: StreamRowKind::Window,
                    help: label.clone(),
                    label,
                    resolution: None,
                    icon: StreamRowIcon::App {
                        app_name: w.app_name.clone(),
                        app_id: w.app_id.clone(),
                        pid: w.pid.unwrap_or(0),
                    },
                    actions: vec![pip(is_open(&w.id))],
                }
            }));
            if !shown.is_empty() {
                None
            } else if !windows.is_empty() {
                Some(copy.stream_no_match)
            } else if input.failed {
                Some(copy.stream_failed)
            } else {
                Some(copy.stream_empty)
            }
        }
    };
    StreamSection { rows, status_text }
}

/// A change to the picture-in-picture panels a shell has open. The panels
/// are native windows, so the shell reports what happened to them (opened
/// from a row, closed from its own title bar, or the full list when it
/// re-reads them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PipEvent {
    /// A row's panel opened.
    Opened {
        /// The row id.
        row: String,
    },
    /// A row's panel closed.
    Closed {
        /// The row id.
        row: String,
    },
    /// The panels open now, as the shell listed them.
    Synced {
        /// Row ids.
        rows: Vec<String>,
    },
}

/// What a row's picture-in-picture button does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PipCommand {
    /// Open the row's panel.
    Open {
        /// The row id.
        row: String,
    },
    /// Close the row's open panel.
    Close {
        /// The row id.
        row: String,
    },
}

/// The open panels (row ids, in the order they opened) after `event`: the
/// Stream section's `open`.
pub fn pip_reduce(open: &[String], event: &PipEvent) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(open.len() + 1);
    let mut push = |row: &str| {
        if !row.is_empty() && !out.iter().any(|o| o == row) {
            out.push(row.to_string());
        }
    };
    match event {
        PipEvent::Opened { row } => {
            open.iter().for_each(|o| push(o));
            push(row);
        }
        PipEvent::Closed { row } => open.iter().filter(|o| *o != row).for_each(|o| push(o)),
        // Keep the known order for panels still open; new ones go last.
        PipEvent::Synced { rows } => {
            open.iter()
                .filter(|o| rows.contains(o))
                .for_each(|o| push(o));
            rows.iter().for_each(|o| push(o));
        }
    }
    out
}

/// What clicking `row`'s picture-in-picture button does: close its panel
/// while open (the button shows `pip.exit`), else open one.
pub fn pip_click(open: &[String], row: &str) -> PipCommand {
    let row = row.to_string();
    if open.contains(&row) {
        PipCommand::Close { row }
    } else {
        PipCommand::Open { row }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pip_panels_track_opens_closes_and_resyncs() {
        let open = pip_reduce(
            &[],
            &PipEvent::Opened {
                row: DESKTOP_ROW_ID.into(),
            },
        );
        let open = pip_reduce(&open, &PipEvent::Opened { row: "w1".into() });
        let open = pip_reduce(&open, &PipEvent::Opened { row: "w1".into() });
        assert_eq!(open, ["desktop", "w1"]);
        assert_eq!(
            pip_click(&open, "w1"),
            PipCommand::Close { row: "w1".into() }
        );
        assert_eq!(
            pip_click(&open, "w2"),
            PipCommand::Open { row: "w2".into() }
        );
        let mut i = input(Some(vec![win("w1", "Firefox", "Mozilla Firefox")]));
        i.open = open.clone();
        let s = stream_section(&i);
        assert_eq!(s.rows[0].actions[0].symbol, "pip.exit");
        assert_eq!(s.rows[1].actions[0].help, "Close picture in picture");
        // Closed from the panel's own title bar.
        let open = pip_reduce(&open, &PipEvent::Closed { row: "w1".into() });
        assert_eq!(open, ["desktop"]);
        let open = pip_reduce(
            &open,
            &PipEvent::Synced {
                rows: vec!["w9".into(), "desktop".into()],
            },
        );
        assert_eq!(open, ["desktop", "w9"]);
        assert!(pip_reduce(&open, &PipEvent::Synced { rows: vec![] }).is_empty());
    }

    fn win(id: &str, app: &str, title: &str) -> RemoteWindow {
        RemoteWindow {
            id: id.into(),
            app_name: app.into(),
            title: title.into(),
            visible: true,
            app_id: app.to_lowercase(),
            target_epoch: 1,
            width_px: None,
            height_px: None,
            pid: Some(42),
        }
    }

    fn input(windows: Option<Vec<RemoteWindow>>) -> StreamSectionInput {
        StreamSectionInput {
            windows,
            failed: false,
            display: Some(StreamDisplay {
                width_px: 1280,
                height_px: 800,
            }),
            os: SpaceOs::Linux,
            os_name: Some("Ubuntu 24.04".into()),
            open: vec![],
            query: String::new(),
        }
    }

    #[test]
    fn desktop_row_carries_the_resolution_and_the_os_mark() {
        let s = stream_section(&input(Some(vec![])));
        let d = &s.rows[0];
        assert_eq!(d.label, "Desktop (1280\u{d7}800)");
        assert_eq!(d.resolution.as_deref(), Some("1280\u{d7}800"));
        assert_eq!(
            d.icon,
            StreamRowIcon::Os {
                id: "os-ubuntu".into()
            }
        );
        assert_eq!(d.actions[0].symbol, "pip.enter");
        assert_eq!(d.actions[0].help, "Picture in picture");
        assert_eq!(
            s.status_text.as_deref(),
            Some("No open windows in this Space yet.")
        );
    }

    #[test]
    fn window_rows_use_the_title_and_fall_back_to_the_app() {
        let mut i = input(Some(vec![
            win("w1", "Firefox", "Mozilla Firefox"),
            win("w2", "xterm", "  "),
            win("w0", "cua driver", "Screen"),
        ]));
        i.open = vec!["w2".into()];
        let s = stream_section(&i);
        let labels: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Desktop (1280\u{d7}800)", "Mozilla Firefox", "xterm"]
        );
        assert_eq!(
            s.rows[1].icon,
            StreamRowIcon::App {
                app_name: "Firefox".into(),
                app_id: "firefox".into(),
                pid: 42
            }
        );
        assert!(s.rows[2].actions[0].active);
        assert_eq!(s.rows[2].actions[0].symbol, "pip.exit");
        assert_eq!(s.status_text, None);
    }

    #[test]
    fn no_display_no_resolution() {
        let mut i = input(None);
        i.display = None;
        let s = stream_section(&i);
        assert_eq!(s.rows[0].label, "Desktop");
        assert_eq!(s.rows[0].resolution, None);
        assert_eq!(
            s.status_text.as_deref(),
            Some("Looking for this Space\u{2019}s windows\u{2026}")
        );
    }
}
