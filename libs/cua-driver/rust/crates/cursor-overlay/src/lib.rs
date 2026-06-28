//! cursor-overlay — shared types and math for the cua-driver cursor overlay.
//!
//! Platform renderers (macOS, Windows, Linux) depend on this crate for:
//! - `CursorConfig` — parsed from CLI args (`--cursor-icon`, `--cursor-id`, etc.)
//! - `Palette` — 9 named colour palettes matching the reference implementations
//! - `MotionConfig` — glide duration, spring, dwell, idle-hide timings
//! - `CubicBezier` + `PathPlanner` — Bezier path math (ported 1:1 from C#)
//! - `CursorShape` — loaded and rasterised custom SVG / ICO / PNG asset
//! - `OverlayCommand` — messages sent from MCP tools to the overlay thread

pub mod palette;
pub mod motion;
pub mod bezier;
pub mod path_planner;
pub mod shape;
pub mod capture_utils;
pub mod util;
pub mod render_state;
pub mod z_order;

pub use palette::Palette;
pub use motion::{MotionConfig, Spring};
pub use bezier::CubicBezier;
pub use path_planner::{PathPlanner, PlannedPath, PathState};
pub use shape::{resolve_cursor_icon, BuiltinShape, CursorShape};
pub use render_state::{RenderStateCore, FocusRect, render_frame, paint_cursor, draw_default_arrow};
pub use z_order::ZOrderEnforcer;

/// Configuration assembled from CLI arguments and passed to every
/// platform backend when it initialises the overlay window.
#[derive(Debug, Clone)]
pub struct CursorConfig {
    /// Multi-cursor instance identifier; affects palette selection.
    /// Defaults to `"default"`.
    pub cursor_id: String,

    /// Custom cursor shape loaded from `--cursor-icon <path>`. Takes
    /// precedence over `builtin_shape` when set. `None` means use the
    /// built-in selected by `builtin_shape`.
    pub shape: Option<CursorShape>,

    /// Which built-in silhouette to render when no custom `shape` is set.
    /// Defaults to [`BuiltinShape::Arrow`] (the procedural gradient
    /// diamond) until the embedded teardrop's retina rasterisation is
    /// fully sorted; opt into the teardrop via `--cursor-shape teardrop`.
    pub builtin_shape: BuiltinShape,

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
            shape: None,
            builtin_shape: BuiltinShape::default(),
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
    /// --cursor-icon  <path.svg|path.ico|path.png>
    /// --cursor-id    <id>
    /// --cursor-shape <arrow|teardrop>  (selects a built-in silhouette;
    ///                                   default: arrow)
    /// --cursor-palette <name>     (selects a named Palette)
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
                "--cursor-icon" => {
                    if let Some(path) = args.get(i + 1) {
                        match CursorShape::load(path) {
                            Ok(s) => cfg.shape = Some(s),
                            Err(e) => tracing::warn!("--cursor-icon {path}: {e}"),
                        }
                        i += 1;
                    }
                }
                "--cursor-id" => {
                    if let Some(id) = args.get(i + 1) {
                        cfg.cursor_id = id.clone();
                        i += 1;
                    }
                }
                "--cursor-palette" => {
                    if let Some(name) = args.get(i + 1) {
                        // Palette is resolved inside the platform backend using the id;
                        // store the name as the id so ForInstance logic picks it up.
                        cfg.cursor_id = name.clone();
                        i += 1;
                    }
                }
                "--cursor-shape" => {
                    if let Some(name) = args.get(i + 1) {
                        match BuiltinShape::parse(name) {
                            Some(s) => cfg.builtin_shape = s,
                            None => tracing::warn!(
                                "--cursor-shape {name}: unknown shape (expected arrow|teardrop); falling back to default"
                            ),
                        }
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

    /// Return the `Palette` for this config (by cursor_id).
    pub fn palette(&self) -> Palette {
        Palette::for_instance(&self.cursor_id)
    }
}

// ── Shared cursor instance registry ──────────────────────────────────────────

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-instance cursor configuration (icon, color, label, size, opacity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorInstanceConfig {
    pub cursor_id: String,
    pub cursor_icon: Option<String>,
    pub cursor_color: Option<String>,
    pub cursor_label: Option<String>,
    pub cursor_size: Option<f64>,
    pub cursor_opacity: Option<f64>,
    pub enabled: bool,
}

impl Default for CursorInstanceConfig {
    fn default() -> Self {
        Self {
            cursor_id: "default".into(),
            cursor_icon: None,
            cursor_color: Some("#00FFFF".into()),
            cursor_label: None,
            cursor_size: Some(16.0),
            cursor_opacity: Some(0.85),
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
        map.insert("default".into(), CursorInstanceState {
            config: CursorInstanceConfig::default(),
            x: None,
            y: None,
        });
        Self { inner: Mutex::new(map) }
    }

    pub fn get_or_create(&self, cursor_id: &str) -> CursorInstanceState {
        let mut inner = self.inner.lock().unwrap();
        inner.entry(cursor_id.to_owned()).or_insert_with(|| CursorInstanceState {
            config: CursorInstanceConfig {
                cursor_id: cursor_id.to_owned(), ..Default::default()
            },
            x: None, y: None,
        }).clone()
    }

    pub fn update_position(&self, cursor_id: &str, x: f64, y: f64) {
        let mut inner = self.inner.lock().unwrap();
        let state = inner.entry(cursor_id.to_owned()).or_insert_with(|| CursorInstanceState {
            config: CursorInstanceConfig { cursor_id: cursor_id.to_owned(), ..Default::default() },
            x: None, y: None,
        });
        state.x = Some(x);
        state.y = Some(y);
    }

    pub fn set_enabled(&self, cursor_id: &str, enabled: bool) {
        let mut inner = self.inner.lock().unwrap();
        let state = inner.entry(cursor_id.to_owned()).or_insert_with(|| CursorInstanceState {
            config: CursorInstanceConfig { cursor_id: cursor_id.to_owned(), ..Default::default() },
            x: None, y: None,
        });
        state.config.enabled = enabled;
    }

    pub fn update_config(&self, cursor_id: &str, f: impl FnOnce(&mut CursorInstanceConfig)) {
        let mut inner = self.inner.lock().unwrap();
        let state = inner.entry(cursor_id.to_owned()).or_insert_with(|| CursorInstanceState {
            config: CursorInstanceConfig { cursor_id: cursor_id.to_owned(), ..Default::default() },
            x: None, y: None,
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
    fn default() -> Self { Self::new() }
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

/// Message carried over the macOS overlay channel. Either a keyed render
/// command or a lifecycle removal. A separate lifecycle enum (rather than an
/// `OverlayCommand::Remove` variant) keeps `OverlayCommand` render-only and
/// avoids forcing a no-op arm onto the Windows/Linux match.
#[derive(Debug, Clone)]
pub enum OverlayMsg {
    Cmd(KeyedOverlayCommand),
    Remove(CursorKey),
}

/// Commands sent from MCP tool handlers to the overlay's render thread.
#[derive(Debug, Clone)]
pub enum OverlayCommand {
    /// Animate the cursor to a new screen position.
    MoveTo { x: f64, y: f64, end_heading_radians: f64 },
    /// Snap the cursor immediately to a screen position, optionally updating heading.
    SnapTo { x: f64, y: f64, heading_radians: Option<f64> },
    /// Start the click-press visual.
    ClickPulse { x: f64, y: f64 },
    /// Toggle the held-button visual state.
    SetPressed(bool),
    /// Show or hide the overlay.
    SetEnabled(bool),
    /// Update the motion/timing config live.
    SetMotion(MotionConfig),
    /// Update the palette live.
    SetPalette(Palette),
    /// Pin the overlay above a specific window (by platform window id).
    PinAbove(u64),
    /// Replace the cursor shape at runtime.
    /// `None` reverts to the built-in gradient arrow.
    SetShape(Option<CursorShape>),
    /// Update the gradient/bloom colours used by the default arrow renderer.
    /// `gradient_colors`: ordered list of `#RRGGBB` hex strings.
    /// `bloom_color`: `#RRGGBB` hex string for the radial halo.
    SetGradient {
        gradient_colors: Vec<[u8; 4]>,
        bloom_color: Option<[u8; 4]>,
    },
    /// Show a focus-highlight rectangle around an AX-targeted element.
    /// `[x, y, width, height]` in screen coordinates (top-left origin).
    /// `None` clears the highlight.
    ShowFocusRect(Option<[f64; 4]>),
}
