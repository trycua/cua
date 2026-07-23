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
        let default_cfg = CursorConfig {
            // Default cursor starts DISABLED. The anonymous / non-session
            // path must never render a cursor — the documented contract is
            // "without a session, actions run cursor-less." A session's
            // first enable/start_session call activates its own cursor.
            // See #1777.
            enabled: false,
            ..CursorConfig::default()
        };
        let default = CursorState {
            config: default_cfg,
            position: None,
        };
        map.insert("default".into(), default);
        Self {
            inner: Mutex::new(map),
        }
    }

    pub fn get_or_create(&self, cursor_id: &str) -> CursorState {
        let mut inner = self.inner.lock().unwrap();
        // Write-boundary resurrection guard: a per-session cursor keys on the
        // session_id, so an in-flight call that lands AFTER session_end (a slow
        // AX click that passed the dispatch gate, then the proxy died and the
        // reaper removed this cursor) must NOT re-create the entry the reaper
        // just cleared — it would be invisible and never reaped again.
        // `fire_session_end` marks ENDED_SESSIONS *before* fanning out to the
        // remove hook, so this check is authoritative. The seeded "default" key
        // and any explicit non-session cursor_id are never in ENDED_SESSIONS, so
        // they pass through unchanged. Return the live entry if one survives,
        // else a transient default so callers get a sane value without a write.
        if cua_driver_core::session::is_session_ended(cursor_id) {
            return inner
                .get(cursor_id)
                .cloned()
                .unwrap_or_else(|| CursorState {
                    config: CursorConfig {
                        cursor_id: cursor_id.to_owned(),
                        ..Default::default()
                    },
                    position: None,
                });
        }
        inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorState {
                config: CursorConfig {
                    cursor_id: cursor_id.to_owned(),
                    ..Default::default()
                },
                position: None,
            })
            .clone()
    }

    /// Non-creating read of a single cursor's state. Returns None when the id
    /// has no entry (used by get_agent_cursor_state to scope its result to the
    /// caller's session without materialising a phantom cursor).
    pub fn get(&self, cursor_id: &str) -> Option<CursorState> {
        self.inner.lock().unwrap().get(cursor_id).cloned()
    }

    pub fn update_config(&self, config: CursorConfig) {
        // Write-boundary resurrection guard (see get_or_create): no-op for an
        // ended session id so a post-session_end set_config can't resurrect the
        // cursor metadata. "default" / non-session ids are never ended.
        if config.cursor_id.is_empty()
            || cua_driver_core::session::is_session_ended(&config.cursor_id)
        {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        let entry = inner
            .entry(config.cursor_id.clone())
            .or_insert_with(|| CursorState {
                config: config.clone(),
                position: None,
            });
        entry.config = config;
    }

    pub fn update_position(&self, cursor_id: &str, x: f64, y: f64) {
        // Write-boundary resurrection guard (see get_or_create): no-op for an
        // ended session id so an in-flight move after session_end can't
        // re-create the cleared cursor. "default" / non-session ids never ended.
        if cursor_id.is_empty() || cua_driver_core::session::is_session_ended(cursor_id) {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        // Get-or-create so a per-session cursor that moves before any explicit
        // enable/style call still shows up in get_agent_cursor_state.
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorState {
                config: CursorConfig {
                    cursor_id: cursor_id.to_owned(),
                    ..Default::default()
                },
                position: None,
            });
        state.position = Some(CursorPosition { x, y });
    }

    pub fn set_enabled(&self, cursor_id: &str, enabled: bool) {
        // Write-boundary resurrection guard (see get_or_create): no-op for an
        // ended session id so a post-session_end enable/disable can't resurrect
        // the cleared cursor. "default" / non-session ids are never ended.
        if cursor_id.is_empty() || cua_driver_core::session::is_session_ended(cursor_id) {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorState {
                config: CursorConfig {
                    cursor_id: cursor_id.to_owned(),
                    ..Default::default()
                },
                position: None,
            });
        state.config.enabled = enabled;
    }

    pub fn all_states(&self) -> Vec<CursorState> {
        self.inner.lock().unwrap().values().cloned().collect()
    }

    /// Remove a cursor instance's metadata (fired from the `session_end` hook
    /// when a session disconnects). The `"default"` cursor backs the
    /// anonymous / one-shot path and is never removed — guarding here AND in
    /// the overlay Remove arm keeps the backward-compatibility contract intact.
    pub fn remove(&self, cursor_id: &str) {
        if cursor_id == "default" {
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

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::session::fire_session_end;

    // Each test uses a unique session id so the process-global ENDED_SESSIONS
    // set never collides across tests running in the same process.

    #[test]
    fn ended_session_id_is_not_resurrected_by_mutators() {
        // THE FIX: mark a session ended (as fire_session_end does before the
        // reaper's remove hook runs), then drive every guarded mutator with that
        // session id. None may create an entry — proving the in-flight write
        // that lands after session_end cannot resurrect the cleared cursor.
        let reg = CursorRegistry::new();
        let sid = "wb-cursor-ended-A1B2C3";
        fire_session_end(sid);
        assert!(cua_driver_core::session::is_session_ended(sid));

        reg.set_enabled(sid, true);
        reg.update_position(sid, 10.0, 20.0);
        reg.update_config(CursorConfig {
            cursor_id: sid.to_owned(),
            ..Default::default()
        });
        let got = reg.get_or_create(sid);

        // get_or_create returns a transient default value, but must NOT persist.
        assert_eq!(got.config.cursor_id, sid);
        assert!(
            reg.get(sid).is_none(),
            "no entry may be created for an ended session"
        );
        assert!(
            !reg.all_states().iter().any(|s| s.config.cursor_id == sid),
            "ended session cursor must never appear in the registry"
        );
    }

    #[test]
    fn default_cursor_starts_disabled() {
        // Default cursor is seeded disabled — anonymous calls must be
        // cursor-less per the documented contract. See #1777.
        let reg = CursorRegistry::new();
        assert!(!cua_driver_core::session::is_session_ended("default"));
        let s = reg.get("default").expect("default cursor always present");
        assert!(!s.config.enabled, "default cursor must start disabled");
        // But mutators still work — a session that enables it should succeed.
        reg.set_enabled("default", true);
        let s2 = reg.get("default").unwrap();
        assert!(s2.config.enabled);
    }

    #[test]
    fn explicit_non_session_cursor_id_unaffected_by_guard() {
        // A user-supplied cursor_id that is not a session id is never in
        // ENDED_SESSIONS, so the plain is_session_ended check leaves it working.
        let reg = CursorRegistry::new();
        let cid = "wb-explicit-user-cursor-XYZ";
        assert!(!cua_driver_core::session::is_session_ended(cid));
        reg.set_enabled(cid, true);
        reg.update_position(cid, 5.0, 6.0);
        let s = reg.get(cid).expect("explicit cursor_id must materialise");
        assert!(s.config.enabled);
        assert_eq!(s.position.as_ref().map(|p| (p.x, p.y)), Some((5.0, 6.0)));
    }

    #[test]
    fn live_session_cursor_writes_still_take_effect() {
        // A live (not-ended) session id writes normally.
        let reg = CursorRegistry::new();
        let sid = "wb-cursor-live-LMNOP";
        assert!(!cua_driver_core::session::is_session_ended(sid));
        reg.set_enabled(sid, true);
        reg.update_position(sid, 7.0, 8.0);
        let s = reg.get(sid).expect("live session cursor must exist");
        assert!(s.config.enabled);
        assert_eq!(s.position.as_ref().map(|p| (p.x, p.y)), Some((7.0, 8.0)));
    }
}
