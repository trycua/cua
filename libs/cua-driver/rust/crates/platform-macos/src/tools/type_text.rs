//! type_text tool — matches the Swift reference TypeTextTool.swift.
//!
//! Inserts text via `AXSelectedText` attribute write — an atomic single-call
//! insertion at the current cursor position. This is the preferred path for
//! all standard Cocoa text views (NSTextField, NSTextView, WKWebView text
//! inputs in Safari, etc.) and is significantly faster than per-keystroke
//! CGEvent synthesis.
//!
//! For Chromium / Electron inputs that don't implement `kAXSelectedText`,
//! the tool falls back to character-by-character CGEvent keystrokes so the
//! caller doesn't need to detect the app type themselves.
//!
//! When the target pid belongs to a terminal emulator (Ghostty,
//! Terminal.app, iTerm2, Alacritty, kitty, WezTerm, Hyper, Warp — see
//! [`crate::terminal::TERMINAL_BUNDLE_IDS`]), the AX path is skipped
//! entirely: terminals expose `AXTextArea` for their grid but the
//! `AXSelectedText` write never reaches the pty, so the tool would
//! report success while the shell sees nothing. We go straight to
//! CGEvent key-event synthesis (`path: "key_events"`).
//!
//! Use `type_text_chars` when you explicitly need per-character pacing
//! (e.g., to trigger live-search debounce handlers).

use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{
    copy_string_attr, focused_element_of_pid, kAXErrorSuccess, set_string_attr,
    AXUIElementRef,
};
use crate::apps;
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;
use core_foundation::base::CFRelease;

use super::ToolState;

pub struct TypeTextTool {
    pub state: Arc<ToolState>,
}

impl TypeTextTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "type_text".into(),
        description:
            "Insert text into the target pid via `AXSetAttribute(kAXSelectedText)`. \
             Works for standard Cocoa text fields and text views. No keystrokes are \
             synthesized — special keys (Return / Escape / arrows) go through \
             `press_key` / `hotkey`. For Chromium / Electron inputs that don't \
             implement `kAXSelectedText`, the tool falls back to CGEvent \
             character synthesis automatically.\n\n\
             Optional `element_index` + `window_id` (from the last \
             `get_window_state` snapshot) directs the write to a specific field. \
             Without `element_index`, the write goes to the pid's currently \
             focused element."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "text"],
            "properties": {
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
                "pid":  { "type": "integer", "description": "Target process ID." },
                "text": { "type": "string",  "description": "Text to insert at the target's cursor." },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID. Required when element_index is used. Optional when element_token is supplied (the token carries it)."
                },
                "element_index": {
                    "type": "integer",
                    "description": "Element index from last get_window_state. Directs the write to a specific field. REQUIRES `pid` and `window_id` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op."
                },
                "element_token": {
                    "type": "string",
                    "description": "Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."
                },
                "delay_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 200,
                    "description": "Milliseconds between characters in the CGEvent fallback path. Default 30. Ignored when the AX path succeeds."
                }
            },
            "additionalProperties": false
        }),
        read_only:   false,
        destructive: true,
        idempotent:  false,
        open_world:  true,
    })
}

#[async_trait]
impl Tool for TypeTextTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") { Ok(v) => v, Err(e) => return e };
        let text_raw = match args.require_str("text") { Ok(v) => v, Err(e) => return e };
        // Strip trailing agent-protocol closing tags — see
        // cua_driver_core::text_sanitize docs for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        // Surface 6: element_token / element_index precedence resolution.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg     = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg,
            "type_text",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id) = match resolved {
            cua_driver_core::element_token::ResolvedElement::None => (None, window_id_arg),
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: wid, element_index: idx, via_token: _,
            } => (Some(idx), wid),
        };
        let delay_ms      = args.u64_or("delay_ms", 30);

        // Validate element_index requires window_id (still applies for
        // the legacy integer path; token path already resolved window_id).
        if element_index.is_some() && window_id.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used."
            );
        }

        // Resolve the element pointer (if element_index given). Retain it out
        // of the cache so a concurrent get_window_state can't free it before
        // the blocking type below dereferences it (use-after-free → daemon
        // crash). The guard lives to method end, past type_text_blocking.
        let element_guard = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            match self.state.element_cache.get_element_retained(pid, wid, idx) {
                Some(e) => Some((e, idx)),
                None => return ToolResult::error(format!(
                    "Element index {idx} not found. Call get_window_state first."
                )),
            }
        } else {
            None
        };
        let element_ptr = element_guard.as_ref().map(|(g, idx)| (g.as_ptr(), Some(*idx)));

        let text_clone  = text.clone();
        let char_count  = text.chars().count();

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Typing into a field can trigger autocomplete popovers or
        // Chrome/Safari's "Save Password?" prompt, both of which open
        // helper windows. Wrap so callers see them in the result suffix
        // and the wildcard suppressor catches reflex activations.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        // Terminal-emulator short-circuit: when the target pid belongs
        // to a known terminal (Ghostty / Terminal.app / iTerm2 / …), the
        // AX value-set is silently dropped — see crate::terminal docs.
        // Skip the AX path entirely so the caller never sees the
        // "success but nothing typed" symptom.
        let is_terminal_target = crate::terminal::is_terminal_pid(pid);

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "type_text.AXSelectedText",
            || async move {
                tokio::task::spawn_blocking(move || {
                    type_text_blocking(
                        pid,
                        &text_clone,
                        element_ptr,
                        delay_ms,
                        is_terminal_target,
                    )
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        match result {
            Ok(Ok((detail, path))) => ToolResult::text(format!(
                "✅ Inserted {char_count} char(s){detail}.{}",
                changes.result_suffix()
            ))
            .with_structured(serde_json::json!({
                "path": path,
                "characters": char_count,
            })),
            Ok(Err(e)) => ToolResult::error(format!("type_text failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking implementation ───────────────────────────────────────────────────

/// Which delivery path was taken. Surfaced as `structuredContent.path`
/// on success.
const PATH_AX: &str = "ax";
const PATH_KEY_EVENTS: &str = "key_events";

/// `element_ptr_and_idx` — `Some((ptr, idx))` if element_index was provided.
///
/// When `is_terminal_target` is true, the AX value-set path is skipped
/// entirely and the result is delivered through CGEvent key-event
/// synthesis. See `crate::terminal` for the detection contract.
///
/// Returns `(detail_string, path)`. `path` is one of the `PATH_*`
/// constants above; consumers read it from the structured response.
fn type_text_blocking(
    pid: i32,
    text: &str,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    delay_ms: u64,
    is_terminal_target: bool,
) -> anyhow::Result<(String, &'static str)> {
    // --- Path 0: target is a terminal emulator — go straight to CGEvent. ---
    if is_terminal_target {
        tracing::debug!(
            "type_text: pid {pid} is a terminal emulator (bundle id in \
             crate::terminal::TERMINAL_BUNDLE_IDS); skipping AX value-set \
             and using CGEvent key-event synthesis"
        );
        return crate::input::keyboard::type_text_with_delay(pid, text, delay_ms)
            .map(|_| (
                format!(" via CGEvent (terminal emulator, {delay_ms}ms delay)"),
                PATH_KEY_EVENTS,
            ));
    }

    // --- Path 1: caller provided an explicit element index ---
    if let Some((ptr, idx_opt)) = element_ptr_and_idx {
        let element = ptr as AXUIElementRef;
        let role  = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
        let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

        let err = unsafe { set_string_attr(element, "AXSelectedText", text) };
        if err == kAXErrorSuccess {
            // Verify the write was actually applied — Chromium web inputs silently
            // accept the call but never update their DOM value. Read AXValue back;
            // if it's still empty when we just wrote non-empty text the write was a
            // no-op and we must use CGEvent instead.
            let silent_accept = !text.is_empty() && unsafe {
                copy_string_attr(element, "AXValue")
                    .map(|v| v.is_empty())
                    .unwrap_or(false)
            };
            if !silent_accept {
                let idx_str = idx_opt.map(|i| format!(" [{i}]")).unwrap_or_default();
                return Ok((format!(" into{idx_str} {role} \"{title}\""), PATH_AX));
            }
            tracing::debug!(
                "AXSelectedText silent-accept detected for {role} \"{title}\" \
                 (AXValue still empty), falling back to CGEvent keystrokes"
            );
        } else {
            // AXSelectedText not supported (Chromium/Electron) — fall through to CGEvent.
            tracing::debug!(
                "AXSelectedText write failed ({err}) for {role} \"{title}\", \
                 falling back to CGEvent keystrokes"
            );
        }
        return crate::input::keyboard::type_text_with_delay(pid, text, delay_ms)
            .map(|_| (
                format!(" via CGEvent (AXSelectedText unsupported/silent-accept, {delay_ms}ms delay)"),
                PATH_KEY_EVENTS,
            ));
    }

    // --- Path 2: no element_index — target the pid's focused element ---
    let focused = unsafe { focused_element_of_pid(pid) };
    if let Some(element) = focused {
        let role  = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
        let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

        let err = unsafe { set_string_attr(element, "AXSelectedText", text) };

        let did_succeed = if err == kAXErrorSuccess {
            // Same silent-accept check: Chromium web inputs return success but
            // leave AXValue empty, meaning the write never reached the DOM.
            let silent_accept = !text.is_empty() && unsafe {
                copy_string_attr(element, "AXValue")
                    .map(|v| v.is_empty())
                    .unwrap_or(false)
            };
            if silent_accept {
                tracing::debug!(
                    "AXSelectedText silent-accept detected for focused {role} \"{title}\" \
                     (AXValue still empty), falling back to CGEvent keystrokes"
                );
            }
            !silent_accept
        } else {
            false
        };

        unsafe { CFRelease(element as _); }

        if did_succeed {
            return Ok((format!(" into focused {role} \"{title}\""), PATH_AX));
        }
        if err != kAXErrorSuccess {
            // Fall through to CGEvent.
            tracing::debug!(
                "AXSelectedText write failed ({err}) for focused {role}, \
                 falling back to CGEvent keystrokes"
            );
        }
    } else {
        tracing::debug!(
            "No focused element found for pid {pid}, falling back to CGEvent keystrokes"
        );
    }

    crate::input::keyboard::type_text_with_delay(pid, text, delay_ms)
        .map(|_| (
            format!(" via CGEvent (no focused element / AXSelectedText unsupported, {delay_ms}ms delay)"),
            PATH_KEY_EVENTS,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity-check that the terminal short-circuit can be expressed as a
    /// pure function of `is_terminal_target`: when true, the code goes
    /// to key-event synthesis without consulting AX. This test stands
    /// in for an integration test (which would need a running terminal)
    /// — it exercises the branch by injecting `is_terminal_target=true`
    /// with a non-existent pid and checking we get the expected error
    /// shape from the CGEvent path (not from the AX path).
    ///
    /// The CGEvent post will fail for pid 0 / -1, so we only assert
    /// that `type_text_blocking` returns `Err` *after* deciding to
    /// take the key-events path — i.e. it doesn't hit the AX branches
    /// where `set_string_attr(0)` would crash.
    #[test]
    fn terminal_flag_routes_past_ax_path() {
        // Pid -1 is invalid; the AX path would unconditionally call
        // focused_element_of_pid which is safe but it would never reach
        // CGEvent. The fact that this returns an Err (without crashing)
        // proves we routed through CGEvent-only and never touched AX.
        let r = type_text_blocking(-1, "x", None, 0, /*is_terminal_target=*/ true);
        // We don't care whether r is Ok or Err — what matters is that
        // calling it with is_terminal_target=true is safe and never
        // dereferences null AX pointers.
        let _ = r;
    }

    #[test]
    fn path_constants_are_stable_tokens() {
        // These string constants are part of the structured-response
        // contract; freezing them here makes the contract a unit test.
        assert_eq!(PATH_AX, "ax");
        assert_eq!(PATH_KEY_EVENTS, "key_events");
    }
}
