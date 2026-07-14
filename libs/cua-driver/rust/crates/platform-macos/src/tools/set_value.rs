//! set_value tool — matches the Swift reference in SetValueTool.swift.
//!
//! Two modes, determined by the element's AXRole:
//!
//! * **AXPopUpButton**: Find the child option whose AXTitle or AXValue matches
//!   `value` (case-insensitive) and AXPress it directly.  The native macOS popup
//!   menu is never opened, so focus is never stolen.  Falls back to Safari
//!   `osascript do JavaScript` for WebKit `<select>` elements that expose no AX
//!   children when the popup is closed.
//!
//! * **Everything else**: Write `AXValue` directly (sliders, steppers, native
//!   text fields that expose a settable AXValue).

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_children, copy_number_attr, copy_string_attr, kAXErrorSuccess, perform_action,
    set_number_attr, set_string_attr, AXUIElementRef,
};
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;
use core_foundation::base::CFRelease;

use super::ToolState;

pub struct SetValueTool {
    state: Arc<ToolState>,
}

impl SetValueTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "set_value".into(),
        description:
            "Set a value on a UI element. Two modes depending on element role:\n\
             \n\
             - **AXPopUpButton / select dropdown**: finds the child option whose \
             title or value matches `value` (case-insensitive) and AXPresses it \
             directly — the native macOS popup menu is never opened, so focus \
             is never stolen. Use this for HTML <select> elements in Safari or \
             any native NSPopUpButton.\n\
             \n\
             - **All other elements**: writes AXValue directly (sliders, steppers, \
             date pickers, native text fields that expose settable AXValue).\n\
             \n\
             For free-form text entry into web inputs, prefer `type_text_chars` \
             which synthesises key events — AXValue writes are ignored by WebKit.\n\
             \n\
             Success returns `changed`, `verified`, and `readback_value` when \
             AXValue is readable. Repeating the same value is an idempotent \
             success with `changed:false`."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "value"],
            "properties": {
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
                "pid": { "type": "integer" },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID for the window whose get_window_state produced the element_index. Required when element_index is used; optional when element_token is supplied (the token carries it)."
                },
                "element_index": { "type": "integer", "description": "Element index from last get_window_state. Must be supplied unless element_token is provided. REQUIRES `pid` and `window_id` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op." },
                "element_token": { "type": "string",  "description": "Opaque stable AX node handle from `structuredContent.elements[].element_token`. Strictly bound to pid, window_id, generation, element_index, and AX node identity. If window_id or element_index are also supplied they must match. Unknown, cross-target, stale-generation, or identity-mismatch tokens fail closed." },
                "value": {
                    "type": "string",
                    "description": "New value. AX will coerce to the element's native type."
                }
            },
            "additionalProperties": false
        }),
        read_only:   false,
        destructive: true,
        idempotent:  true,
        open_world:  true,
    })
}

#[async_trait]
impl Tool for SetValueTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let value = match args.require_str("value") {
            Ok(v) => v,
            Err(e) => return e,
        };

        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let (element_index, window_id, token_guard, token_generation) =
            if let Some(token) = element_token_arg.as_deref() {
                match self.state.element_cache.resolve_token(
                    pid,
                    window_id_arg,
                    element_index_arg,
                    token,
                ) {
                    Ok(validated) => (
                        validated.element_index,
                        validated.window_id,
                        Some(validated.element),
                        Some(validated.generation),
                    ),
                    Err(error) => return error.into_tool_result(),
                }
            } else if let Some(element_index) = element_index_arg {
                let Some(window_id) = window_id_arg else {
                    return ToolResult::error(
                        "set_value requires window_id when element_index is used \
                         (omit only when supplying element_token, which carries it).",
                    );
                };
                (element_index, window_id, None, None)
            } else {
                return ToolResult::error(
                    "set_value requires element_index (+ window_id) or element_token to \
                     address the target element.",
                );
            };

        // Retain out of the cache so a concurrent get_window_state can't free
        // the element mid-action (use-after-free → daemon crash). Guard lives
        // to the end of this method, past the AX write below.
        let element_guard =
            match token_guard {
                Some(element) => element,
                None => match self.state.element_cache.get_element_retained(
                    pid,
                    window_id,
                    element_index,
                ) {
                    Some(e) => e,
                    None => {
                        return ToolResult::error(format!(
                            "Element index {element_index} not found. Call get_window_state first."
                        ))
                    }
                },
            };
        let element_ptr = element_guard.as_ptr();

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // AXValue writes on popups / sliders can cause reflex activations
        // in Chromium-based apps; the AXPopUpButton path also AXPresses a
        // child option which can trigger app activation in some setups.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "set_value.AXValue",
            || async move {
                tokio::task::spawn_blocking(move || {
                    set_value_blocking(element_ptr, element_index, pid, &value)
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        match result {
            Ok(Ok(mut outcome)) => {
                outcome.message.push_str(&changes.result_suffix());
                let mut structured = serde_json::json!({
                    "path": "ax",
                    "element_index": element_index,
                    "window_id": window_id,
                    "changed": outcome.changed,
                    "verified": outcome.verified,
                });
                if let Some(readback) = outcome.readback_value {
                    structured["value"] = serde_json::json!(readback);
                    structured["readback_value"] = structured["value"].clone();
                }
                if let Some(generation) = token_generation {
                    structured["element_token"] = serde_json::json!(element_token_arg.as_deref());
                    structured["generation"] = serde_json::json!(generation);
                }
                ToolResult::text(outcome.message).with_structured(structured)
            }
            Ok(Err(e)) => ToolResult::error(format!("set_value failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking implementation (runs on spawn_blocking thread) ─────────────────

struct SetValueOutcome {
    message: String,
    readback_value: Option<String>,
    changed: bool,
    verified: bool,
}

fn set_value_blocking(
    element_ptr: usize,
    element_index: usize,
    pid: i32,
    value: &str,
) -> anyhow::Result<SetValueOutcome> {
    let element = element_ptr as AXUIElementRef;

    let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
    let before = read_ax_value(element);
    if before
        .as_deref()
        .is_some_and(|current| values_equivalent(current, value))
    {
        return Ok(SetValueOutcome {
            message: format!(
                "✅ AXValue on [{element_index}] {role} already equals the requested value."
            ),
            readback_value: before,
            changed: false,
            verified: true,
        });
    }

    let message = if role == "AXPopUpButton" {
        let element_title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
        select_popup_option(element, element_index, pid, value, &element_title)?
    } else {
        // Default path: write AXValue directly. Numeric controls (AXSlider /
        // AXStepper) reject a CFString with -25201 and need a CFNumber; text
        // fields take a CFString. Try numeric first when the value parses as a
        // number, then fall back to a string write.
        // Numeric target carried through so we can step toward it if the
        // direct writes are rejected (SwiftUI AXSlider rejects every AXValue
        // write with -25200 yet exposes a readable AXValue + increment/decrement
        // actions).
        let numeric_target = value.trim().parse::<f64>().ok();
        let err = match numeric_target {
            Some(n) => {
                let e = unsafe { set_number_attr(element, "AXValue", n) };
                if e == kAXErrorSuccess {
                    e
                } else {
                    unsafe { set_string_attr(element, "AXValue", value) }
                }
            }
            None => unsafe { set_string_attr(element, "AXValue", value) },
        };
        if err == kAXErrorSuccess {
            format!("✅ Set AXValue on [{element_index}] {role}.")
        } else if let Some(target) = numeric_target {
            // Both direct writes failed for a numeric target — fall back to
            // stepping the control via AXIncrement / AXDecrement actions.
            if step_to_value(element, target) {
                format!(
                    "✅ Set AXValue on [{element_index}] {role} via AXIncrement/AXDecrement stepping."
                )
            } else {
                anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
            }
        } else {
            anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
        }
    };
    let readback_value = read_ax_value(element);
    let verified = readback_value
        .as_deref()
        .is_some_and(|current| values_equivalent(current, value));
    Ok(SetValueOutcome {
        message,
        readback_value,
        changed: true,
        verified,
    })
}

fn read_ax_value(element: AXUIElementRef) -> Option<String> {
    unsafe { copy_string_attr(element, "AXValue") }
        .or_else(|| unsafe { copy_number_attr(element, "AXValue") }.map(|value| value.to_string()))
}

fn values_equivalent(current: &str, requested: &str) -> bool {
    if current == requested {
        return true;
    }
    match (
        current.trim().parse::<f64>(),
        requested.trim().parse::<f64>(),
    ) {
        (Ok(current), Ok(requested)) => {
            (current - requested).abs()
                <= f64::EPSILON * current.abs().max(requested.abs()).max(1.0)
        }
        _ => false,
    }
}

// ── AXIncrement / AXDecrement stepping fallback ──────────────────────────────

/// Step a numeric control toward `target` using its `AXIncrement` /
/// `AXDecrement` actions. Used only when direct `AXValue` writes are rejected
/// (notably SwiftUI's `AXSlider`, which exposes a readable-but-unsettable
/// `AXValue` plus increment/decrement actions).
///
/// Returns `true` once the control's value lands within half of the last
/// observed step of `target`, `false` if it can't be read or can't be moved.
fn step_to_value(element: AXUIElementRef, target: f64) -> bool {
    // Can't target precisely without feedback — bail if AXValue is unreadable.
    let mut current = match unsafe { copy_number_attr(element, "AXValue") } {
        Some(v) => v,
        None => return false,
    };

    // Half of the last observed step. Start near-zero so we never declare the
    // target "reached" before performing (and observing) a real
    // AXIncrement/AXDecrement — otherwise a slider at 0.0 targeting 0.5 would
    // report success without ever moving. The radius widens only after we learn
    // the control's actual step size from an observed value change.
    let mut step_radius = f64::EPSILON;

    // Hard cap to prevent runaway on a control that never quite converges.
    for _ in 0..500 {
        if (current - target).abs() <= step_radius {
            return true;
        }

        let action = if current < target {
            "AXIncrement"
        } else {
            "AXDecrement"
        };
        let _ = unsafe { perform_action(element, action) };

        let next = match unsafe { copy_number_attr(element, "AXValue") } {
            Some(v) => v,
            None => return false,
        };

        // The action didn't move the value — the control can't be stepped (or
        // has hit a min/max bound short of target). Stop to avoid looping.
        if next == current {
            return false;
        }

        // Refine the stop threshold to half of the actual step the control took.
        let step = (next - current).abs();
        if step > 0.0 {
            step_radius = step / 2.0;
        }
        current = next;
    }

    // Exhausted the iteration cap without converging.
    (current - target).abs() <= step_radius
}

// ── AXPopUpButton path ───────────────────────────────────────────────────────

fn select_popup_option(
    element: AXUIElementRef,
    element_index: usize,
    pid: i32,
    value: &str,
    element_title: &str,
) -> anyhow::Result<String> {
    let children = unsafe { copy_children(element) };

    if !children.is_empty() {
        // Strategy 1: AX children (native AppKit NSPopUpButton).
        let value_lower = value.to_lowercase();
        let mut matched_idx: Option<usize> = None;
        let mut available: Vec<String> = Vec::with_capacity(children.len());

        for (i, &child) in children.iter().enumerate() {
            let child_title = unsafe { copy_string_attr(child, "AXTitle") }.unwrap_or_default();
            let child_value = unsafe { copy_string_attr(child, "AXValue") }.unwrap_or_default();
            available.push(child_title.clone());
            if child_title.to_lowercase() == value_lower
                || child_value.to_lowercase() == value_lower
            {
                matched_idx = Some(i);
                break;
            }
        }

        let result = if let Some(i) = matched_idx {
            let child = children[i];
            let opt_title =
                unsafe { copy_string_attr(child, "AXTitle") }.unwrap_or_else(|| value.to_string());
            let err = unsafe { perform_action(child, "AXPress") };
            if err == kAXErrorSuccess {
                Ok(format!(
                    "✅ Selected '{opt_title}' in AXPopUpButton [{element_index}] \
                     \"{element_title}\" via AX child AXPress."
                ))
            } else {
                anyhow::bail!("AXPress on child option failed with error {err}")
            }
        } else {
            let avail = available
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "No AX child matching '{value}' in AXPopUpButton [{element_index}] \
                 \"{element_title}\". Available: [{avail}]"
            )
        };

        // Release children (copy_children retains each one).
        for &child in &children {
            unsafe {
                CFRelease(child as _);
            }
        }

        return result;
    }

    // Strategy 2: Safari/WebKit — no AX children when popup is closed.
    // Use osascript do JavaScript to set the <select> element's DOM value.
    let app_name = crate::apps::get_app_name_for_pid(pid).unwrap_or_default();

    if app_name != "Safari" {
        anyhow::bail!(
            "AXPopUpButton [{element_index}] '{element_title}' has no AX children and \
             target is '{app_name}' (not Safari) — no fallback available."
        )
    }

    set_select_via_js(element_index, element_title, value)
}

// ── Safari JavaScript fallback ───────────────────────────────────────────────

/// Set an HTML `<select>` value in Safari via `osascript do JavaScript`.
/// Searches all `<select>` elements for an `<option>` whose text or value matches
/// `value` (case-insensitive), then sets it and dispatches a `change` event.
fn set_select_via_js(
    element_index: usize,
    element_title: &str,
    value: &str,
) -> anyhow::Result<String> {
    // Percent-encode the lowercased value using only unreserved URL characters
    // as the allowed set, matching the Swift reference's percent-encoding approach.
    // This makes the string safe to embed in both a JS single-quoted string
    // (via decodeURIComponent) and an AppleScript double-quoted string.
    let v_low = value.to_lowercase();
    let v_encoded = percent_encode_unreserved(&v_low);

    // JavaScript that matches the Swift reference verbatim.
    let js = format!(
        "(function(){{\
         var v=decodeURIComponent('{v_encoded}');\
         var ss=document.querySelectorAll('select'),opts=[];\
         for(var i=0;i<ss.length;i++){{\
         for(var j=0;j<ss[i].options.length;j++){{\
         var t=ss[i].options[j].text.toLowerCase(),\
         u=ss[i].options[j].value.toLowerCase();\
         opts.push(t+'|'+u);\
         if(t===v||u===v){{\
         ss[i].value=ss[i].options[j].value;\
         ss[i].dispatchEvent(new Event('change',{{bubbles:true}}));\
         return 'SET:'+ss[i].value;}}}}\
         }}return 'NOTFOUND:'+opts.join(',');\
         }})()"
    );

    let apple_script =
        format!("tell application \"Safari\" to do JavaScript \"{js}\" in front document");

    // Spawn osascript with a 10-second deadline. A stuck Safari permission
    // prompt or unresponsive renderer can cause wait() to block indefinitely,
    // which would stall the MCP tool handler permanently.
    let mut child = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&apple_script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("osascript launch failed: {e}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    anyhow::bail!("osascript timed out after 10 seconds");
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => anyhow::bail!("osascript wait error: {e}"),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| anyhow::anyhow!("osascript output error: {e}"))?;

    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();

    if raw.starts_with("SET:") {
        let dom_val = &raw[4..];
        Ok(format!(
            "✅ Set select [{element_index}] '{element_title}' to '{value}' via \
             Safari JavaScript (DOM value: \"{dom_val}\")."
        ))
    } else if raw.starts_with("NOTFOUND:") {
        let available = &raw[9..];
        anyhow::bail!(
            "No <option> matching '{value}' found in any <select>. \
             Available (text|value): {available}"
        )
    } else if raw.is_empty() && !out.status.success() {
        let err_text = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("osascript failed: {}", err_text.trim())
    } else {
        anyhow::bail!(
            "JavaScript returned unexpected output: {}",
            &raw[..raw.len().min(200)]
        )
    }
}

// ── Percent-encoding helper ──────────────────────────────────────────────────

/// Percent-encode a string, leaving only unreserved URL characters (`-._~` +
/// alphanumerics) unencoded.  Matches the Swift reference's approach.
fn percent_encode_unreserved(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_' || b == b'~' {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0xF));
        }
    }
    out
}

fn hex_digit(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + n - 10) as char,
        _ => '0',
    }
}

#[cfg(test)]
mod tests {
    use super::values_equivalent;

    #[test]
    fn value_equivalence_is_idempotent_for_text_and_numeric_representations() {
        assert!(values_equivalent("ready", "ready"));
        assert!(!values_equivalent("ready", "Ready"));
        assert!(values_equivalent("1", "1.0"));
        assert!(values_equivalent("0.5", "0.5000000000000000"));
        assert!(!values_equivalent("1", "2"));
    }
}
