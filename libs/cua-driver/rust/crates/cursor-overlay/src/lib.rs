//! cursor-overlay — shared types and math for the cua-driver cursor overlay.
//!
//! Platform renderers (macOS, Windows, Linux) depend on this crate for:
//! - `CursorConfig` — theme, accessibility, visibility, and motion settings
//! - `MotionConfig` — glide duration, spring, dwell, idle-hide timings
//! - `CubicBezier` + `PathPlanner` — Bezier path math (ported 1:1 from C#)
//! - `OverlayCommand` — messages sent from MCP tools to the overlay thread

pub mod badge_glyphs;
pub mod bezier;
pub mod capture_utils;
pub mod motion;
pub mod path_planner;
pub mod render_state;
pub mod session_badge;
pub mod theme;
pub mod theme_artifact;
pub mod util;
pub mod z_order;

pub use badge_glyphs::{BadgeChip, BadgeGlyph};
pub use bezier::CubicBezier;
pub use motion::{MotionConfig, Spring};
pub use path_planner::{PathPlanner, PathState, PlannedPath};
pub use render_state::{
    paint_cursor, render_frame, FocusRect, RenderStateCore, SESSION_BADGE_FADE_SECS,
    SESSION_BADGE_HOLD_SECS,
};
pub use session_badge::{
    paint_session_badge, sanitize_session_label, session_badge_extents, session_badge_layout,
    BadgeExtents, BadgeLabelLayout, SessionBadgeInput, SessionBadgeLayout, BADGE_CHIP_GAP,
    BADGE_CHIP_GROUP_GAP, BADGE_CHIP_SIZE, BADGE_CURSOR_GAP, BADGE_HEIGHT, BADGE_MAX_WIDTH,
    MAX_SESSION_LABEL_CHARS,
};
pub use theme::{
    session_fill_hex, session_fill_rgba, CursorAction, CursorVisualState, DeliveryModifier,
    PlaybackKind, ReducedMotion, TargetModifier, DEFAULT_CURSOR_FILL, DEFAULT_THEME_ID,
    DEFAULT_THEME_VERSION, THEME_PROFILE,
};
pub use theme_artifact::{
    decode_theme, embedded_default_theme, inspect_artifact, list_installed_themes,
    load_installed_theme, paint_compiled_theme, paint_compiled_theme_with_tint,
    resolve_theme_selection, theme_store_root, validate_compiled_theme, CompiledAnimation,
    CompiledDrawCommand, CompiledFrame, CompiledGeometry, CompiledStroke, CompiledTheme,
    CompiledTransform,
};
#[cfg(feature = "theme-authoring")]
pub use theme_artifact::{encode_theme, install_artifact, uninstall_theme};
pub use z_order::ZOrderEnforcer;

/// Configuration assembled from CLI arguments and passed to every
/// platform backend when it initialises the overlay window.
#[derive(Debug, Clone)]
pub struct CursorConfig {
    /// Multi-cursor instance identifier. Defaults to `"default"`.
    pub cursor_id: String,

    /// Installed theme selected at launch.
    pub theme_id: String,

    /// Accessibility motion preference.
    pub reduced_motion: ReducedMotion,

    /// Initial motion config (can be updated at runtime via MCP tool).
    pub motion: MotionConfig,

    /// Whether the overlay is visible at startup.
    /// Pass `--no-overlay` to disable.
    pub enabled: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            cursor_id: "default".into(),
            theme_id: DEFAULT_THEME_ID.into(),
            reduced_motion: ReducedMotion::Auto,
            motion: MotionConfig::default(),
            enabled: true,
        }
    }
}

impl CursorConfig {
    /// Parse from `std::env::args()`.
    ///
    /// Recognised flags:
    /// ```text
    /// --cursor-theme <installed-theme-id>
    /// --cursor-reduced-motion <auto|on|off>
    /// --no-overlay                (start with overlay disabled)
    /// --glide-ms     <f64>        (glideDurationMs override)
    /// --dwell-ms     <f64>        (dwellAfterClickMs override)
    /// --idle-hide-ms <f64>        (idleHideMs override)
    /// ```
    pub fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self::parse(&args[1..])
    }

    pub fn parse(args: &[String]) -> Self {
        let mut cfg = CursorConfig::default();
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "--cursor-theme" => {
                    if let Some(theme_id) = args.get(i + 1) {
                        cfg.theme_id = theme_id.clone();
                        i += 1;
                    }
                }
                "--cursor-reduced-motion" => {
                    if let Some(value) = args.get(i + 1) {
                        cfg.reduced_motion = match value.as_str() {
                            "auto" => ReducedMotion::Auto,
                            "on" => ReducedMotion::On,
                            "off" => ReducedMotion::Off,
                            _ => {
                                tracing::warn!(
                                    "--cursor-reduced-motion {value}: expected auto|on|off; using auto"
                                );
                                ReducedMotion::Auto
                            }
                        };
                        i += 1;
                    }
                }
                "--no-overlay" => cfg.enabled = false,
                "--glide-ms" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        cfg.motion.glide_duration_ms = v;
                        i += 1;
                    }
                }
                "--dwell-ms" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        cfg.motion.dwell_after_click_ms = v;
                        i += 1;
                    }
                }
                "--idle-hide-ms" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        cfg.motion.idle_hide_ms = v;
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        cfg
    }
}

// ── Shared cursor instance registry ──────────────────────────────────────────

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-instance cursor configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorInstanceConfig {
    pub cursor_id: String,
    pub theme_id: String,
    pub reduced_motion: ReducedMotion,
    pub enabled: bool,
}

impl Default for CursorInstanceConfig {
    fn default() -> Self {
        Self {
            cursor_id: "default".into(),
            theme_id: DEFAULT_THEME_ID.into(),
            reduced_motion: ReducedMotion::Auto,
            enabled: true,
        }
    }
}

/// Runtime state for a cursor instance (config + last known position).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorInstanceState {
    pub config: CursorInstanceConfig,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

/// Global registry of cursor instances, keyed by `cursor_id`.
pub struct CursorRegistry {
    inner: Mutex<HashMap<String, CursorInstanceState>>,
}

impl CursorRegistry {
    pub fn new() -> Self {
        let mut map = HashMap::new();
        map.insert(
            "default".into(),
            CursorInstanceState {
                config: CursorInstanceConfig::default(),
                x: None,
                y: None,
            },
        );
        Self {
            inner: Mutex::new(map),
        }
    }

    pub fn get_or_create(&self, cursor_id: &str) -> CursorInstanceState {
        let mut inner = self.inner.lock().unwrap();
        inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorInstanceState {
                config: CursorInstanceConfig {
                    cursor_id: cursor_id.to_owned(),
                    ..Default::default()
                },
                x: None,
                y: None,
            })
            .clone()
    }

    /// Read one cursor without materializing a new registry entry.
    pub fn get(&self, cursor_id: &str) -> Option<CursorInstanceState> {
        self.inner.lock().unwrap().get(cursor_id).cloned()
    }

    pub fn update_position(&self, cursor_id: &str, x: f64, y: f64) {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorInstanceState {
                config: CursorInstanceConfig {
                    cursor_id: cursor_id.to_owned(),
                    ..Default::default()
                },
                x: None,
                y: None,
            });
        state.x = Some(x);
        state.y = Some(y);
    }

    pub fn set_enabled(&self, cursor_id: &str, enabled: bool) {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorInstanceState {
                config: CursorInstanceConfig {
                    cursor_id: cursor_id.to_owned(),
                    ..Default::default()
                },
                x: None,
                y: None,
            });
        state.config.enabled = enabled;
    }

    pub fn update_config(&self, cursor_id: &str, f: impl FnOnce(&mut CursorInstanceConfig)) {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorInstanceState {
                config: CursorInstanceConfig {
                    cursor_id: cursor_id.to_owned(),
                    ..Default::default()
                },
                x: None,
                y: None,
            });
        f(&mut state.config);
    }

    pub fn all_states(&self) -> Vec<CursorInstanceState> {
        self.inner.lock().unwrap().values().cloned().collect()
    }

    /// Drop a session's cursor metadata entry (fired from the `session_end`
    /// hook). The `"default"` key backs the anonymous / one-shot path and is
    /// guarded against removal; an empty or absent key is a harmless no-op.
    pub fn remove(&self, cursor_id: &str) {
        if cursor_id.is_empty() || cursor_id == "default" {
            return;
        }
        self.inner.lock().unwrap().remove(cursor_id);
    }
}

impl Default for CursorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifier for one owned cursor in the keyed render collection.
///
/// Resolved by the macOS tool layer (see `resolve_cursor_key`) with the
/// precedence: explicit `cursor_id` arg > injected `_session_id` > `"default"`.
/// The render side treats it as an opaque insertion-ordered map key; the
/// `"default"` key is special-cased (never removed) so the anonymous /
/// one-shot `cua-driver call` path is backward compatible.
pub type CursorKey = String;

/// A render command tagged with the cursor it targets. Wrapping the key
/// here (rather than inside [`OverlayCommand`]) keeps `OverlayCommand` and
/// the shared `apply_command_base` / `render_frame` API untouched, so the
/// Windows and Linux overlays — which never see a key — keep compiling
/// and behaving exactly as before.
#[derive(Debug, Clone)]
pub struct KeyedOverlayCommand {
    pub key: CursorKey,
    pub cmd: OverlayCommand,
}

/// Message carried over a platform overlay channel. Either a keyed render
/// command or an explicit session-lifecycle transition. A separate lifecycle
/// enum (rather than `OverlayCommand` variants) keeps render commands
/// render-only.
#[derive(Debug, Clone)]
pub enum OverlayMsg {
    Cmd(KeyedOverlayCommand),
    Remove(CursorKey),
    /// Clear the render-side tombstone for an explicitly revived session.
    /// This deliberately does not recreate a cursor; the next command does so
    /// lazily after the successful `start_session` boundary.
    Revive(CursorKey),
}

/// Commands sent from MCP tool handlers to the overlay's render thread.
#[derive(Debug, Clone)]
pub enum OverlayCommand {
    /// Animate the cursor to a new screen position.
    MoveTo {
        x: f64,
        y: f64,
        end_heading_radians: f64,
    },
    /// Snap the cursor immediately to a screen position, optionally updating heading.
    SnapTo {
        x: f64,
        y: f64,
        heading_radians: Option<f64>,
    },
    /// Start the click-press visual.
    ClickPulse { x: f64, y: f64 },
    /// Toggle the held-button visual state.
    SetPressed(bool),
    /// Show or hide the overlay.
    SetEnabled(bool),
    /// Update the motion/timing config live.
    SetMotion(MotionConfig),
    /// Pin the overlay above a specific window (by platform window id).
    PinAbove(u64),
    /// Begin a best-effort semantic cursor cue.
    BeginAction {
        action: CursorAction,
        delivery: Option<DeliveryModifier>,
        target: Option<TargetModifier>,
    },
    /// End a held or looping cue if it still owns the visual state.
    EndAction(CursorAction),
    /// Select an already-installed cursor theme for this cursor instance.
    SetTheme {
        theme_id: String,
        reduced_motion: ReducedMotion,
    },
    /// Set the sanitized public label shown beneath this cursor.
    SetSessionLabel(String),
    /// Show a focus-highlight rectangle around an AX-targeted element.
    /// `[x, y, width, height]` in screen coordinates (top-left origin).
    /// `None` clears the highlight.
    ShowFocusRect(Option<[f64; 4]>),
}

/// Build the shared overlay command for one native pointer position.
///
/// Native drag implementations report the actual event coordinate while the
/// cursor artwork is centred 16 points down-right so its tip lands on that
/// coordinate. Keeping this transform here prevents platform-specific drag
/// loops from drifting apart.
pub fn track_pointer_command(x: f64, y: f64) -> OverlayCommand {
    const CLICK_OFFSET: f64 = 16.0;
    let heading = std::f64::consts::FRAC_PI_4;
    OverlayCommand::SnapTo {
        x: x + heading.cos() * CLICK_OFFSET,
        y: y + heading.sin() * CLICK_OFFSET,
        heading_radians: Some(heading),
    }
}

#[cfg(test)]
mod pointer_tracking_tests {
    use super::*;

    #[test]
    fn tracked_artwork_keeps_its_tip_on_the_native_pointer() {
        let OverlayCommand::SnapTo {
            x,
            y,
            heading_radians: Some(heading),
        } = track_pointer_command(120.0, 80.0)
        else {
            panic!("pointer tracking must produce an anchored snap");
        };
        assert!((x - (120.0 + heading.cos() * 16.0)).abs() < f64::EPSILON);
        assert!((y - (80.0 + heading.sin() * 16.0)).abs() < f64::EPSILON);
    }
}
