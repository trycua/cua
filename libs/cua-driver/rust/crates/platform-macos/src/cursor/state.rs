//! Cursor configuration and runtime state.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-instance cursor configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorConfig {
    /// Instance identifier for multi-cursor use cases. Default: "default".
    pub cursor_id: String,
    /// Built-in icon name ("arrow", "crosshair", "hand") or path to PNG/SVG.
    pub cursor_icon: Option<String>,
    /// Hex color (e.g. "#00FFFF") or CSS name for the glow/indicator.
    pub cursor_color: Option<String>,
    /// Short label rendered near the cursor dot.
    pub cursor_label: Option<String>,
    /// Dot radius in points. Default: 16.
    pub cursor_size: Option<f64>,
    /// Opacity 0.0–1.0. Default: 0.85.
    pub cursor_opacity: Option<f64>,
    /// Whether the overlay is currently visible.
    pub enabled: bool,
}

impl Default for CursorConfig {
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

/// Runtime position of the agent cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPosition {
    pub x: f64,
    pub y: f64,
}

/// Full state for a cursor instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorState {
    pub config: CursorConfig,
    pub position: Option<CursorPosition>,
}

/// Global registry of cursor instances, keyed by `cursor_id`.
pub struct CursorRegistry {
    inner: Mutex<HashMap<String, CursorState>>,
}

impl CursorRegistry {
    pub fn new() -> Self {
        let mut map = HashMap::new();
        let default = CursorState {
            config: CursorConfig::default(),
            position: None,
        };
        map.insert("default".into(), default);
        Self { inner: Mutex::new(map) }
    }

    pub fn get_or_create(&self, cursor_id: &str) -> CursorState {
        let mut inner = self.inner.lock().unwrap();
        inner.entry(cursor_id.to_owned()).or_insert_with(|| CursorState {
            config: CursorConfig { cursor_id: cursor_id.to_owned(), ..Default::default() },
            position: None,
        }).clone()
    }

    pub fn update_config(&self, config: CursorConfig) {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner.entry(config.cursor_id.clone()).or_insert_with(|| CursorState {
            config: config.clone(),
            position: None,
        });
        entry.config = config;
    }

    pub fn update_position(&self, cursor_id: &str, x: f64, y: f64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(state) = inner.get_mut(cursor_id) {
            state.position = Some(CursorPosition { x, y });
        }
    }

    pub fn set_enabled(&self, cursor_id: &str, enabled: bool) {
        let mut inner = self.inner.lock().unwrap();
        let state = inner.entry(cursor_id.to_owned()).or_insert_with(|| CursorState {
            config: CursorConfig { cursor_id: cursor_id.to_owned(), ..Default::default() },
            position: None,
        });
        state.config.enabled = enabled;
    }

    pub fn all_states(&self) -> Vec<CursorState> {
        self.inner.lock().unwrap().values().cloned().collect()
    }
}

impl Default for CursorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
