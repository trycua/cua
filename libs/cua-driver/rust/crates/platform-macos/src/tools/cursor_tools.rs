//! Agent cursor control tools: set_agent_cursor_enabled, set_agent_cursor_motion,
//! get_agent_cursor_state.
//!
//! Extended with multi-cursor customization fields matching the customer use case
//! where codex wrapper wants control over the cursor icon.

use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

/// Resolve the cursor key for a tool invocation.
///
/// Precedence (mandatory): an explicit, non-empty `cursor_id` arg wins, then
/// the daemon-injected `_session_id` (so each MCP session owns a cursor by
/// default), then the seeded `"default"` cursor (anonymous / one-shot
/// `cua-driver call`). Putting `cursor_id` first means a wrapper that
/// deliberately shares one cursor_id across sessions is NOT fragmented.
pub(crate) fn resolve_cursor_key(args: &Value) -> String {
    use cua_driver_core::tool_args::ArgsExt;
    if let Some(explicit) = args.opt_str("cursor_id") {
        if !explicit.is_empty() {
            return explicit;
        }
    }
    if let Some(session) = args.opt_str("_session_id") {
        if !session.is_empty() {
            return session;
        }
    }
    "default".to_owned()
}

// ── SetAgentCursorEnabled ─────────────────────────────────────────────────────

pub struct SetAgentCursorEnabledTool {
    state: Arc<ToolState>,
}

impl SetAgentCursorEnabledTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static ENABLED_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn enabled_def() -> &'static ToolDef {
    ENABLED_DEF.get_or_init(|| ToolDef {
        name: "set_agent_cursor_enabled".into(),
        description: "Show or hide the agent cursor overlay for a cursor instance. The overlay \
                      is ON by default and each MCP session automatically owns its own cursor \
                      (keyed by session id) — you do NOT need to call this to make the cursor \
                      appear. Use enabled=false to hide it, or enabled=true to re-show a hidden \
                      one. Pass cursor_id only to target a deliberately-shared cursor.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["enabled"],
            "properties": {
                "enabled": { "type": "boolean", "description": "true = show, false = hide." },
                "cursor_id": { "type": "string", "description": "Cursor instance. Default: 'default'." }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for SetAgentCursorEnabledTool {
    fn def(&self) -> &ToolDef { enabled_def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let enabled = match args.require_bool("enabled") { Ok(v) => v, Err(e) => return e };
        let key = resolve_cursor_key(&args);
        self.state.cursor_registry.set_enabled(&key, enabled);
        // Drive the visual overlay for THIS session's cursor.
        crate::cursor::overlay::send_command(
            key.clone(),
            cursor_overlay::OverlayCommand::SetEnabled(enabled),
        );
        ToolResult::text(format!("Agent cursor '{}' {}.", key, if enabled { "enabled" } else { "disabled" }))
    }
}

// ── SetAgentCursorMotion (config) ─────────────────────────────────────────────

pub struct SetAgentCursorMotionTool {
    state: Arc<ToolState>,
}

impl SetAgentCursorMotionTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static MOTION_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn motion_def() -> &'static ToolDef {
    MOTION_DEF.get_or_init(|| ToolDef {
        name: "set_agent_cursor_motion".into(),
        description: "Configure the visual appearance and motion curve of an agent cursor instance.\n\n\
            Appearance (multi-cursor customization):\n\
            - cursor_id: instance name (default='default')\n\
            - cursor_icon: built-in ('arrow','crosshair','hand','dot') or PNG/SVG file path\n\
            - cursor_color: hex color e.g. '#00FFFF' or CSS name\n\
            - cursor_label: short text shown near the cursor\n\
            - cursor_size: dot radius in points (default=16)\n\
            - cursor_opacity: 0.0–1.0 (default=0.85)\n\n\
            Motion curve (Bezier path shape):\n\
            - start_handle: departure control-point fraction [0,1]. Default 0.3\n\
            - end_handle: arrival control-point fraction [0,1]. Default 0.3\n\
            - arc_size: perpendicular deflection as fraction of path length [0,1]. Default 0.25\n\
            - arc_flow: asymmetry [-1,1]; positive bulges toward destination. Default 0.0\n\
            - spring: settle damping [0.3,1.0]; 1.0=no overshoot. Default 0.72\n\
            - glide_duration_ms: flight duration per move [50,5000]. Default 160\n\
            - dwell_after_click_ms: pause after click ripple [0,5000]. Default 80\n\
            - idle_hide_ms: auto-hide delay [0,60000]; 0=never. Default 20000".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "cursor_id":    { "type": "string", "description": "Cursor instance name. Default: 'default'." },
                "cursor_icon":  { "type": "string", "description": "Built-in icon name or file path to PNG/SVG." },
                "cursor_color": { "type": "string", "description": "Hex color (e.g. '#00FFFF') or CSS color name." },
                "cursor_label": { "type": "string", "description": "Short label near the cursor dot." },
                "cursor_size":  { "type": "number", "description": "Dot radius in points. Default: 16." },
                "cursor_opacity": { "type": "number", "description": "Opacity 0.0–1.0. Default: 0.85." },
                "start_handle": {
                    "type": "number",
                    "description": "Start-handle fraction in [0, 1]. Default 0.3."
                },
                "end_handle": {
                    "type": "number",
                    "description": "End-handle fraction in [0, 1]. Default 0.3."
                },
                "arc_size": {
                    "type": "number",
                    "description": "Arc deflection as fraction of path length [0, 1]. Default 0.25."
                },
                "arc_flow": {
                    "type": "number",
                    "description": "Asymmetry bias in [-1, 1]. Default 0.0."
                },
                "spring": {
                    "type": "number",
                    "description": "Settle damping in [0.3, 1.0]. Default 0.72."
                },
                "glide_duration_ms": {
                    "type": "number",
                    "minimum": 50,
                    "maximum": 5000,
                    "description": "Flight duration per move in ms. Default 160."
                },
                "dwell_after_click_ms": {
                    "type": "number",
                    "minimum": 0,
                    "maximum": 5000,
                    "description": "Pause after click ripple in ms. Default 80."
                },
                "idle_hide_ms": {
                    "type": "number",
                    "minimum": 0,
                    "maximum": 60000,
                    "description": "Auto-hide delay in ms. 0 = never hide. Default 20000."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for SetAgentCursorMotionTool {
    fn def(&self) -> &ToolDef { motion_def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_id = resolve_cursor_key(&args);

        // Start from the current state or defaults.
        let mut current = self.state.cursor_registry.get_or_create(&cursor_id);
        let config = &mut current.config;

        // ── Appearance fields ────────────────────────────────────────────────
        if let Some(icon) = args.opt_str("cursor_icon") {
            config.cursor_icon = Some(icon);
        }
        if let Some(color) = args.opt_str("cursor_color") {
            config.cursor_color = Some(color);
        }
        if let Some(label) = args.opt_str("cursor_label") {
            config.cursor_label = Some(label);
        }
        if let Some(size) = args.opt_f64("cursor_size") {
            config.cursor_size = Some(size);
        }
        if let Some(opacity) = args.opt_f64("cursor_opacity") {
            config.cursor_opacity = Some(opacity.clamp(0.0, 1.0));
        }

        self.state.cursor_registry.update_config(config.clone());

        // ── Motion curve fields (apply to shared overlay MotionConfig) ───────
        // JSON integers from MCP decode as i64; coerce to f64 here.
        fn num(v: Option<&Value>) -> Option<f64> {
            v.and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
        }
        let motion_changed = args.get("start_handle").is_some()
            || args.get("end_handle").is_some()
            || args.get("arc_size").is_some()
            || args.get("arc_flow").is_some()
            || args.get("spring").is_some()
            || args.get("glide_duration_ms").is_some()
            || args.get("dwell_after_click_ms").is_some()
            || args.get("idle_hide_ms").is_some();

        if motion_changed {
            // Read this cursor's current motion from the overlay, apply
            // overrides, push back.
            let current_motion = crate::cursor::overlay::current_motion(&cursor_id);
            let new_motion = current_motion.with_overrides(
                num(args.get("start_handle")),
                num(args.get("end_handle")),
                num(args.get("arc_size")),
                num(args.get("arc_flow")),
                num(args.get("spring")),
                num(args.get("glide_duration_ms")),
                num(args.get("dwell_after_click_ms")),
                num(args.get("idle_hide_ms")),
                None, // press_duration_ms not exposed
            );
            crate::cursor::overlay::send_command(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetMotion(new_motion.clone()),
            );
            ToolResult::text(format!(
                "Cursor '{}' updated. Motion: start={:.2} end={:.2} arc={:.2} flow={:.2} \
                 spring={:.2} glide={}ms dwell={}ms idle={}ms",
                cursor_id,
                new_motion.start_handle, new_motion.end_handle,
                new_motion.arc_size, new_motion.arc_flow, new_motion.spring,
                new_motion.glide_duration_ms as u32,
                new_motion.dwell_after_click_ms as u32,
                new_motion.idle_hide_ms as u32,
            )).with_structured(serde_json::to_value(&current.config).unwrap_or_default())
        } else {
            ToolResult::text(format!("Cursor '{}' config updated.", cursor_id))
                .with_structured(serde_json::to_value(&current.config).unwrap_or_default())
        }
    }
}

// ── SetAgentCursorStyle ───────────────────────────────────────────────────────

pub struct SetAgentCursorStyleTool {
    state: Arc<ToolState>,
}

impl SetAgentCursorStyleTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static STYLE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn style_def() -> &'static ToolDef {
    STYLE_DEF.get_or_init(|| ToolDef {
        name: "set_agent_cursor_style".into(),
        description:
            "Update the visual style of the agent cursor overlay.\n\n\
             - gradient_colors: array of CSS hex strings (e.g. [\"#FF0000\",\"#0000FF\"]) \
               used as the arrow fill gradient from tip to tail. Empty array reverts to \
               the default palette colours.\n\
             - bloom_color: hex string for the radial halo/bloom behind the cursor \
               (e.g. \"#00FFFF\"). Empty string reverts to the default.\n\
             - image_path: path to a PNG, JPEG, SVG, or ICO file to use as the cursor \
               icon instead of the default gradient arrow. Empty string reverts to the \
               procedural arrow.\n\
             All parameters are optional; omit any you do not want to change."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "cursor_id": {
                    "type": "string",
                    "description": "Cursor instance. Default: 'default'."
                },
                "gradient_colors": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "CSS hex gradient stops tip→tail. [] = revert to default."
                },
                "bloom_color": {
                    "type": "string",
                    "description": "Hex bloom/halo colour (e.g. '#00FFFF'). '' = revert to default."
                },
                "image_path": {
                    "type": "string",
                    "description": "Path to PNG/JPEG/SVG/ICO cursor image. '' = revert to arrow."
                }
            },
            "additionalProperties": false
        }),
        read_only:   false,
        destructive: false,
        idempotent:  true,
        open_world:  false,
    })
}

#[async_trait]
impl Tool for SetAgentCursorStyleTool {
    fn def(&self) -> &ToolDef { style_def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);

        // ── image_path ────────────────────────────────────────────────────────
        let image_path = args.get("image_path").and_then(|v| v.as_str());
        let shape_cmd: Option<cursor_overlay::OverlayCommand> = if let Some(path) = image_path {
            if path.is_empty() {
                // Revert to procedural arrow.
                Some(cursor_overlay::OverlayCommand::SetShape(None))
            } else {
                let path_owned = path.to_owned();
                match tokio::task::spawn_blocking(move || {
                    cursor_overlay::CursorShape::load(&path_owned)
                }).await {
                    Ok(Ok(shape)) => {
                        // Also persist to registry so it's available for state queries.
                        let mut current = self.state.cursor_registry.get_or_create(&cursor_id);
                        current.config.cursor_icon = Some(path.to_owned());
                        self.state.cursor_registry.update_config(current.config);
                        Some(cursor_overlay::OverlayCommand::SetShape(Some(shape)))
                    }
                    Ok(Err(e)) => return ToolResult::error(format!("Failed to load image_path: {e}")),
                    Err(e) => return ToolResult::error(format!("Task error: {e}")),
                }
            }
        } else {
            None
        };

        // ── gradient_colors ───────────────────────────────────────────────────
        let gradient_colors: Vec<[u8; 4]> = if let Some(arr) = args.get("gradient_colors").and_then(|v| v.as_array()) {
            let mut out = vec![];
            for v in arr {
                if let Some(hex) = v.as_str() {
                    match parse_hex_color(hex) {
                        Some(c) => out.push(c),
                        None => return ToolResult::error(format!("Invalid hex color: {hex}")),
                    }
                }
            }
            out
        } else {
            // Not provided — don't change.
            vec![]
        };

        // ── bloom_color ───────────────────────────────────────────────────────
        let bloom_color: Option<Option<[u8; 4]>> = if let Some(hex) = args.get("bloom_color").and_then(|v| v.as_str()) {
            if hex.is_empty() {
                Some(None) // revert
            } else {
                match parse_hex_color(hex) {
                    Some(c) => Some(Some(c)),
                    None => return ToolResult::error(format!("Invalid bloom_color: {hex}")),
                }
            }
        } else {
            None
        };

        // ── Dispatch to overlay (keyed to this session's cursor) ──────────────
        if let Some(cmd) = shape_cmd {
            crate::cursor::overlay::send_command(cursor_id.clone(), cmd);
        }

        let gradient_provided = args.get("gradient_colors").is_some();
        let bloom_provided = args.get("bloom_color").is_some();
        if gradient_provided || bloom_provided {
            crate::cursor::overlay::send_command(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetGradient {
                    gradient_colors,
                    bloom_color: bloom_color.flatten(),
                },
            );
        }

        // Build response summary.
        let grad_str = args.get("gradient_colors")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let strs: Vec<String> = arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                format!("[{}]", strs.join(", "))
            })
            .unwrap_or_else(|| "(unchanged)".into());

        let bloom_str = args.get("bloom_color")
            .and_then(|v| v.as_str())
            .map(|s| if s.is_empty() { "(reverted)".to_owned() } else { s.to_owned() })
            .unwrap_or_else(|| "(unchanged)".into());

        let img_str = image_path
            .map(|s| if s.is_empty() { "(reverted to arrow)".to_owned() } else { s.to_owned() })
            .unwrap_or_else(|| "(unchanged)".into());

        ToolResult::text(format!(
            "✅ cursor style: gradient_colors={grad_str} bloom_color={bloom_str} image_path={img_str}"
        ))
    }
}

/// Parse `#RRGGBB` or `#RGB` hex string to `[R, G, B, A=255]`.
fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
    let s = hex.trim_start_matches('#');
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some([r, g, b, 255])
        }
        3 => {
            let r = u8::from_str_radix(&s[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&s[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&s[2..3].repeat(2), 16).ok()?;
            Some([r, g, b, 255])
        }
        _ => None,
    }
}

// ── GetAgentCursorState ───────────────────────────────────────────────────────

pub struct GetAgentCursorStateTool {
    state: Arc<ToolState>,
}

impl GetAgentCursorStateTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static STATE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn state_def() -> &'static ToolDef {
    STATE_DEF.get_or_init(|| ToolDef {
        name: "get_agent_cursor_state".into(),
        description: "Return the current state of THIS session's agent cursor: position, \
            config (color, icon, label, size, opacity), enabled flag. Pass cursor_id to \
            inspect a specific instance.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "cursor_id": { "type": "string", "description": "Cursor instance. Default: this session's cursor." }
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for GetAgentCursorStateTool {
    fn def(&self) -> &ToolDef { state_def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Scope to the CALLER's cursor (explicit cursor_id > injected
        // _session_id > "default"). Returning every session's cursors here was a
        // cross-session leak, and deriving the top-level `enabled` via
        // `.first()` over a HashMap-backed Vec was nondeterministic with N
        // cursors. A non-creating `get` keeps a never-touched session from
        // materialising a phantom entry.
        let key = resolve_cursor_key(&args);
        let state = self.state.cursor_registry.get(&key);
        // `enabled` defaults to true when the session has no cursor yet — the
        // anonymous / one-shot path resolves to "default", which is always
        // present, so this default only applies to a brand-new session that has
        // not enabled/moved its cursor, where "visible by default" is correct.
        let enabled = state.as_ref().map(|s| s.config.enabled).unwrap_or(true);
        let cursors: Vec<&crate::cursor::CursorState> = state.iter().collect();
        let json = serde_json::to_value(&cursors).unwrap_or_default();
        ToolResult::text(format!("{} cursor instance(s) for '{}'.", cursors.len(), key))
            .with_structured(serde_json::json!({ "cursors": json, "enabled": enabled }))
    }
}

// ── Headless unit tests for cursor-key resolution ─────────────────────────

#[cfg(test)]
mod tests {
    use super::resolve_cursor_key;
    use serde_json::json;

    #[test]
    fn anonymous_resolves_to_default() {
        // No cursor_id, no _session_id → seeded "default" cursor (backward
        // compatible one-shot `cua-driver call`).
        assert_eq!(resolve_cursor_key(&json!({})), "default");
        assert_eq!(resolve_cursor_key(&json!({ "x": 1 })), "default");
    }

    #[test]
    fn session_id_owns_a_cursor_by_default() {
        assert_eq!(
            resolve_cursor_key(&json!({ "_session_id": "sess-7" })),
            "sess-7"
        );
    }

    #[test]
    fn explicit_cursor_id_wins_over_session() {
        // Precedence: explicit cursor_id > injected _session_id > "default".
        assert_eq!(
            resolve_cursor_key(&json!({ "cursor_id": "user-handle", "_session_id": "sess-7" })),
            "user-handle"
        );
    }

    #[test]
    fn get_agent_cursor_state_is_session_scoped() {
        // A fabricated registry with two sessions' cursors; the scoped read for
        // one session must return ONLY that session's cursor (no cross-session
        // leak) and a deterministic `enabled` for that key.
        use crate::cursor::CursorRegistry;
        let reg = CursorRegistry::new();
        reg.set_enabled("sessA", true);
        reg.set_enabled("sessB", false);

        let a = reg.get("sessA").expect("sessA present");
        assert!(a.config.enabled);
        let b = reg.get("sessB").expect("sessB present");
        assert!(!b.config.enabled);
        // Scoping to sessB must not see sessA's enabled flag.
        assert_ne!(a.config.enabled, b.config.enabled);
        // A never-touched session has no entry — no phantom materialised.
        assert!(reg.get("sessC-never-touched").is_none());
    }

    #[test]
    fn enable_and_ax_click_resolve_the_same_session_cursor() {
        // BUG verify (b): in one MCP session, set_agent_cursor_enabled (no
        // cursor_id) and a click(element_index) carry the SAME injected
        // _session_id, so both resolve the same cursor key — i.e. enabling the
        // cursor lights the very cursor the AX click drives. Mirrors the args
        // the daemon injects (_session_id) for each forwarded tool call.
        let session = "mcp-12345-678";
        let enable_args = json!({ "enabled": true, "_session_id": session });
        let ax_click_args =
            json!({ "pid": 844, "window_id": 10725, "element_index": 14, "_session_id": session });
        let enable_key = resolve_cursor_key(&enable_args);
        let click_key = resolve_cursor_key(&ax_click_args);
        assert_eq!(enable_key, session);
        assert_eq!(enable_key, click_key,
            "set_agent_cursor_enabled and the AX click must drive the same session cursor");
    }

    #[test]
    fn get_config_reports_calling_session_cursor_deterministically() {
        // BUG 3 regression: get_config's cursor_enabled must reflect the CALLING
        // session's own cursor (resolved by key), not a nondeterministic
        // HashMap.first(). Two sessions with opposite enabled flags must each
        // read back their OWN value.
        use crate::cursor::CursorRegistry;
        let reg = CursorRegistry::new();
        reg.set_enabled("sessA", true);
        reg.set_enabled("sessB", false);

        // Replicate get_config's resolution: key = resolve_cursor_key(args),
        // then get(key) or get("default").
        let read_for = |args: &serde_json::Value| -> bool {
            let key = resolve_cursor_key(args);
            reg.get(&key)
                .or_else(|| reg.get("default"))
                .map(|s| s.config.enabled)
                .unwrap_or(true)
        };
        assert!(read_for(&json!({ "_session_id": "sessA" })));
        assert!(!read_for(&json!({ "_session_id": "sessB" })));
        // Anonymous caller (no session) falls back to the seeded default (on).
        assert!(read_for(&json!({})));
    }

    #[test]
    fn empty_strings_fall_through() {
        // An empty cursor_id falls through to _session_id; empty session falls
        // through to "default".
        assert_eq!(
            resolve_cursor_key(&json!({ "cursor_id": "", "_session_id": "sess-7" })),
            "sess-7"
        );
        assert_eq!(
            resolve_cursor_key(&json!({ "cursor_id": "", "_session_id": "" })),
            "default"
        );
    }
}
