// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The notch panel: the only overlay. It shows Space tiles and the Teleport
//! prompt, nothing else.
//!
//! - [`geometry`]: the Tauri portal window's modes and frames;
//! - [`layout`]: the notch rectangle from a screen's facts (safe area,
//!   auxiliary top areas), with the no-notch fallback, and the open size;
//! - [`reduce`]: hover, click and drag state, with timers returned as effects
//!   so every shell expands and collapses at the same moments;
//! - [`tiles`]: which Spaces the panel shows, filtered by the header's
//!   search ([`matches`]), each with its OS icon ([`os_icon`]);
//! - [`tab`]: the "N Spaces" tab (count over word);
//! - [`activity`]: the indicator left of the closed notch (a transfer, the
//!   network hotspot, a Space starting), in that priority.
//!
//! Motion constants ([`MOTION`]) are shared so both shells animate alike:
//! the shape springs open from the hardware notch first, then the content
//! fades and scales in after [`NotchMotion::content_delay_ms`]; on close the
//! content fades out first and the shape follows, critically damped so it
//! never rises above the notch.
//! No copyleft source was used.

pub mod drag_trigger;
pub mod geometry;

pub use geometry::{DisplayStyle, DisplayStyleSource, LogicalRect, Size, WindowMode};

use crate::host::THIS_MACHINE_ID;
use crate::model::{Space, SpaceOs, SpaceStatus};
use crate::spaces::sort_by_mru;
use crate::teleport::drag::{
    self, DragOverlayEffect, DragOverlayEvent, DragOverlayPhase, DragOverlayState,
};
use serde::{Deserialize, Serialize};

/// Springs and delays both shells use.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchMotion {
    /// Hover dwell before the panel opens.
    pub hover_dwell_ms: u32,
    /// Delay before closing after the pointer leaves.
    pub close_delay_ms: u32,
    /// Opening spring response (s).
    pub open_response: f64,
    /// Opening spring damping fraction.
    pub open_damping: f64,
    /// Closing spring response (s).
    pub close_response: f64,
    /// Closing spring damping fraction.
    pub close_damping: f64,
    /// Reduce Motion: opacity-only duration (s).
    pub reduced_duration: f64,
    /// Hover acknowledgement spring response (s): the closed notch grows a
    /// little while the dwell runs.
    pub hover_response: f64,
    /// Hover acknowledgement damping fraction.
    pub hover_damping: f64,
    /// Horizontal scale of the closed notch (and its tab) under the pointer.
    pub hover_scale: f64,
    /// Vertical scale of the closed notch under the pointer (a few points
    /// taller, anchored at the top).
    pub hover_scale_y: f64,
    /// The content starts fading in this long after the shape starts
    /// opening (ms).
    pub content_delay_ms: u32,
    /// Content fade-in duration (s).
    pub content_in: f64,
    /// Content fade-out duration on close (s); the shape waits for it.
    pub content_out: f64,
    /// Content scale at the start of its fade-in (anchored at the top).
    pub content_scale: f64,
}

/// The shared motion.
pub const MOTION: NotchMotion = NotchMotion {
    hover_dwell_ms: 300,
    close_delay_ms: 400,
    open_response: 0.42,
    open_damping: 0.8,
    close_response: 0.4,
    close_damping: 1.0,
    reduced_duration: 0.15,
    hover_response: 0.3,
    hover_damping: 0.65,
    hover_scale: 1.08,
    hover_scale_y: 1.12,
    content_delay_ms: 90,
    content_in: 0.22,
    content_out: 0.1,
    content_scale: 0.92,
};

/// Corner radii of the notch shape, closed and open.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchRadii {
    /// Top (concave ear) radius.
    pub top: f64,
    /// Bottom radius.
    pub bottom: f64,
}

/// Closed notch radii.
pub const CLOSED_RADII: NotchRadii = NotchRadii {
    top: 6.0,
    bottom: 14.0,
};
/// Open panel radii.
pub const OPEN_RADII: NotchRadii = NotchRadii {
    top: 19.0,
    bottom: 24.0,
};
/// Content padding inside the open panel.
pub const CONTENT_PADDING: f64 = 15.0;
/// A tile's thumbnail width (16:10).
pub const TILE_WIDTH: f64 = 128.0;
/// A tile's thumbnail height.
pub const TILE_THUMB_HEIGHT: f64 = 80.0;
/// Space between tiles.
pub const TILE_GAP: f64 = 12.0;
/// A tile's header line: the OS logo and where the Space runs.
pub const TILE_HEADER_HEIGHT: f64 = 14.0;
/// Tile row height: the header line, a 5 pt gap, the thumbnail, a 6 pt gap
/// and the one-line caption.
pub const TILE_ROW_HEIGHT: f64 = TILE_HEADER_HEIGHT + 5.0 + TILE_THUMB_HEIGHT + 6.0 + 16.0;
/// Teleport prompt (or permission line) height.
pub const PROMPT_HEIGHT: f64 = 36.0;
/// How much taller than the notch the "Teleport to Cua" box grows.
pub const PROMPT_BOX_EXTRA: f64 = 28.0;
/// Width of the "N Spaces" tab hanging off the notch's right edge: the
/// most it takes (the shells size it to its content, see [`TAB_INSETS`]).
pub const TAB_WIDTH: f64 = 72.0;
/// Space between the closed notch's side content (the "N Spaces" rows, the
/// activity ring) and its tab's edges, in points: `(toward the notch, the
/// outer side)`. None toward the notch and 6 pt outside, so the tabs
/// barely widen the notch but the content does not touch the outer edge.
pub const TAB_INSETS: (f64, f64) = (0.0, 6.0);
/// Room around the open panel for its shadow and the spring's overshoot.
pub const SHADOW_PADDING: f64 = 24.0;
/// Most tiles the panel shows.
pub const MAX_TILES: usize = 6;

/// What the shell knows about one screen, in points, AppKit orientation
/// (origin bottom left).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenFacts {
    /// `NSScreen.frame`.
    pub frame: LogicalRect,
    /// `NSScreen.visibleFrame`.
    pub visible_frame: LogicalRect,
    /// `safeAreaInsets.top` (0 without a notch).
    pub safe_area_top: f64,
    /// `auxiliaryTopLeftArea` width, when present.
    pub aux_left_width: Option<f64>,
    /// `auxiliaryTopRightArea` width, when present.
    pub aux_right_width: Option<f64>,
}

/// Where the notch is and how big the panel gets.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchLayout {
    /// A real camera housing.
    pub has_notch: bool,
    /// The notch (or the virtual one), AppKit coordinates.
    pub notch: LogicalRect,
    /// The closed panel's hit area: the notch grown for hover and drags.
    pub closed_frame: LogicalRect,
    /// The open panel frame (tiles and prompt), top-centred.
    pub open_frame: LogicalRect,
    /// The "Teleport to Cua" box alone, hanging below the notch.
    pub prompt_frame: LogicalRect,
    /// The "N Spaces" tab beside the closed notch (right of it), at its
    /// widest.
    pub tab_frame: LogicalRect,
    /// Space between a closed tab's content (the "N Spaces" rows, the
    /// activity ring on the left) and the tab's edge toward the notch.
    pub tab_inset_notch: f64,
    /// Space between a closed tab's content and its outer edge (before the
    /// outer ear).
    pub tab_inset_outer: f64,
    /// The overlay window: every frame above plus [`SHADOW_PADDING`], so
    /// the shape animates inside one window that never resizes mid-spring.
    /// Its transparent pixels pass clicks through.
    pub stage_frame: LogicalRect,
    /// Draw the black notch shape (true) or a floating glass capsule.
    pub notch_style: bool,
}

/// The layout for a screen. `prompt` adds the Teleport prompt row.
pub fn layout(screen: &ScreenFacts, prompt: bool) -> NotchLayout {
    let f = screen.frame;
    let top = f.y + f.height;
    let has_notch = screen.aux_left_width.is_some()
        && screen.aux_right_width.is_some()
        && screen.safe_area_top > 0.0;
    let (w, h) = if has_notch {
        (
            f.width - screen.aux_left_width.unwrap_or(0.0) - screen.aux_right_width.unwrap_or(0.0)
                + 4.0,
            screen.safe_area_top,
        )
    } else {
        let menu = top - (screen.visible_frame.y + screen.visible_frame.height);
        (
            (f.width * 0.14).clamp(160.0, 240.0),
            if menu > 0.0 { menu } else { 25.0 },
        )
    };
    let mid = f.x + f.width / 2.0;
    let round = |r: LogicalRect| {
        LogicalRect::new(r.x.round(), r.y.round(), r.width.round(), r.height.round())
    };
    let notch = round(LogicalRect::new(mid - w / 2.0, top - h, w, h));
    let closed = round(LogicalRect::new(
        notch.x - 16.0,
        notch.y - 8.0,
        notch.width + 32.0,
        notch.height + 8.0,
    ));
    let open_w = (f.width * 0.45)
        .min(640.0)
        .max(notch.width + 2.0 * OPEN_RADII.top);
    let open_h = h
        + CONTENT_PADDING
        + TILE_ROW_HEIGHT
        + if prompt { PROMPT_HEIGHT } else { 0.0 }
        + CONTENT_PADDING;
    let open = round(LogicalRect::new(
        mid - open_w / 2.0,
        top - open_h,
        open_w,
        open_h,
    ));
    // The notch itself grows a little taller around one compact line.
    let prompt_w = (w + 2.0 * OPEN_RADII.top).min(open_w);
    let prompt_h = h + PROMPT_BOX_EXTRA;
    let prompt_frame = round(LogicalRect::new(
        mid - prompt_w / 2.0,
        top - prompt_h,
        prompt_w,
        prompt_h,
    ));
    let tab = round(LogicalRect::new(
        notch.x + notch.width,
        notch.y,
        TAB_WIDTH,
        notch.height,
    ));
    // Symmetric about the notch so the shape stays centred in the window.
    let half = [
        open.width,
        prompt_frame.width,
        closed.width,
        notch.width + 2.0 * TAB_WIDTH,
    ]
    .into_iter()
    .fold(0.0_f64, f64::max)
        / 2.0
        + SHADOW_PADDING;
    let low = open.height.max(prompt_frame.height) + SHADOW_PADDING;
    let stage = round(LogicalRect::new(mid - half, top - low, 2.0 * half, low));
    NotchLayout {
        has_notch,
        notch,
        closed_frame: closed,
        open_frame: open,
        prompt_frame,
        tab_frame: tab,
        tab_inset_notch: TAB_INSETS.0,
        tab_inset_outer: TAB_INSETS.1,
        stage_frame: stage,
        notch_style: has_notch,
    }
}

/// One tile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchTile {
    /// Space id.
    pub id: String,
    /// Name.
    pub name: String,
    /// OS.
    pub os: SpaceOs,
    /// Status.
    pub status: SpaceStatus,
    /// Drawn at reduced opacity.
    pub dim: bool,
    /// Accepts a teleport drop (reachable).
    pub drop_target: bool,
    /// Under the dragged window.
    pub targeted: bool,
    /// The Space's OS icon id ([`os_icon`]: `os-macos`, `os-windows`,
    /// `os-ubuntu`, ..., `os-linux`); the artwork is [`os_icon_svg`], or on
    /// macOS the system symbol [`os_icon_system_symbol`] names.
    pub symbol: String,
    /// Accessibility label: name, OS, where it runs and status.
    pub label: String,
    /// Where it runs, in words, on the tile's header line next to the OS
    /// logo: "This Mac", the machine that provides it ("Mac mini"), the
    /// address of one added by address, or the cloud's place ([`location`]).
    #[serde(default)]
    pub location: String,
    /// While it is being created: overall progress in thousandths (a ring
    /// over the tile).
    pub progress: Option<u32>,
    /// While it is being created: the phase in words ("Starting…"), or
    /// "Failed"; while it is being deleted, "Deleting…".
    pub progress_label: Option<String>,
    /// Signed in through the Keyvault (and not dismissed): the key badge.
    #[serde(default)]
    pub signed_in: bool,
}

/// Linux distributions with their own icon: a word in the reported OS
/// name, and the icon id. Checked in order (Omarchy before Arch).
const DISTROS: &[(&str, &str)] = &[
    ("omarchy", "os-omarchy"),
    ("ubuntu", "os-ubuntu"),
    ("manjaro", "os-manjaro"),
    ("arch", "os-arch"),
    ("alpine", "os-alpine"),
    ("mint", "os-mint"),
    ("kali", "os-kali"),
    ("opensuse", "os-opensuse"),
    ("suse", "os-opensuse"),
    ("centos", "os-centos"),
    ("red hat", "os-redhat"),
    ("rhel", "os-redhat"),
    ("pop!_os", "os-popos"),
    ("pop_os", "os-popos"),
    ("pop os", "os-popos"),
    ("elementary", "os-elementary"),
];

/// Every OS icon id.
pub const OS_ICONS: &[&str] = &[
    "os-macos",
    "os-windows",
    "os-linux",
    "os-omarchy",
    "os-ubuntu",
    "os-manjaro",
    "os-arch",
    "os-alpine",
    "os-mint",
    "os-kali",
    "os-opensuse",
    "os-centos",
    "os-redhat",
    "os-popos",
    "os-elementary",
];

/// The OS icon for a Space: the Linux distribution's own mark when the
/// Space reported one we ship (`os_name`, for example "Ubuntu"), else the
/// family's (`os-macos`, `os-windows`, `os-linux`).
pub fn os_icon(os: SpaceOs, os_name: Option<&str>) -> &'static str {
    match os {
        SpaceOs::Macos => "os-macos",
        SpaceOs::Windows => "os-windows",
        SpaceOs::Linux => {
            let name = os_name.unwrap_or_default().to_lowercase();
            DISTROS
                .iter()
                .find(|(word, _)| name.contains(word))
                .map_or("os-linux", |(_, id)| id)
        }
    }
}

/// The icon's artwork (a single-color 24 x 24 SVG; the shells tint it).
pub fn os_icon_svg(id: &str) -> Option<&'static str> {
    Some(match id {
        "os-macos" => include_str!("../../assets/os-icons/os-macos.svg"),
        "os-windows" => include_str!("../../assets/os-icons/os-windows.svg"),
        "os-linux" => include_str!("../../assets/os-icons/os-linux.svg"),
        "os-omarchy" => include_str!("../../assets/os-icons/os-omarchy.svg"),
        "os-ubuntu" => include_str!("../../assets/os-icons/os-ubuntu.svg"),
        "os-manjaro" => include_str!("../../assets/os-icons/os-manjaro.svg"),
        "os-arch" => include_str!("../../assets/os-icons/os-arch.svg"),
        "os-alpine" => include_str!("../../assets/os-icons/os-alpine.svg"),
        "os-mint" => include_str!("../../assets/os-icons/os-mint.svg"),
        "os-kali" => include_str!("../../assets/os-icons/os-kali.svg"),
        "os-opensuse" => include_str!("../../assets/os-icons/os-opensuse.svg"),
        "os-centos" => include_str!("../../assets/os-icons/os-centos.svg"),
        "os-redhat" => include_str!("../../assets/os-icons/os-redhat.svg"),
        "os-popos" => include_str!("../../assets/os-icons/os-popos.svg"),
        "os-elementary" => include_str!("../../assets/os-icons/os-elementary.svg"),
        _ => return None,
    })
}

/// The system symbol to draw instead of the SVG on macOS (`apple.logo`
/// for macOS Spaces), if any.
pub fn os_icon_system_symbol(id: &str) -> Option<&'static str> {
    (id == "os-macos").then_some("apple.logo")
}

/// Where a Space runs, in words, for its tile: "This Mac" for one on this
/// machine; for one your other machine provides, that machine's name (the
/// relay's device name, else the machine's own row in `spaces`, else its
/// relay id); for one added by address, its name for the machine, else the
/// address; for one in your cloud, its place ("AWS \u{b7} us-west-2").
pub fn location(space: &Space, spaces: &[Space]) -> String {
    use crate::model::SpaceProvider;
    let text = |v: &Option<String>| v.clone().filter(|v| !v.trim().is_empty());
    if space.id == THIS_MACHINE_ID {
        return "This Mac".into();
    }
    match space.provider.unwrap_or(SpaceProvider::Cloud) {
        SpaceProvider::Local => "This Mac".into(),
        SpaceProvider::Cloud => crate::spaces::sidebar::location_text(space).into(),
        SpaceProvider::Direct => text(&space.host_name).unwrap_or_else(|| {
            space
                .id
                .strip_prefix("space://direct/")
                .or_else(|| space.id.strip_prefix("direct:"))
                .unwrap_or(&space.id)
                .to_string()
        }),
        SpaceProvider::Relay => {
            if let Some(place) =
                text(&space.cloud_place).filter(|_| crate::spaces::sidebar::in_your_cloud(space))
            {
                return place;
            }
            match text(&space.host) {
                Some(host) => text(&space.host_name)
                    .or_else(|| {
                        spaces
                            .iter()
                            .find(|m| {
                                m.id.strip_prefix("relay:") == Some(host.as_str())
                                    && text(&m.host).is_none()
                            })
                            .map(|m| m.name.clone())
                    })
                    .unwrap_or(host),
                // The machine itself.
                None => text(&space.host_name).unwrap_or_else(|| space.name.clone()),
            }
        }
    }
}

/// Whether `space` matches the header's search: every word of `query`
/// (case-insensitive) is in its name, OS, OS name, status, detail or
/// group. An empty query matches everything.
pub fn matches(space: &Space, query: &str) -> bool {
    let hay = [
        space.name.as_str(),
        space.os.as_str(),
        space.os.label(),
        space.os_name.as_deref().unwrap_or_default(),
        space.status.label(),
        &format!("{:?}", space.status),
        space.detail.as_str(),
        space.fleet_id.as_deref().unwrap_or_default(),
        crate::spaces::sidebar::location_text(space),
        space.host_name.as_deref().unwrap_or_default(),
    ]
    .join("\n")
    .to_lowercase();
    query
        .split_whitespace()
        .all(|w| hay.contains(&w.to_lowercase()))
}

/// `spaces` narrowed to the ones matching `query` (order kept).
pub fn filter(spaces: &[Space], query: &str) -> Vec<Space> {
    spaces
        .iter()
        .filter(|s| matches(s, query))
        .cloned()
        .collect()
}

/// The Space tiles: MRU order, not this machine, at most [`MAX_TILES`].
pub fn tiles(spaces: &[Space], targeted: Option<&str>) -> Vec<NotchTile> {
    tiles_matching(spaces, targeted, "")
}

/// [`tiles`] narrowed by the header's search.
pub fn tiles_matching(spaces: &[Space], targeted: Option<&str>, query: &str) -> Vec<NotchTile> {
    sort_by_mru(spaces)
        .into_iter()
        .filter(|s| s.id != THIS_MACHINE_ID && matches(s, query))
        .take(MAX_TILES)
        .map(|s| NotchTile {
            location: location(&s, spaces),
            dim: !s.status.is_live(),
            drop_target: accepts_drop(&s),
            targeted: targeted == Some(s.id.as_str()),
            symbol: os_icon(s.os, s.os_name.as_deref()).into(),
            label: format!(
                "{}, {}, {}, {}",
                s.name,
                s.os.label(),
                location(&s, spaces),
                crate::spaces::sidebar::status_text(&s)
            ),
            progress: s
                .progress
                .as_ref()
                .filter(|p| p.error.is_none())
                .map(|p| p.permille),
            progress_label: if s.status == SpaceStatus::Deleting {
                Some(s.status.label().into())
            } else {
                s.progress.as_ref().map(|p| p.label.clone())
            },
            signed_in: false,
            id: s.id,
            name: s.name,
            os: s.os,
            status: s.status,
        })
        .collect()
}

/// What the panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotchPhase {
    /// Just the notch.
    Closed,
    /// The Space tiles.
    Tiles,
    /// The Teleport prompt ("Teleport to Cua").
    Prompt,
}

/// The panel's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchState {
    /// Opened by hover or click.
    pub open: bool,
    /// The pointer is over the panel.
    pub hovering: bool,
    /// A file or app drag (pasteboard) is over the panel.
    pub drop_targeted: bool,
    /// The window drag state machine.
    pub drag: DragOverlayState,
    /// Window-drag detection needs a permission this process lacks (macOS
    /// Accessibility): the open panel says so instead of failing silently.
    pub drag_permission_missing: bool,
    /// The header's search text (cleared when the panel closes).
    #[serde(default)]
    pub query: String,
    /// This machine shares its network to a Space (the hotspot).
    #[serde(default)]
    pub hotspot: bool,
    /// A teleport or file transfer is in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<NotchTransfer>,
    /// Keyvault sign-ins are live in a Space right now (the core's
    /// [`crate::keyvault::view::sharing_label`]), so access is never silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyvault: Option<String>,
    /// The Spaces signed in through the Keyvault, less the copies the user
    /// dismissed ([`crate::keyvault::view::signed_in_spaces`]): their tiles
    /// carry the key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signed_in: Vec<String>,
    /// "Spaces tab in the notch: Hide" (menu bar only): nothing shows in
    /// the notch, and hover, clicks and window drags do nothing.
    #[serde(default)]
    pub hidden: bool,
}

/// A transfer in flight: bytes sent of the total, when known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchTransfer {
    /// Bytes sent so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<u64>,
    /// Total bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl Default for NotchState {
    fn default() -> Self {
        Self {
            open: false,
            hovering: false,
            drop_targeted: false,
            drag: drag::initial(),
            drag_permission_missing: false,
            query: String::new(),
            hotspot: false,
            transfer: None,
            keyvault: None,
            signed_in: vec![],
            hidden: false,
        }
    }
}

/// An input to the panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum NotchEvent {
    /// Pointer entered.
    HoverEnter,
    /// Pointer left.
    HoverExit,
    /// The dwell timer the core asked for fired.
    DwellElapsed,
    /// The close timer the core asked for fired.
    CloseElapsed,
    /// Click on the closed notch.
    Click,
    /// Esc or a click outside.
    Dismiss,
    /// A pasteboard drag entered (true) or left (false) the panel.
    DropTargeted {
        /// Over the panel.
        targeted: bool,
    },
    /// A window drag event.
    Drag {
        /// The event.
        event: DragOverlayEvent,
    },
    /// The shell checked the window-drag permission.
    DragPermission {
        /// Granted (or not needed on this OS).
        granted: bool,
    },
    /// The header's search text changed.
    Search {
        /// The text.
        query: String,
    },
    /// Esc: clears the search first, then closes.
    Escape,
    /// The notch setting changed: shown, or hidden (menu bar only).
    Visibility {
        /// Shown.
        shown: bool,
    },
    /// The hotspot or a transfer started, progressed or ended.
    Activity {
        /// The hotspot is on.
        hotspot: bool,
        /// The transfer in flight, if any.
        #[serde(default)]
        transfer: Option<NotchTransfer>,
    },
    /// Keyvault sharing started, changed or stopped (none: nothing live).
    Keyvault {
        /// The sharing label, if any.
        #[serde(default)]
        label: Option<String>,
        /// The Space ids signed in (their tiles carry the key).
        #[serde(default, rename = "signedIn")]
        signed_in: Vec<String>,
    },
}

/// Something the shell must do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NotchEffect {
    /// Start (or restart) the dwell timer.
    StartDwell {
        /// Milliseconds.
        ms: u32,
    },
    /// Start the close timer.
    StartClose {
        /// Milliseconds.
        ms: u32,
    },
    /// Cancel pending timers.
    CancelTimers,
    /// A window drag effect (capture the ghost, commit the teleport).
    Drag {
        /// The effect.
        effect: DragOverlayEffect,
    },
}

/// A transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotchTransition {
    /// New state.
    pub state: NotchState,
    /// Effects to run.
    pub effects: Vec<NotchEffect>,
}

/// Advances the panel.
pub fn reduce(state: &NotchState, event: &NotchEvent) -> NotchTransition {
    let mut s = state.clone();
    let mut effects = Vec::new();
    if s.hidden
        && matches!(
            event,
            NotchEvent::HoverEnter
                | NotchEvent::DwellElapsed
                | NotchEvent::Click
                | NotchEvent::DropTargeted { .. }
                | NotchEvent::Drag { .. }
        )
    {
        // Hidden: the notch takes nothing.
        return NotchTransition { state: s, effects };
    }
    match event {
        NotchEvent::HoverEnter => {
            s.hovering = true;
            effects.push(NotchEffect::CancelTimers);
            if !s.open {
                effects.push(NotchEffect::StartDwell {
                    ms: MOTION.hover_dwell_ms,
                });
            }
        }
        NotchEvent::HoverExit => {
            s.hovering = false;
            effects.push(NotchEffect::CancelTimers);
            if s.open {
                effects.push(NotchEffect::StartClose {
                    ms: MOTION.close_delay_ms,
                });
            }
        }
        NotchEvent::DwellElapsed => {
            if s.hovering {
                s.open = true;
            }
        }
        NotchEvent::CloseElapsed => {
            if !s.hovering && !s.drop_targeted && s.drag.phase == DragOverlayPhase::Idle {
                s.open = false;
            }
        }
        NotchEvent::Click => {
            s.open = !s.open;
            effects.push(NotchEffect::CancelTimers);
        }
        NotchEvent::Dismiss => {
            s.open = false;
            effects.push(NotchEffect::CancelTimers);
        }
        NotchEvent::DropTargeted { targeted } => {
            // A drag opens immediately: no dwell.
            s.drop_targeted = *targeted;
            if *targeted {
                s.open = true;
                effects.push(NotchEffect::CancelTimers);
            } else if !s.hovering {
                effects.push(NotchEffect::StartClose {
                    ms: MOTION.close_delay_ms,
                });
            }
        }
        NotchEvent::Drag { event } => {
            let t = drag::apply(&s.drag, event);
            let ended = matches!(
                event,
                DragOverlayEvent::Drop { .. } | DragOverlayEvent::Cancel
            );
            s.drag = t.state;
            effects.extend(
                t.effects
                    .into_iter()
                    .map(|effect| NotchEffect::Drag { effect }),
            );
            if ended {
                s.open = false;
            }
        }
        NotchEvent::DragPermission { granted } => {
            s.drag_permission_missing = !*granted;
        }
        NotchEvent::Search { query } => {
            s.query = query.clone();
        }
        NotchEvent::Escape => {
            if s.query.is_empty() {
                s.open = false;
                effects.push(NotchEffect::CancelTimers);
            } else {
                s.query.clear();
            }
        }
        NotchEvent::Visibility { shown } => {
            s.hidden = !*shown;
            if s.hidden {
                s.open = false;
                s.hovering = false;
                s.drop_targeted = false;
                s.drag = drag::initial();
                effects.push(NotchEffect::CancelTimers);
            }
        }
        NotchEvent::Activity { hotspot, transfer } => {
            s.hotspot = *hotspot;
            s.transfer = transfer.clone();
        }
        NotchEvent::Keyvault { label, signed_in } => {
            s.keyvault = label.clone();
            s.signed_in = signed_in.clone();
        }
    }
    // A closed panel forgets its search.
    if !s.open && !s.drop_targeted && s.drag.phase == DragOverlayPhase::Idle {
        s.query.clear();
    }
    NotchTransition { state: s, effects }
}

/// The panel as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchView {
    /// What shows.
    pub phase: NotchPhase,
    /// The tiles (in `tiles`).
    pub tiles: Vec<NotchTile>,
    /// Tiles accept drops.
    pub drop_mode: bool,
    /// The prompt line.
    pub prompt: Option<String>,
    /// Accessibility label of the panel.
    pub label: String,
    /// "N Spaces": the tab's spoken label (remote Spaces only).
    pub count_label: String,
    /// The tab's two rows: the count over the word.
    pub tab: NotchTab,
    /// The row flanking the notch in the open panel: the search left of
    /// it, the buttons right of it. None while dropping.
    pub header: Option<NotchHeader>,
    /// The line in place of the tiles when the search matches nothing.
    pub empty: Option<String>,
    /// The indicator left of the closed notch.
    pub activity: Option<NotchActivity>,
    /// The notch is hidden (menu bar only): draw nothing.
    pub hidden: bool,
    /// The tab shows (closed, no drag in flight; a drag tucks it away).
    pub show_tab: bool,
    /// The pointer rests on the closed notch while the dwell runs: the
    /// shells grow it by [`NotchMotion::hover_scale`].
    pub hover_cue: bool,
    /// One line in the open panel when window drags cannot be detected.
    pub permission: Option<NotchPermission>,
    /// One line in the open panel while Keyvault sign-ins are live in a
    /// Space (and not dismissed): what is live, and Dismiss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NotchAccess>,
}

/// The live-access line and its button. Dismiss hides the indicator and
/// the tiles' key; it revokes and wipes nothing (the Keyvault's Access page
/// does that).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchAccess {
    /// "Keyvault sign-ins live in dev-1" (opens the Access page).
    pub text: String,
    /// "Dismiss".
    pub dismiss: String,
}

/// The missing-permission line and its button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchPermission {
    /// The line.
    pub text: String,
    /// The button.
    pub action: String,
    /// Which settings pane the button opens (`accessibility`).
    pub pane: String,
}

/// The "N Spaces" tab: two rows, the count over the word. The shells set
/// the count at 11 pt bold (tabular digits) and the word at 8 pt semibold,
/// uppercase, 0.03 em tracking, 70% white.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchTab {
    /// "9".
    pub count: String,
    /// "Spaces" (or "Space").
    pub word: String,
}

/// The tab for `spaces`: remote Spaces only (not this machine).
pub fn tab(spaces: &[Space]) -> NotchTab {
    let n = remote_count(spaces);
    NotchTab {
        count: n.to_string(),
        word: if n == 1 { "Space" } else { "Spaces" }.into(),
    }
}

fn remote_count(spaces: &[Space]) -> usize {
    crate::spaces::openable_count(spaces) as usize
}

/// A button in the open panel's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotchButtonId {
    /// Opens the main window (the Spaces list).
    List,
    /// Opens Settings.
    Settings,
}

/// One header button: a system symbol, its spoken label and tooltip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchButton {
    /// What it does.
    pub id: NotchButtonId,
    /// SF Symbol.
    pub symbol: String,
    /// Accessibility label.
    pub label: String,
    /// Tooltip.
    pub help: String,
}

/// The open panel's header row: the search field left of the notch and
/// the buttons right of it, left to right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchHeader {
    /// The search text.
    pub query: String,
    /// "Search".
    pub placeholder: String,
    /// The field's spoken label.
    pub search_label: String,
    /// How many Spaces match, while searching.
    pub match_count: Option<u32>,
    /// Left to right: the list, then Settings at the far right.
    pub buttons: Vec<NotchButton>,
}

/// What the indicator left of the closed notch shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotchActivityKind {
    /// A teleport or file transfer: a progress ring.
    Transfer,
    /// Someone is connected to this machine (host sharing): its symbol,
    /// pulsing, for as long as they are.
    RemoteAccess,
    /// The network hotspot: its symbol, pulsing.
    Hotspot,
    /// A Space starting: a progress ring on the startup estimate.
    Provisioning,
    /// A Space being deleted: a ring on the delete estimate.
    Deleting,
    /// Keyvault sign-ins live in a Space: the key symbol, pulsing.
    Keyvault,
}

/// The indicator left of the closed notch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchActivity {
    /// Which.
    pub kind: NotchActivityKind,
    /// Spoken label and tooltip.
    pub label: String,
    /// The symbol to draw (the hotspot), else a ring.
    pub symbol: Option<String>,
    /// Real progress in thousandths (a transfer with a known total).
    pub permille: Option<u32>,
    /// Epoch ms the ring's estimate runs from (the earliest starting
    /// Space); none means from when the ring appears.
    pub started_at: Option<i64>,
    /// The estimate reaches about 80% after this long ([`estimated_progress`]).
    pub estimate_ms: u32,
}

/// Time to about 80% on a ring without real progress (the measured Fleet
/// bind time).
pub const ACTIVITY_ESTIMATE_MS: u32 = 80_000;

/// Time to about 80% on the ring while a Space is deleted.
pub const DELETE_ESTIMATE_MS: u32 = 10_000;

/// The ring's fill without real progress, in thousandths: it eases toward
/// completion (about 80% at `estimate_ms`) and never passes 96% until the
/// work ends. Never below 4%, so the ring always shows.
pub fn estimated_progress(elapsed_ms: i64, estimate_ms: u32) -> u32 {
    let tau = f64::from(estimate_ms.max(1)) / 1.609;
    let p = 1.0 - (-(elapsed_ms.max(0) as f64) / tau).exp();
    (p.clamp(0.04, 0.96) * 1000.0).round() as u32
}

/// The SF Symbol of the remote-access indicator.
pub const REMOTE_ACCESS_SYMBOL: &str = "person.2.fill";

/// The indicator: a transfer first (it is brief and time-sensitive), then
/// someone connected to this machine, then live Keyvault sharing (access is
/// never silent), then the hotspot, then Spaces starting, then Spaces being
/// deleted. None when idle.
pub fn activity(
    spaces: &[Space],
    hotspot: bool,
    transfer: Option<&NotchTransfer>,
    keyvault: Option<&str>,
) -> Option<NotchActivity> {
    if let Some(t) = transfer {
        return Some(NotchActivity {
            kind: NotchActivityKind::Transfer,
            label: "Transferring to a Space".into(),
            symbol: None,
            permille: match (t.sent, t.total) {
                (Some(sent), Some(total)) if total > 0 => {
                    Some(((sent.min(total) as f64 / total as f64) * 1000.0).round() as u32)
                }
                _ => None,
            }
            .map(|p| p.max(40)),
            started_at: None,
            estimate_ms: ACTIVITY_ESTIMATE_MS,
        });
    }
    let connected: u32 = spaces.iter().map(crate::host::connected_now).sum();
    if connected > 0 {
        return Some(NotchActivity {
            kind: NotchActivityKind::RemoteAccess,
            label: if connected == 1 {
                "Someone is connected to this machine".into()
            } else {
                format!("{connected} are connected to this machine")
            },
            symbol: Some(REMOTE_ACCESS_SYMBOL.into()),
            permille: None,
            started_at: None,
            estimate_ms: ACTIVITY_ESTIMATE_MS,
        });
    }
    if let Some(label) = keyvault {
        return Some(NotchActivity {
            kind: NotchActivityKind::Keyvault,
            label: label.to_string(),
            symbol: Some("key.fill".into()),
            permille: None,
            started_at: None,
            estimate_ms: ACTIVITY_ESTIMATE_MS,
        });
    }
    if hotspot {
        return Some(NotchActivity {
            kind: NotchActivityKind::Hotspot,
            label: "Sharing your network to a Space".into(),
            symbol: Some("personalhotspot".into()),
            permille: None,
            started_at: None,
            estimate_ms: ACTIVITY_ESTIMATE_MS,
        });
    }
    let starting: Vec<&Space> = spaces
        .iter()
        .filter(|s| s.status == SpaceStatus::Provisioning)
        .collect();
    if starting.is_empty() {
        let n = spaces
            .iter()
            .filter(|s| s.status == SpaceStatus::Deleting)
            .count();
        return (n > 0).then(|| NotchActivity {
            kind: NotchActivityKind::Deleting,
            label: format!("Deleting {n} {}", if n == 1 { "Space" } else { "Spaces" }),
            symbol: None,
            permille: None,
            started_at: None,
            estimate_ms: DELETE_ESTIMATE_MS,
        });
    }
    let n = starting.len();
    Some(NotchActivity {
        kind: NotchActivityKind::Provisioning,
        label: format!("Starting {n} {}", if n == 1 { "Space" } else { "Spaces" }),
        symbol: None,
        // Real progress when every starting Space reports it (the least
        // advanced one), else the time estimate.
        permille: starting
            .iter()
            .map(|s| s.progress.as_ref().map(|p| p.permille))
            .collect::<Option<Vec<u32>>>()
            .and_then(|all| all.into_iter().min()),
        started_at: starting.iter().filter_map(|s| s.started_at).min(),
        estimate_ms: ACTIVITY_ESTIMATE_MS,
    })
}

fn header(query: &str, matched: Option<u32>) -> NotchHeader {
    NotchHeader {
        query: query.into(),
        placeholder: "Search".into(),
        search_label: "Filter Cua Spaces by name, OS, status, or location".into(),
        match_count: matched,
        buttons: vec![
            NotchButton {
                id: NotchButtonId::List,
                symbol: "list.bullet".into(),
                label: "List view".into(),
                help: "Open the Cua Spaces window".into(),
            },
            NotchButton {
                id: NotchButtonId::Settings,
                symbol: "gearshape".into(),
                label: "Settings".into(),
                help: "Settings".into(),
            },
        ],
    }
}

/// The drop-target tile under a point (AppKit coordinates, like
/// `layout`) in the open panel: tiles sit left to right from the content
/// inset, [`TILE_WIDTH`] wide with [`TILE_GAP`] between them, below the
/// notch row and (`row`) the line above the tiles. A few points of slack
/// round each tile so a drop on its edge still lands.
pub fn tile_at(
    layout: &NotchLayout,
    tiles: &[NotchTile],
    row: bool,
    x: f64,
    y: f64,
) -> Option<String> {
    const SLACK: f64 = 6.0;
    let open = layout.open_frame;
    let x0 = open.x
        + if layout.notch_style {
            OPEN_RADII.top
        } else {
            0.0
        }
        + CONTENT_PADDING;
    let tiles_top = open.y + open.height
        - if layout.notch_style {
            layout.notch.height
        } else {
            0.0
        }
        - CONTENT_PADDING
        - if row { PROMPT_HEIGHT } else { 0.0 };
    if y > tiles_top + SLACK || y < open.y || x < x0 - SLACK {
        return None;
    }
    let i = ((x - x0 + SLACK) / (TILE_WIDTH + TILE_GAP)).floor() as usize;
    tiles.get(i).filter(|t| t.drop_target).map(|t| t.id.clone())
}

/// Whether a Space takes a teleport drop (a dragged window, an app or
/// files): reachable and not still being created. The notch tiles and the
/// main window's Space rows use the same rule.
pub fn accepts_drop(space: &Space) -> bool {
    space.sdk.as_ref().is_none_or(|x| x.reachable)
        && !matches!(
            space.status,
            SpaceStatus::Provisioning | SpaceStatus::Deleting
        )
        && space.progress.is_none()
}

/// The panel as drawn for `spaces`.
pub fn view(state: &NotchState, spaces: &[Space]) -> NotchView {
    let phase = match state.drag.phase {
        DragOverlayPhase::Prompt => NotchPhase::Prompt,
        DragOverlayPhase::Selector => NotchPhase::Tiles,
        DragOverlayPhase::Idle if state.open || state.drop_targeted => NotchPhase::Tiles,
        DragOverlayPhase::Idle => NotchPhase::Closed,
    };
    let drop_mode = state.drop_targeted || state.drag.phase == DragOverlayPhase::Selector;
    let app = state.drag.app_name.clone().filter(|a| !a.is_empty());
    let prompt = match phase {
        NotchPhase::Prompt => Some("Teleport to Cua".to_string()),
        NotchPhase::Tiles if drop_mode => Some(match &app {
            Some(a) => format!("Drop {a} on a Space"),
            None => "Drop on a Space".to_string(),
        }),
        _ => None,
    };
    // A drag drops on any Space: the search only narrows browsing.
    let query = if drop_mode { "" } else { state.query.trim() };
    let mut visible = if phase == NotchPhase::Tiles {
        tiles_matching(spaces, state.drag.target_space_id.as_deref(), query)
    } else {
        Vec::new()
    };
    for t in &mut visible {
        t.signed_in = state.signed_in.contains(&t.id);
    }
    let n = visible.len();
    let remote = remote_count(spaces);
    let searching = !query.is_empty();
    let header = (phase == NotchPhase::Tiles && !drop_mode)
        .then(|| header(&state.query, searching.then_some(n as u32)));
    let empty = (phase == NotchPhase::Tiles && searching && n == 0)
        .then(|| format!("No Spaces match \u{201c}{query}\u{201d}."));
    let activity = (phase == NotchPhase::Closed)
        .then(|| {
            activity(
                spaces,
                state.hotspot,
                state.transfer.as_ref(),
                state.keyvault.as_deref(),
            )
        })
        .flatten();
    let permission = (state.drag_permission_missing && phase == NotchPhase::Tiles && !drop_mode)
        .then(|| NotchPermission {
            text: "Allow Accessibility to teleport dragged windows.".into(),
            action: "Open Settings".into(),
            pane: "accessibility".into(),
        });
    let access = (phase == NotchPhase::Tiles && !drop_mode && permission.is_none())
        .then(|| state.keyvault.clone())
        .flatten()
        .map(|text| NotchAccess {
            text,
            dismiss: "Dismiss".into(),
        });
    let (show_tab, hover_cue, activity) = if state.hidden {
        (false, false, None)
    } else {
        (
            phase == NotchPhase::Closed && state.drag.phase == DragOverlayPhase::Idle,
            phase == NotchPhase::Closed && state.hovering,
            activity,
        )
    };
    NotchView {
        count_label: format!("{remote} {}", if remote == 1 { "Space" } else { "Spaces" }),
        tab: tab(spaces),
        hidden: state.hidden,
        header,
        empty,
        activity,
        show_tab,
        hover_cue,
        permission,
        access,
        phase,
        label: match phase {
            NotchPhase::Closed => "Cua Spaces".into(),
            NotchPhase::Prompt => "Teleport to Cua".into(),
            NotchPhase::Tiles => format!("{n} {}", if n == 1 { "Space" } else { "Spaces" }),
        },
        tiles: visible,
        drop_mode,
        prompt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mbp14() -> ScreenFacts {
        ScreenFacts {
            frame: LogicalRect::new(0.0, 0.0, 1512.0, 982.0),
            visible_frame: LogicalRect::new(0.0, 0.0, 1512.0, 945.0),
            safe_area_top: 32.0,
            aux_left_width: Some(662.0),
            aux_right_width: Some(662.0),
        }
    }

    #[test]
    fn drops_need_a_reachable_space_that_is_not_being_created() {
        let mut s = crate::host::this_machine_space(None, 0, crate::model::SpaceOs::Macos);
        assert!(accepts_drop(&s), "no SDK record: the host itself");
        s.sdk = Some(crate::model::SpaceSdkRef {
            features: vec![],
            spacesd_version: "0.4.0".into(),
            reachable: false,
            error: Some("timed out".into()),
        });
        assert!(!accepts_drop(&s));
        s.sdk.as_mut().unwrap().reachable = true;
        assert!(accepts_drop(&s));
        s.status = SpaceStatus::Provisioning;
        assert!(!accepts_drop(&s));
    }

    #[test]
    fn notch_rect_from_auxiliary_areas() {
        let l = layout(&mbp14(), false);
        assert!(l.has_notch);
        assert_eq!(l.notch.width, 192.0);
        assert_eq!(l.notch.height, 32.0);
        assert_eq!(l.notch.y + l.notch.height, 982.0);
        assert_eq!(l.notch.x + l.notch.width / 2.0, 756.0);
        assert!(l.open_frame.width <= 640.0);
    }

    #[test]
    fn no_notch_gets_a_virtual_notch_under_the_menu_bar() {
        let l = layout(
            &ScreenFacts {
                frame: LogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
                visible_frame: LogicalRect::new(0.0, 0.0, 1920.0, 1055.0),
                safe_area_top: 0.0,
                aux_left_width: None,
                aux_right_width: None,
            },
            true,
        );
        assert!(!l.has_notch && !l.notch_style);
        assert_eq!(l.notch.width, 240.0);
        assert_eq!(l.notch.height, 25.0);
        assert!(l.open_frame.height > layout(&mbp14(), false).open_frame.height - 32.0);
    }

    fn space(id: &str, name: &str, os: SpaceOs, status: SpaceStatus) -> Space {
        let mut s = crate::host::this_machine_space(None, 0, os);
        s.id = id.into();
        s.name = name.into();
        s.status = status;
        s
    }

    #[test]
    fn the_stage_holds_every_frame_and_stays_centred_on_the_notch() {
        for prompt in [false, true] {
            let l = layout(&mbp14(), prompt);
            let st = l.stage_frame;
            for r in [
                l.notch,
                l.closed_frame,
                l.open_frame,
                l.prompt_frame,
                l.tab_frame,
            ] {
                assert!(
                    r.x >= st.x && r.x + r.width <= st.x + st.width,
                    "{r:?} in {st:?}"
                );
                assert!(
                    r.y >= st.y && r.y + r.height <= st.y + st.height,
                    "{r:?} in {st:?}"
                );
            }
            assert_eq!(st.x + st.width / 2.0, l.notch.x + l.notch.width / 2.0);
            assert_eq!(st.y + st.height, 982.0, "flush with the top of the screen");
        }
        let l = layout(&mbp14(), false);
        assert_eq!(
            l.tab_frame.x,
            l.notch.x + l.notch.width,
            "the tab hangs off the right edge"
        );
        assert_eq!(l.tab_frame.height, l.notch.height);
        // The tab's content sits against the notch, 6 pt from its outer edge.
        assert_eq!((l.tab_inset_notch, l.tab_inset_outer), (0.0, 6.0));
        // Four tiles fit the open panel without scrolling.
        let inner = l.open_frame.width - 2.0 * (OPEN_RADII.top + CONTENT_PADDING);
        assert!(4.0 * TILE_WIDTH + 3.0 * TILE_GAP <= inner, "{inner}");
    }

    #[test]
    fn tiles_carry_the_os_icon_and_a_spoken_label() {
        let mut ubuntu = space("a", "Aurora", SpaceOs::Linux, SpaceStatus::Running);
        ubuntu.os_name = Some("Ubuntu".into());
        let spaces = vec![
            ubuntu,
            space("b", "Lab PC", SpaceOs::Windows, SpaceStatus::Suspended),
            space("c", "Studio", SpaceOs::Macos, SpaceStatus::Running),
            space("d", "Box", SpaceOs::Linux, SpaceStatus::Running),
        ];
        let t = tiles(&spaces, Some("b"));
        let by = |id: &str| t.iter().find(|x| x.id == id).unwrap().clone();
        assert_eq!(by("a").symbol, "os-ubuntu");
        assert_eq!(by("b").symbol, "os-windows");
        assert_eq!(by("c").symbol, "os-macos");
        assert_eq!(by("d").symbol, "os-linux", "no distro reported: Tux");
        assert_eq!(by("b").label, "Lab PC, Windows, Cua Cloud, Suspended");
        assert!(by("b").targeted && by("b").dim);
    }

    #[test]
    fn tiles_say_where_each_space_runs() {
        use crate::model::SpaceProvider;
        let with = |id: &str, name: &str, p: SpaceProvider| {
            let mut s = space(id, name, SpaceOs::Linux, SpaceStatus::Running);
            s.provider = Some(p);
            s
        };
        let local = with("local:dev", "Dev", SpaceProvider::Local);
        let machine = with("relay:m1", "Mac mini", SpaceProvider::Relay);
        let mut named = with("relay:m1/a", "Aurora", SpaceProvider::Relay);
        named.host = Some("m1".into());
        named.host_name = Some("Studio Mac mini".into());
        let mut unnamed = named.clone();
        unnamed.id = "relay:m1/b".into();
        unnamed.host_name = None;
        let mut stranger = unnamed.clone();
        stranger.id = "relay:m9/c".into();
        stranger.host = Some("m9".into());
        let direct = with("direct:10.0.0.7:8000", "Box", SpaceProvider::Direct);
        let mut aws = with("relay:aws/x", "Worker", SpaceProvider::Relay);
        aws.cloud = Some("aws".into());
        aws.cloud_place = Some("AWS \u{b7} us-west-2".into());
        let all = vec![
            local.clone(),
            machine.clone(),
            named.clone(),
            unnamed.clone(),
            stranger.clone(),
            direct.clone(),
            aws.clone(),
        ];
        for (s, want) in [
            (&local, "This Mac"),
            (&machine, "Mac mini"),
            (&named, "Studio Mac mini"),
            (&unnamed, "Mac mini"),
            (&stranger, "m9"),
            (&direct, "10.0.0.7:8000"),
            (&aws, "AWS \u{b7} us-west-2"),
        ] {
            assert_eq!(location(s, &all), want, "{}", s.id);
        }
        let t = tiles(&[local], None);
        assert_eq!(t[0].location, "This Mac");
        assert_eq!(t[0].label, "Dev, Linux, This Mac, Running");
    }

    #[test]
    fn the_tab_counts_remote_spaces_and_tucks_away_during_a_drag() {
        let spaces = vec![
            space("a", "Aurora", SpaceOs::Linux, SpaceStatus::Running),
            crate::host::this_machine_space(None, 0, SpaceOs::Macos),
        ];
        let v = view(&NotchState::default(), &spaces);
        assert_eq!(v.count_label, "1 Space");
        assert!(v.show_tab && !v.hover_cue);
        let s = reduce(&NotchState::default(), &NotchEvent::HoverEnter).state;
        assert!(view(&s, &spaces).hover_cue, "the dwell shows a cue");
        let s = reduce(
            &s,
            &NotchEvent::Drag {
                event: DragOverlayEvent::Start {
                    window_id: Some(1),
                    app_name: None,
                },
            },
        )
        .state;
        let v = view(&s, &spaces);
        assert_eq!(v.phase, NotchPhase::Prompt);
        assert!(!v.show_tab && !v.hover_cue);
        assert_eq!(v.prompt.as_deref(), Some("Teleport to Cua"));
    }

    #[test]
    fn a_missing_permission_is_one_line_in_the_open_panel() {
        let spaces = vec![space("a", "Aurora", SpaceOs::Linux, SpaceStatus::Running)];
        let s = reduce(
            &NotchState::default(),
            &NotchEvent::DragPermission { granted: false },
        )
        .state;
        assert!(view(&s, &spaces).permission.is_none(), "closed: nothing");
        let open = reduce(&s, &NotchEvent::Click).state;
        let p = view(&open, &spaces).permission.expect("open: the line");
        assert_eq!(p.action, "Open Settings");
        assert_eq!(p.pane, "accessibility");
        assert!(!p.text.contains('\n'));
        // A pasteboard drag is a drop target, not the place for it.
        let dropping = reduce(&s, &NotchEvent::DropTargeted { targeted: true }).state;
        assert!(view(&dropping, &spaces).permission.is_none());
        let granted = reduce(&open, &NotchEvent::DragPermission { granted: true }).state;
        assert!(view(&granted, &spaces).permission.is_none());
    }

    #[test]
    fn os_icons_cover_distros_and_every_icon_has_artwork() {
        let icon = |n: &str| os_icon(SpaceOs::Linux, Some(n));
        assert_eq!(icon("Ubuntu 24.04.1 LTS"), "os-ubuntu");
        assert_eq!(icon("Omarchy"), "os-omarchy", "not Arch");
        assert_eq!(icon("Arch Linux"), "os-arch");
        assert_eq!(icon("Debian GNU/Linux"), "os-linux", "no icon shipped");
        assert_eq!(icon("Red Hat Enterprise Linux"), "os-redhat");
        assert_eq!(
            os_icon(SpaceOs::Windows, Some("Windows 11 Pro")),
            "os-windows"
        );
        assert_eq!(os_icon(SpaceOs::Macos, None), "os-macos");
        for id in OS_ICONS {
            let svg = os_icon_svg(id).unwrap_or_else(|| panic!("{id}"));
            assert!(
                svg.starts_with("<svg") && svg.contains("viewBox=\"0 0 24 24\""),
                "{id}"
            );
        }
        for (_, id) in DISTROS {
            assert!(OS_ICONS.contains(id), "{id}");
        }
        assert_eq!(os_icon_system_symbol("os-macos"), Some("apple.logo"));
        assert_eq!(os_icon_system_symbol("os-ubuntu"), None);
    }

    #[test]
    fn every_catalog_distribution_gets_its_own_icon_from_its_name() {
        for image in crate::wizard::picker_images() {
            let d = image.distro.expect("every image names its distribution");
            let want = format!("os-{}", d.id);
            assert!(OS_ICONS.contains(&want.as_str()), "{}", image.image_ref);
            assert_eq!(
                os_icon(image.os, Some(&d.name)),
                want,
                "{}",
                image.image_ref
            );
        }
    }

    #[test]
    fn the_tab_is_the_count_over_the_word() {
        let mut spaces = vec![crate::host::this_machine_space(None, 0, SpaceOs::Macos)];
        assert_eq!(
            tab(&spaces),
            NotchTab {
                count: "0".into(),
                word: "Spaces".into()
            }
        );
        spaces.push(space("a", "Aurora", SpaceOs::Linux, SpaceStatus::Running));
        assert_eq!(
            tab(&spaces),
            NotchTab {
                count: "1".into(),
                word: "Space".into()
            }
        );
        for i in 0..8 {
            spaces.push(space(
                &format!("s{i}"),
                "dev",
                SpaceOs::Linux,
                SpaceStatus::Running,
            ));
        }
        let v = view(&NotchState::default(), &spaces);
        assert_eq!((v.tab.count.as_str(), v.tab.word.as_str()), ("9", "Spaces"));
        assert_eq!(v.count_label, "9 Spaces");
    }

    #[test]
    fn the_header_searches_and_escape_clears_before_closing() {
        let mut aurora = space("a", "Aurora", SpaceOs::Linux, SpaceStatus::Running);
        aurora.os_name = Some("Ubuntu".into());
        let spaces = vec![
            aurora,
            space("b", "Lab PC", SpaceOs::Windows, SpaceStatus::Suspended),
        ];
        let closed = view(&NotchState::default(), &spaces);
        assert!(closed.header.is_none(), "closed: no header");
        let open = reduce(&NotchState::default(), &NotchEvent::Click).state;
        let h = view(&open, &spaces).header.expect("open: the header");
        assert_eq!(h.placeholder, "Search");
        assert_eq!(h.match_count, None);
        let ids: Vec<_> = h
            .buttons
            .iter()
            .map(|b| (b.id, b.symbol.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                (NotchButtonId::List, "list.bullet"),
                (NotchButtonId::Settings, "gearshape")
            ]
        );
        for (q, want) in [
            ("ubuntu", vec!["Aurora"]),
            ("WINDOWS", vec!["Lab PC"]),
            ("suspended lab", vec!["Lab PC"]),
            ("", vec!["Aurora", "Lab PC"]),
        ] {
            let s = reduce(&open, &NotchEvent::Search { query: q.into() }).state;
            let v = view(&s, &spaces);
            let names: Vec<_> = v.tiles.iter().map(|t| t.name.as_str()).collect();
            assert_eq!(names, want, "{q}");
            assert_eq!(
                v.header.unwrap().match_count,
                (!q.is_empty()).then_some(want.len() as u32)
            );
        }
        let s = reduce(
            &open,
            &NotchEvent::Search {
                query: "zzz".into(),
            },
        )
        .state;
        assert_eq!(
            view(&s, &spaces).empty.as_deref(),
            Some("No Spaces match \u{201c}zzz\u{201d}.")
        );
        // Esc clears the search, then closes.
        let s = reduce(&s, &NotchEvent::Escape).state;
        assert!(s.open && s.query.is_empty());
        let s = reduce(&s, &NotchEvent::Escape).state;
        assert!(!s.open);
        // Closing forgets the search.
        let s = reduce(
            &open,
            &NotchEvent::Search {
                query: "lab".into(),
            },
        )
        .state;
        assert!(reduce(&s, &NotchEvent::Dismiss).state.query.is_empty());
        // A drop hint replaces the header, and every Space is a target.
        let s = reduce(&s, &NotchEvent::DropTargeted { targeted: true }).state;
        let v = view(&s, &spaces);
        assert!(v.header.is_none());
        assert_eq!(v.tiles.len(), 2);
    }

    #[test]
    fn live_keyvault_sharing_shows_left_of_the_notch() {
        let label = "Keyvault sign-ins live in dev-1";
        let a = activity(&[], true, None, Some(label)).unwrap();
        assert_eq!(a.kind, NotchActivityKind::Keyvault);
        assert_eq!(a.label, label);
        assert_eq!(a.symbol.as_deref(), Some("key.fill"));
        // A transfer in flight still comes first.
        let t = NotchTransfer {
            sent: None,
            total: None,
        };
        assert_eq!(
            activity(&[], true, Some(&t), Some(label)).unwrap().kind,
            NotchActivityKind::Transfer
        );
        // Through the event, and gone when sharing stops.
        let on = reduce(
            &NotchState::default(),
            &NotchEvent::Keyvault {
                label: Some(label.into()),
                signed_in: vec![],
            },
        )
        .state;
        assert_eq!(
            view(&on, &[]).activity.unwrap().kind,
            NotchActivityKind::Keyvault
        );
        let off = reduce(
            &on,
            &NotchEvent::Keyvault {
                label: None,
                signed_in: vec![],
            },
        )
        .state;
        assert!(view(&off, &[]).activity.is_none());
    }

    #[test]
    fn signed_in_spaces_carry_the_key_on_their_tiles() {
        let mut a = space("a", "a", SpaceOs::Linux, SpaceStatus::Running);
        a.last_used_at = 10;
        let mut b = space("b", "b", SpaceOs::Linux, SpaceStatus::Running);
        b.last_used_at = 5;
        let spaces = vec![a, b];
        let mut s = reduce(
            &NotchState::default(),
            &NotchEvent::Keyvault {
                label: Some("Keyvault sign-ins live in a".into()),
                signed_in: vec!["a".into()],
            },
        )
        .state;
        s.open = true;
        let v = view(&s, &spaces);
        let keyed: Vec<(&str, bool)> = v
            .tiles
            .iter()
            .map(|t| (t.id.as_str(), t.signed_in))
            .collect();
        assert_eq!(keyed, [("a", true), ("b", false)]);
        // Dismissed (the shell sends the rest): no key, no indicator.
        let s = reduce(
            &s,
            &NotchEvent::Keyvault {
                label: None,
                signed_in: vec![],
            },
        )
        .state;
        let v = view(&s, &spaces);
        assert!(v.tiles.iter().all(|t| !t.signed_in));
        assert!(v.activity.is_none());
        assert!(v.access.is_none(), "dismissed: no line either");
    }

    #[test]
    fn the_open_panel_names_live_access_with_a_dismiss() {
        let label = "Keyvault sign-ins live in dev-1";
        let mut s = reduce(
            &NotchState::default(),
            &NotchEvent::Keyvault {
                label: Some(label.into()),
                signed_in: vec![],
            },
        )
        .state;
        assert!(view(&s, &[]).access.is_none(), "closed: the indicator only");
        s.open = true;
        let a = view(&s, &[]).access.unwrap();
        assert_eq!((a.text.as_str(), a.dismiss.as_str()), (label, "Dismiss"));
        // A drop hint takes the line.
        s.drop_targeted = true;
        assert!(view(&s, &[]).access.is_none());
    }

    #[test]
    fn someone_connected_to_this_machine_shows_until_they_leave() {
        use crate::host::{HostSummaryInput, this_machine_space};
        let input = HostSummaryInput {
            configured: true,
            service_running: true,
            sharing: true,
            clients: 1,
            ..Default::default()
        };
        let me = this_machine_space(Some(&input), 0, SpaceOs::Macos);
        let a = activity(
            std::slice::from_ref(&me),
            true,
            None,
            Some("Keyvault sign-ins live in dev-1"),
        )
        .unwrap();
        assert_eq!(
            a.kind,
            NotchActivityKind::RemoteAccess,
            "before the hotspot"
        );
        assert_eq!(a.symbol.as_deref(), Some(REMOTE_ACCESS_SYMBOL));
        assert_eq!(a.label, "Someone is connected to this machine");
        // A transfer in flight still shows its progress first.
        let t = NotchTransfer {
            sent: Some(1),
            total: Some(2),
        };
        assert_eq!(
            activity(std::slice::from_ref(&me), false, Some(&t), None)
                .unwrap()
                .kind,
            NotchActivityKind::Transfer
        );
        let two = this_machine_space(
            Some(&HostSummaryInput {
                clients: 2,
                ..input.clone()
            }),
            0,
            SpaceOs::Macos,
        );
        assert_eq!(
            activity(&[two], false, None, None).unwrap().label,
            "2 are connected to this machine"
        );
        let gone = this_machine_space(
            Some(&HostSummaryInput {
                clients: 0,
                ..input
            }),
            0,
            SpaceOs::Macos,
        );
        assert!(activity(&[gone], false, None, None).is_none());
    }

    #[test]
    fn activity_is_transfer_then_hotspot_then_starting() {
        let mut starting = space("p", "New", SpaceOs::Linux, SpaceStatus::Provisioning);
        starting.started_at = Some(1_000);
        let mut later = space("q", "Newer", SpaceOs::Linux, SpaceStatus::Provisioning);
        later.started_at = Some(5_000);
        let spaces = vec![starting, later];
        assert!(activity(&[], false, None, None).is_none(), "idle");
        let a = activity(&spaces, false, None, None).unwrap();
        assert_eq!(a.kind, NotchActivityKind::Provisioning);
        assert_eq!(a.label, "Starting 2 Spaces");
        assert_eq!(a.started_at, Some(1_000), "the earliest start");
        let a = activity(&spaces, true, None, None).unwrap();
        assert_eq!(
            (a.kind, a.symbol.as_deref()),
            (NotchActivityKind::Hotspot, Some("personalhotspot"))
        );
        let t = NotchTransfer {
            sent: Some(25),
            total: Some(100),
        };
        let a = activity(&spaces, true, Some(&t), None).unwrap();
        assert_eq!(
            (a.kind, a.permille),
            (NotchActivityKind::Transfer, Some(250))
        );
        let unknown = NotchTransfer {
            sent: Some(5),
            total: None,
        };
        assert_eq!(
            activity(&[], false, Some(&unknown), None).unwrap().permille,
            None
        );
        // The view carries it while closed, through the Activity event.
        let s = reduce(
            &NotchState::default(),
            &NotchEvent::Activity {
                hotspot: true,
                transfer: None,
            },
        )
        .state;
        assert_eq!(
            view(&s, &spaces).activity.unwrap().kind,
            NotchActivityKind::Hotspot
        );
        let open = reduce(&s, &NotchEvent::Click).state;
        assert!(
            view(&open, &spaces).activity.is_none(),
            "open: the panel covers it"
        );
        // The estimate: ~80% at the estimate, floor 4%, cap 96%.
        assert_eq!(estimated_progress(0, 80_000), 40);
        assert!((795..=805).contains(&estimated_progress(80_000, 80_000)));
        assert_eq!(estimated_progress(10_000_000, 80_000), 960);
    }

    #[test]
    fn a_space_being_deleted_is_a_dim_tile_and_a_ring() {
        let gone = space("d", "Gone", SpaceOs::Linux, SpaceStatus::Deleting);
        let a = activity(std::slice::from_ref(&gone), false, None, None).unwrap();
        assert_eq!(
            (a.kind, a.label.as_str(), a.symbol, a.estimate_ms),
            (
                NotchActivityKind::Deleting,
                "Deleting 1 Space",
                None,
                DELETE_ESTIMATE_MS
            )
        );
        // Starting outranks deleting.
        let new = space("p", "New", SpaceOs::Linux, SpaceStatus::Provisioning);
        assert_eq!(
            activity(&[gone.clone(), new], false, None, None)
                .unwrap()
                .kind,
            NotchActivityKind::Provisioning
        );
        let t = &tiles(&[gone], None)[0];
        assert!(t.dim && !t.drop_target);
        assert_eq!(
            (t.progress, t.progress_label.as_deref()),
            (None, Some("Deleting\u{2026}"))
        );
        assert_eq!(t.label, "Gone, Linux, Cua Cloud, Deleting\u{2026}");
    }

    #[test]
    fn a_hidden_notch_shows_nothing_and_takes_no_hover_or_drag() {
        let spaces = vec![space(
            "a",
            "Aurora",
            SpaceOs::Linux,
            SpaceStatus::Provisioning,
        )];
        let open = reduce(&NotchState::default(), &NotchEvent::Click).state;
        let t = reduce(&open, &NotchEvent::Visibility { shown: false });
        assert!(t.effects.contains(&NotchEffect::CancelTimers));
        let hidden = t.state;
        assert!(!hidden.open && hidden.hidden);
        let v = view(&hidden, &spaces);
        assert!(v.hidden && !v.show_tab && !v.hover_cue && v.activity.is_none());
        for e in [
            NotchEvent::HoverEnter,
            NotchEvent::DwellElapsed,
            NotchEvent::Click,
            NotchEvent::DropTargeted { targeted: true },
            NotchEvent::Drag {
                event: DragOverlayEvent::Start {
                    window_id: Some(1),
                    app_name: None,
                },
            },
        ] {
            let t = reduce(&hidden, &e);
            assert_eq!(t.state, hidden, "{e:?}");
            assert!(t.effects.is_empty(), "{e:?}");
        }
        let shown = reduce(&hidden, &NotchEvent::Visibility { shown: true }).state;
        let v = view(&shown, &spaces);
        assert!(!v.hidden && v.show_tab && v.activity.is_some());
        assert!(reduce(&shown, &NotchEvent::Click).state.open);
    }

    #[test]
    fn motion_opens_with_a_spring_and_closes_without_overshoot() {
        // What the shells receive over `notch.motion`, not the constant: the
        // wire must carry the shared motion and it must keep its shape.
        let json = crate::dispatch::call("notch.motion", "").unwrap();
        let m: NotchMotion = serde_json::from_str(&json).unwrap();
        assert_eq!(m, MOTION);
        assert_eq!(m.hover_dwell_ms, 300);
        assert_eq!(m.close_damping, 1.0, "critically damped close");
        assert!(m.open_damping < 1.0, "a little bounce on open");
        assert!(m.content_delay_ms > 0, "content follows the shape");
        assert!(m.content_out < m.close_response, "content leaves first");
        assert!(m.hover_scale > 1.0 && m.hover_scale < 1.15);
        // The cue answers the pointer at once with a little bounce, a few
        // points wider and taller, well before the dwell opens it.
        assert!(m.hover_scale_y > 1.0 && m.hover_scale_y < 1.2);
        assert!(m.hover_response <= 0.3 && m.hover_damping >= 0.6 && m.hover_damping <= 0.7);
        assert!(m.hover_response * 1000.0 <= f64::from(m.hover_dwell_ms));
    }

    #[test]
    fn hover_opens_after_the_dwell_and_closes_after_leaving() {
        let s = NotchState::default();
        let t = reduce(&s, &NotchEvent::HoverEnter);
        assert!(t.effects.contains(&NotchEffect::StartDwell { ms: 300 }));
        assert!(!t.state.open);
        let t = reduce(&t.state, &NotchEvent::DwellElapsed);
        assert!(t.state.open);
        let t = reduce(&t.state, &NotchEvent::HoverExit);
        assert!(t.effects.contains(&NotchEffect::StartClose { ms: 400 }));
        let t = reduce(&t.state, &NotchEvent::CloseElapsed);
        assert!(!t.state.open);
        // A dwell that fires after leaving does nothing.
        let t = reduce(&NotchState::default(), &NotchEvent::DwellElapsed);
        assert!(!t.state.open);
    }

    #[test]
    fn tile_at_finds_the_drop_target_under_a_point() {
        let l = layout(&mbp14(), true);
        let mut a = space("a", "A", SpaceOs::Linux, SpaceStatus::Running);
        a.last_used_at = 3;
        let mut b = space("b", "B", SpaceOs::Linux, SpaceStatus::Provisioning);
        b.last_used_at = 2;
        let mut c = space("c", "C", SpaceOs::Windows, SpaceStatus::Running);
        c.last_used_at = 1;
        let t = tiles(&[a, b, c], None);
        let x0 = l.open_frame.x + OPEN_RADII.top + CONTENT_PADDING;
        // The tiles' top: below the notch row and the line above them.
        let top =
            l.open_frame.y + l.open_frame.height - l.notch.height - CONTENT_PADDING - PROMPT_HEIGHT;
        let y = top - 40.0;
        assert_eq!(tile_at(&l, &t, true, x0 + 10.0, y).as_deref(), Some("a"));
        // The second tile is still being created: not a target.
        let second = x0 + TILE_WIDTH + TILE_GAP + 10.0;
        assert_eq!(tile_at(&l, &t, true, second, y), None);
        let third = x0 + 2.0 * (TILE_WIDTH + TILE_GAP) + 10.0;
        assert_eq!(tile_at(&l, &t, true, third, y).as_deref(), Some("c"));
        // Above the tiles (the line), left of them, below the panel, past
        // the last tile: none.
        assert_eq!(tile_at(&l, &t, true, x0 + 10.0, top + 10.0), None);
        assert_eq!(tile_at(&l, &t, true, x0 - 10.0, y), None);
        assert_eq!(tile_at(&l, &t, true, x0 + 10.0, l.open_frame.y - 1.0), None);
        let fourth = x0 + 3.0 * (TILE_WIDTH + TILE_GAP) + 10.0;
        assert_eq!(tile_at(&l, &t, true, fourth, y), None);
        // Without the line the tiles start higher.
        assert_eq!(
            tile_at(&layout(&mbp14(), false), &t, false, x0 + 10.0, top + 10.0).as_deref(),
            Some("a")
        );
    }
}
