//! cursor-overlay — shared types and math for the cua-driver cursor overlay.
//!
//! Platform renderers (macOS, Windows, Linux) depend on this crate for:
//! - `CursorConfig` — theme, accessibility, visibility, and motion settings
//! - `MotionConfig` — glide duration, spring, dwell, idle-hide timings
//! - `CubicBezier` + `PathPlanner` — Bezier path math (ported 1:1 from C#)
//! - `OverlayCommand` — messages sent from MCP tools to the overlay thread
//! - `SurfaceFit` — keeps each platform's overlay surface fitted to the live
//!   display geometry, so screen-coordinate cursors stay on the pointer

pub mod badge_glyphs;
pub mod bezier;
pub mod capture_exclusion;
pub mod capture_utils;
pub mod motion;
pub mod path_planner;
pub mod render_map;
pub mod render_state;
pub mod session_badge;
pub mod surface_fit;
pub mod theme;
pub mod theme_artifact;
pub mod z_order;

pub use badge_glyphs::{BadgeChip, BadgeGlyph};
pub use bezier::CubicBezier;
pub use motion::{MotionConfig, Spring};
pub use path_planner::{PathPlanner, PathState, PlannedPath};
pub use render_map::{
    keyed_config, seed_position, CursorMap, MsgOutcome, RenderEntry, RenderMap, ScreenFrame,
    DEFAULT_CURSOR_KEY, SEED_OFFSET,
};
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
pub use surface_fit::{SurfaceFit, SurfaceGeometry, SURFACE_REFIT_INTERVAL};
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
    /// No state change: wakes a parked render loop so it services an
    /// out-of-band request (such as hiding for a Driver desktop capture)
    /// without waiting for its next maintenance tick.
    Wake,
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

/// Distance, in points, between a cursor's pointer point and its anchor.
///
/// `RenderStateCore::pos` is the anchor that path motion, the session badge,
/// and platform damage regions follow. The theme hotspot is drawn at the
/// pointer point, `POINTER_ANCHOR_OFFSET` points from the anchor opposite the
/// heading, so a cursor anchored by [`anchor_for_pointer`] draws its tip on
/// the requested coordinate at every heading and backing scale.
pub const POINTER_ANCHOR_OFFSET: f64 = 16.0;

/// Anchor that places a cursor's hotspot on `(x, y)` at `heading`.
pub fn anchor_for_pointer(x: f64, y: f64, heading: f64) -> (f64, f64) {
    (
        x + heading.cos() * POINTER_ANCHOR_OFFSET,
        y + heading.sin() * POINTER_ANCHOR_OFFSET,
    )
}

/// Pointer point, where the theme hotspot is drawn, for an anchor at `heading`.
pub fn pointer_for_anchor(x: f64, y: f64, heading: f64) -> (f64, f64) {
    (
        x - heading.cos() * POINTER_ANCHOR_OFFSET,
        y - heading.sin() * POINTER_ANCHOR_OFFSET,
    )
}

/// Cursor key for a named session's keyboard and text feedback: the explicit
/// `session` label, else the trusted lifecycle `_session_id`. Anonymous calls
/// return `None` and keep their existing behavior.
pub fn named_session_cursor_key(args: &serde_json::Value) -> Option<String> {
    ["session", "_session_id"].into_iter().find_map(|key| {
        args.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|session| !session.is_empty())
            .map(str::to_owned)
    })
}

/// Where a keyboard or text action places its session cursor: the explicit
/// element or pixel target, else the cursor's remembered position, else the
/// target window's centre, else the current pointer. Non-finite points are
/// skipped so a bad geometry read cannot poison position reuse.
pub fn keyboard_cursor_target(
    explicit: Option<(f64, f64)>,
    remembered: Option<(f64, f64)>,
    window_center: Option<(f64, f64)>,
    current_pointer: Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    let finite = |point: Option<(f64, f64)>| point.filter(|(x, y)| x.is_finite() && y.is_finite());
    finite(explicit)
        .or_else(|| finite(remembered))
        .or_else(|| finite(window_center))
        .or_else(|| finite(current_pointer))
}

/// Build the shared overlay command for one native pointer position.
///
/// Native drag implementations report the actual event coordinate. Anchoring
/// it here keeps the theme hotspot on that coordinate and prevents
/// platform-specific drag loops from drifting apart.
pub fn track_pointer_command(x: f64, y: f64) -> OverlayCommand {
    let heading = std::f64::consts::FRAC_PI_4;
    let (x, y) = anchor_for_pointer(x, y, heading);
    OverlayCommand::SnapTo {
        x,
        y,
        heading_radians: Some(heading),
    }
}

/// Balance one cursor's visual press even if its action future is dropped.
///
/// The adapter binds `send` to its own cursor. This only resets artwork; it
/// does not release native input, cancel a worker, or prove gesture cleanup.
#[must_use = "keep the guard alive for the visual press interval"]
pub struct PressedVisualGuard<F: Fn(OverlayCommand)> {
    send: F,
}

impl<F: Fn(OverlayCommand)> PressedVisualGuard<F> {
    pub fn new(send: F) -> Self {
        send(OverlayCommand::SetPressed(true));
        Self { send }
    }
}

impl<F: Fn(OverlayCommand)> Drop for PressedVisualGuard<F> {
    fn drop(&mut self) {
        (self.send)(OverlayCommand::SetPressed(false));
    }
}

#[cfg(test)]
mod pointer_tracking_tests {
    use super::*;

    #[test]
    fn visual_press_guard_releases_only_its_bound_cursor() {
        use std::cell::Cell;
        let first = Cell::new(false);
        let sibling = Cell::new(false);
        let send = |state: &Cell<bool>, command| {
            let OverlayCommand::SetPressed(pressed) = command else {
                panic!("visual press guard must only change pressed artwork");
            };
            state.set(pressed);
        };
        let first_guard = PressedVisualGuard::new(|command| send(&first, command));
        let sibling_guard = PressedVisualGuard::new(|command| send(&sibling, command));
        assert!(first.get() && sibling.get());
        drop(first_guard);
        assert!(!first.get());
        assert!(sibling.get());
        drop(sibling_guard);
        assert!(!sibling.get());
    }

    #[test]
    fn named_sessions_opt_into_keyboard_cursor_positioning() {
        use serde_json::json;
        assert_eq!(
            named_session_cursor_key(&json!({"session": "editing-run"})).as_deref(),
            Some("editing-run")
        );
        assert_eq!(
            named_session_cursor_key(&json!({"_session_id": "implicit"})).as_deref(),
            Some("implicit")
        );
        assert_eq!(
            named_session_cursor_key(&json!({"cursor_id": "legacy"})),
            None
        );
        assert_eq!(named_session_cursor_key(&json!({"session": ""})), None);
    }

    #[test]
    fn keyboard_cursor_uses_explicit_then_remembered_then_window_then_pointer() {
        let explicit = Some((10.0, 20.0));
        let remembered = Some((30.0, 40.0));
        let window = Some((50.0, 60.0));
        let pointer = Some((70.0, 80.0));
        assert_eq!(
            keyboard_cursor_target(explicit, remembered, window, pointer),
            explicit
        );
        assert_eq!(
            keyboard_cursor_target(None, remembered, window, pointer),
            remembered
        );
        assert_eq!(keyboard_cursor_target(None, None, window, pointer), window);
        assert_eq!(keyboard_cursor_target(None, None, None, pointer), pointer);
        assert_eq!(keyboard_cursor_target(None, None, None, None), None);
        assert_eq!(
            keyboard_cursor_target(Some((f64::NAN, 1.0)), Some((12.0, 34.0)), None, None),
            Some((12.0, 34.0)),
            "invalid coordinates must not poison session position reuse"
        );
    }

    #[test]
    fn session_cleanup_removes_named_cursor_but_preserves_anonymous_default() {
        let registry = CursorRegistry::new();
        registry.update_position("session-a", 12.0, 34.0);
        registry.update_position("default", 56.0, 78.0);

        registry.remove("session-a");
        registry.remove("default");

        assert!(registry.get("session-a").is_none());
        assert_eq!(
            registry
                .get("default")
                .and_then(|cursor| cursor.x.zip(cursor.y)),
            Some((56.0, 78.0))
        );
    }
}
