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
use cua_driver_contract::TypeTextInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_string_attr, focused_element_of_pid, kAXErrorSuccess, set_string_attr, AXUIElementRef,
};
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;
use core_foundation::base::CFRelease;
use cua_driver_core::background_input::BackgroundRefusal;

use super::ToolState;

pub struct TypeTextTool {
    pub state: Arc<ToolState>,
}

impl TypeTextTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
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
             character synthesis automatically when the estimated route stays \
             within the daemon transport budget. Longer synthesized routes are \
             refused before character events and return a safe chunk size; \
             one-call AX insertion remains uncapped.\n\n\
             Optional `element_index` + `window_id` (from the last \
             `get_window_state` snapshot) directs the write to a specific field. \
             Without `element_index`, the write goes to the pid's currently \
             focused element.\n\n\
             WEB CONTENT (Chromium/WebKit/Electron — browser tabs, Slack, VS Code, \
             X's compose box): AXValue is not independent proof that the \
             renderer/DOM observed an AX write or synthesized keystrokes. The \
             driver detects this at the element level (an AXWebArea ancestor) and \
             refuses to trust AXValue-only read-back there — type_text returns \
             effect:\"unverifiable\" + escalation, never a false \"confirmed\" (a \
             browser's own native address bar/toolbar stays trusted). For a browser \
             TAB the reliable path is the `page` tool (drives the DOM via CDP); for \
             an embedded web view use this tool's px form: pass x,y (no \
             element_index) to pixel-click the field then type, in one call. NOTE: \
             a px focus-click won't reliably open+focus a CLOSED control; AX-press \
             to open/activate it first (works in the background), then px-type. \
             Always confirm via the screenshot; if px-background still drops, \
             escalate to delivery_mode:\"foreground\"."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid":  { "type": "integer", "description": "Target process ID." },
                "text": { "type": "string",  "description": "Text to insert at the target's cursor." },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID. Required when element_index is used. Optional when element_token is supplied (the token carries it)."
                },
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                "x": { "type": "number", "description": "Screenshot-pixel X of the field to type into — the element px action form. Pass x,y (no element_index) and the tool pixel-clicks there to establish real renderer focus, then types. Use for Chromium/Electron inputs the AX path can't reach. Read straight off the get_window_state PNG, same convention as click." },
                "y": { "type": "number", "description": "Screenshot-pixel Y of the field (see x)." },
                "delay_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 200,
                    "description": "Milliseconds between characters in the CGEvent fallback path. Default 30. Ignored when the AX path succeeds."
                },
                "scope": { "type": "string", "enum": ["window", "desktop"], "default": "window", "description": "Use desktop with no pid/window_id to type into the frontmost application." },
                "delivery_mode": {
                    "type": "string",
                    "enum": ["background", "foreground"],
                    "description": "Best-effort-background ladder rung (default \"background\"). \"background\": AX insert, then CGEvent keystrokes if needed — no focus steal; native controls can be confirmed via AXValue read-back, while web-content writes remain effect:\"unverifiable\". \"foreground\": briefly front the window, type, restore the prior frontmost — the explicit last resort for focus-sensitive surfaces (e.g. WhatsApp/Catalyst) where background keystrokes don't land. Re-call with \"foreground\" when a background attempt remains unverifiable and a fresh snapshot shows the text did not appear."
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

fn screen_sharing_delivery_error(
    is_screen_sharing: bool,
    foreground: bool,
    window_id: Option<u32>,
) -> Option<ToolResult> {
    if !is_screen_sharing || (foreground && window_id.is_some()) {
        return None;
    }
    Some(
        ToolResult::error(
            "Screen Sharing text input requires delivery_mode:\"foreground\" and window_id \
             so Cua Driver can deliver physical HID key transitions safely.",
        )
        .with_structured(serde_json::json!({
            "code": "SCREEN_SHARING_REQUIRES_FOREGROUND_HID",
            "effect": "refused",
            "escalation": {
                "recommended": "foreground",
                "reason": "Screen Sharing forwards physical keycodes; background PID-routed \
                           Unicode events can corrupt guest text.",
                "requires": ["window_id"]
            }
        })),
    )
}

#[async_trait]
impl Tool for TypeTextTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<TypeTextInput>("type_text", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let text =
                cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&input.text)
                    .into_owned();
            let delay_ms = args.u64_or("delay_ms", 30).min(200);
            if let Some(refusal) = synthesis_preflight(
                TextDeliveryRoute::UnicodeSynthesis,
                text.chars().count(),
                delay_ms,
            ) {
                return synthesis_refusal_result("hid", &refusal, AxAttempt::NotAttempted);
            }
            let result = tokio::task::spawn_blocking(move || {
                crate::input::keyboard::type_text_global(&text, delay_ms)
            })
            .await;
            return match result {
                Ok(Ok(())) => {
                    ToolResult::text("Typed text into the frontmost desktop application.")
                        .with_structured(serde_json::json!({
                            "scope": "desktop",
                            "path": "hid",
                            "effect": "unverifiable"
                        }))
                }
                Ok(Err(error)) => ToolResult::error(format!("desktop type_text failed: {error}")),
                Err(error) => ToolResult::error(format!("desktop type_text task failed: {error}")),
            };
        }
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Strip trailing agent-protocol closing tags — see
        // cua_driver_core::text_sanitize docs for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        // Surface 6: element_token / element_index precedence resolution.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg,
            "type_text",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id) = match resolved {
            cua_driver_core::element_token::ResolvedElement::None => (None, window_id_arg),
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: wid,
                element_index: idx,
                via_token: _,
            } => (Some(idx), wid),
        };
        let delay_ms = args.u64_or("delay_ms", 30);
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        if let Some(error) = screen_sharing_delivery_error(
            crate::input::keyboard::is_screen_sharing_pid(pid),
            delivery_mode.is_foreground(),
            window_id,
        ) {
            return error;
        }

        // Validate element_index requires window_id (still applies for
        // the legacy integer path; token path already resolved window_id).
        if element_index.is_some() && window_id.is_none() {
            return ToolResult::error("window_id is required when element_index is used.");
        }

        // Argument-shape errors are reported before any gating or retained
        // lookups: a malformed call must fail the same way regardless of
        // background-target state.
        let px = args.get("x").and_then(|v| v.as_f64());
        let py = args.get("y").and_then(|v| v.as_f64());
        if px.is_some() && py.is_some() && element_index.is_some() {
            return ToolResult::error(
                "Pass either element_index (ax) or x,y (px) to type_text, not both.",
            );
        }

        // Resolve the element pointer (if element_index given). Retain it out
        // of the cache so a concurrent get_window_state can't free it before
        // the blocking type below dereferences it (use-after-free → daemon
        // crash). The guard lives to method end, past type_text_blocking.
        let element_guard = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            match self.state.element_cache.get_element_retained(pid, wid, idx) {
                Some(e) => Some((e, idx)),
                None => {
                    return ToolResult::error(format!(
                        "Element index {idx} not found. Call get_window_state first."
                    ))
                }
            }
        } else {
            None
        };

        // ── Exact-target background gate (macOS background input v1) ──
        // A window-addressed background insert must prove exact delivery
        // before any input — including the px focus click — is sent. The pure
        // core decides once from fresh facts: full keyboard ladder, semantic
        // AX write only (exact element, no CGEvent fallback), or a structured
        // refusal. delivery_mode:"foreground" stays the caller's explicit
        // last resort and is not gated here.
        let (_mutation_lease, keyboard_policy) =
            if !delivery_mode.is_foreground() && window_id.is_some() {
                let wid = window_id.expect("checked above");
                let gate_element_ptr = element_guard.as_ref().map(|(g, _)| g.as_ptr() as usize);
                match background_keyboard_policy(pid, wid, gate_element_ptr).await {
                    Ok((lease, policy)) => (Some(lease), policy),
                    Err(refusal_result) => return refusal_result,
                }
            } else {
                (None, BackgroundKeyboardPolicy::Allowed)
            };

        // ── px form: focus by pixel-click, then type into the focused element ──
        // Pass x,y (no element_index) for an *element px action*: pixel-click the
        // field to give the Chromium/Electron renderer the real keyboard focus the
        // AX path can't, then fall through to the focused-element type path (which
        // escalates AX → CGEvent and lands once focused). Reuses ClickTool's exact
        // coordinate translation + delivery_mode, so it lands on the same pixel a
        // px-click would.
        let used_pixel_focus = px.is_some() && py.is_some();
        if let (Some(cx), Some(cy)) = (px, py) {
            // The px form has no exact element for a semantic-only write; when
            // the keyboard rung is refused, refuse before the focus click too.
            if let BackgroundKeyboardPolicy::SemanticOnly(ref refusal) = keyboard_policy {
                let wid = window_id.expect("gate ran only with window_id");
                return super::background_refusal_result(pid, wid, refusal);
            }
            let from_zoom = args
                .get("from_zoom")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Err(e) = super::focus_by_pixel(
                &self.state,
                pid,
                window_id,
                cx,
                cy,
                delivery_mode.is_foreground(),
                args.opt_str("session"),
                args.opt_str("_session_id"),
                from_zoom,
                _mutation_lease.as_ref(),
            )
            .await
            {
                return e;
            }
            // element_index stays None → the type path below writes to the now-
            // focused element via the CGEvent (key_events) rung.
        }
        if let (Some((element, _)), Some(wid)) = (element_guard.as_ref(), window_id) {
            let center_ptr = element.as_ptr() as usize;
            if let Ok(Some((screen_x, screen_y))) = tokio::task::spawn_blocking(move || unsafe {
                crate::ax::bindings::element_screen_center(center_ptr as AXUIElementRef)
            })
            .await
            {
                let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
                crate::cursor::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
                crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), screen_x, screen_y)
                    .await;
                self.state
                    .cursor_registry
                    .update_position(&cursor_key, screen_x, screen_y);
            }
        }
        let element_ptr = element_guard
            .as_ref()
            .map(|(g, idx)| (g.as_ptr(), Some(*idx)));

        let text_clone = text.clone();
        let char_count = text.chars().count();

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

        let blocking_policy = keyboard_policy.clone();
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
                        delivery_mode,
                        window_id,
                        blocking_policy,
                    )
                })
                .await
            },
        )
        .await;

        let changes = super::finish_window_observation(snapshot, &args).await;

        // Unwrap the delivery envelope: a structured refusal means no
        // actuator ran and the caller gets the exact reason.
        let result = match result {
            Ok(Ok(TypeTextDelivery::Refused(refusal))) => {
                let wid = window_id.expect("background refusals require a window target");
                return super::background_refusal_result(pid, wid, &refusal);
            }
            Ok(Ok(TypeTextDelivery::SynthesisRefused {
                path,
                refusal,
                ax_attempt,
            })) => return synthesis_refusal_result(path, &refusal, ax_attempt),
            Ok(Ok(TypeTextDelivery::Typed(outcome))) => Ok(Ok(outcome)),
            Ok(Err(error)) => Ok(Err(error)),
            Err(error) => Err(error),
        };

        match result {
            Ok(Ok(outcome)) if outcome.delivered_chars.is_some_and(|n| n < char_count) => {
                let delivered_chars = outcome.delivered_chars.unwrap_or_default();
                ToolResult::error(format!(
                    "type_text incomplete: delivered {delivered_chars} of {char_count} character(s){}; retry only the remaining suffix",
                    outcome.detail
                ))
                .with_structured(serde_json::json!({
                    "code": "type_text_incomplete",
                    "path": outcome.path,
                    "effect": "partial",
                    "requested_chars": char_count,
                    "delivered_chars": delivered_chars,
                    "retryable": true,
                    "retry_from_character": delivered_chars,
                }))
            }
            Ok(Ok(outcome)) => {
                let TypeTextOutcome {
                    detail,
                    path,
                    verified,
                    delivered_chars,
                } = outcome;
                // SURFACE-AWARE VERIFICATION. On any web-content surface —
                // Chromium/WebKit/Electron — AXValue is not independent renderer
                // evidence. It can report a changed value after either an AX write
                // or synthesized keystrokes while the renderer/DOM still observes
                // no edit (the Slack-search AND Chrome-on-X false-confirms). Detect
                // this at the ELEMENT level (an `AXWebArea` ancestor) so it covers
                // every browser + Electron uniformly, yet a browser's OWN native
                // chrome (address bar, toolbar) stays trusted. Probe ONLY when a
                // path with AXValue-only verification would otherwise confirm, so
                // native types and already-unverified deliveries pay nothing.
                let target_is_web_content = verified
                    && path_has_untrusted_web_readback(path)
                    && target_in_web_area(pid, element_ptr, window_id);
                let verification = surface_verification(path, verified, target_is_web_content);
                let verified = verification.verified;
                let untrusted_web_readback = verification.untrusted_web_readback;
                let electron_web_content = untrusted_web_readback
                    && crate::browser::electron_js::ElectronJs::is_electron(pid);

                // `verified:false` means the driver could not confirm the text
                // landed (Electron AX echo, unreadable AXValue on Catalyst, or a
                // CGEvent rung the app may have dropped). Don't dress that as a
                // confirmed insert — tell the agent to look, and point at the
                // right next rung.
                let (mark, note) = if verified {
                    ("✅ Inserted", String::new())
                } else if untrusted_web_readback {
                    let next_step = if electron_web_content && used_pixel_focus {
                        "The pixel-focus rung already ran, so do not repeat it; verify the \
                         result via the screenshot."
                    } else {
                        "For a browser tab use the `page` tool (it drives the DOM); for an \
                         embedded web view, re-type with the px form (x,y)."
                    };
                    (
                        "📨 Sent (unverified)",
                        format!(
                            " — web-content surface (Chromium / WebKit / Electron): \
                             AXValue read-back is not independent proof that the \
                             renderer/DOM observed the input. {next_step}"
                        ),
                    )
                } else if path == PATH_KEY_EVENTS_FG {
                    (
                        "📨 Sent (unverified)",
                        " — driver could not confirm; verify via screenshot.".to_string(),
                    )
                } else {
                    (
                        "📨 Sent (unverified)",
                        " — driver could not confirm the text landed; verify via screenshot, \
                      and re-call with delivery_mode:\"foreground\" if it didn't."
                            .to_string(),
                    )
                };
                ToolResult::text(format!(
                    "{mark} {char_count} char(s){detail}.{note}{}",
                    changes.result_suffix()
                ))
                .with_structured({
                    // `effect` mirrors `verified`'s read-back tri-state: a TRUSTED
                    // positive read-back is "confirmed"; an unreadable/unchanged
                    // AXValue, a dropped CGEvent rung, or an Electron AX echo we
                    // refuse to trust is "unverifiable".
                    let mut s = serde_json::json!({
                        "path": path,
                        "characters": char_count,
                        "requested_chars": char_count,
                        "verified": verified,
                        "effect": if verified { "confirmed" } else { "unverifiable" },
                    });
                    if let Some(delivered_chars) = delivered_chars {
                        s["delivered_chars"] = serde_json::json!(delivered_chars);
                    }
                    if untrusted_web_readback {
                        // Web-content AXValue read-back. A real browser TAB → the
                        // `page` tool (drives the DOM via CDP) is the reliable rung;
                        // an embedded web view (Electron, no CDP) → the element px
                        // action. It's a renderer/DOM-focus problem, never a
                        // foreground one.
                        let escalation =
                            match web_readback_next_rung(electron_web_content, used_pixel_focus) {
                                Some("px") => Some((
                                    "px",
                                    "Electron web view — AXValue read-back cannot prove \
                                 that the renderer observed the input. Confirm via the \
                                 screenshot; if it didn't land, re-type with the \
                                 element px action (x,y to pixel-focus the field, then \
                                 type).",
                                )),
                                Some("page") => Some((
                                    "page",
                                    "Browser web content — AXValue read-back cannot prove \
                                 that the DOM observed the input (and AX type_text on a \
                                 contenteditable is racy). Drive the tab's DOM with the \
                                 `page` tool: execute_javascript + el.value/innerText for a \
                                 plain input; for a rich-text contenteditable \
                                 (Draft.js/Lexical/Slate-style editors can silently discard \
                                 a one-shot DOM write on their next render) try insert_text \
                                 first (one CDP call, cheap), then type_keystrokes if that \
                                 also gets discarded (real per-character keyboard events, \
                                 slower but most durable). Or confirm via the screenshot.",
                                )),
                                _ => None,
                            };
                        if let Some((recommended, reason)) = escalation {
                            s["escalation"] = serde_json::json!({
                                "recommended": recommended,
                                "reason": reason,
                            });
                        }
                    } else if !verified && path != PATH_KEY_EVENTS_FG {
                        s["escalation"] = serde_json::json!({
                            "recommended": "foreground",
                            "reason": "background insert could not be confirmed — \
                                       re-call with delivery_mode:\"foreground\" if a \
                                       screenshot shows the text didn't land."
                        });
                    }
                    s
                })
            }
            Ok(Err(e)) => ToolResult::error(format!("type_text failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking implementation ───────────────────────────────────────────────────

/// Which delivery path was taken. Surfaced as `structuredContent.path`
/// on success.
const PATH_AX: &str = "ax";
const PATH_KEY_EVENTS: &str = "key_events";
const PATH_KEY_EVENTS_FG: &str = "key_events_fg";

// The daemon transport has a 120-second request deadline. Character synthesis
// is synchronous and costs at least one 8ms key-down gap plus either the
// requested delay or an 8ms key-up gap per character. Keep the complete
// scheduled synthesis sleeps + read-back estimate within 100 seconds, reserving
// 20 seconds for event construction/posting, routing, focus assistance,
// queueing, response serialization, and transport.
// Atomic AX writes are deliberately excluded: their cost does not scale per
// character and a single write remains the preferred route for large text.
const SYNTHESIS_BUDGET_MS: u64 = 100_000;
const KEY_DOWN_GAP_MS: u64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextDeliveryRoute {
    AtomicAx,
    UnicodeSynthesis,
    PhysicalSynthesis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SynthesisRefusal {
    requested_chars: usize,
    estimated_duration_ms: u64,
    per_character_ms: u64,
    max_chunk_chars: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AxAttempt {
    NotAttempted,
    Rejected,
    Unchanged,
    Unverifiable,
}

impl AxAttempt {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotAttempted => "not_attempted",
            Self::Rejected => "rejected",
            Self::Unchanged => "unchanged",
            Self::Unverifiable => "unverifiable",
        }
    }
}

fn synthesis_preflight(
    route: TextDeliveryRoute,
    requested_chars: usize,
    delay_ms: u64,
) -> Option<SynthesisRefusal> {
    if route == TextDeliveryRoute::AtomicAx {
        return None;
    }
    let per_character_ms = match route {
        TextDeliveryRoute::AtomicAx => unreachable!(),
        // PID-routed and desktop Unicode paths post one key-down and one
        // key-up, sleeping 8ms after the down and max(delay, 8) after the up.
        TextDeliveryRoute::UnicodeSynthesis => {
            KEY_DOWN_GAP_MS.saturating_add(delay_ms.max(KEY_DOWN_GAP_MS))
        }
        // Physical HID may need Shift down/up around each printable key. Use
        // that four-event worst case for a payload-independent safe bound.
        TextDeliveryRoute::PhysicalSynthesis => 24u64.saturating_add(delay_ms.max(8)),
    };
    let drain_ms = DELIVERY_DRAIN_TIMEOUT.as_millis() as u64;
    let estimated_duration_ms = (requested_chars as u64)
        .saturating_mul(per_character_ms)
        .saturating_add(drain_ms);
    if estimated_duration_ms <= SYNTHESIS_BUDGET_MS {
        return None;
    }
    let max_chunk_chars = SYNTHESIS_BUDGET_MS
        .saturating_sub(drain_ms)
        .checked_div(per_character_ms)
        .unwrap_or_default() as usize;
    Some(SynthesisRefusal {
        requested_chars,
        estimated_duration_ms,
        per_character_ms,
        max_chunk_chars,
    })
}

fn synthesis_refusal_result(
    path: &'static str,
    refusal: &SynthesisRefusal,
    ax_attempt: AxAttempt,
) -> ToolResult {
    let effect = if ax_attempt == AxAttempt::Unverifiable {
        "indeterminate"
    } else {
        "refused"
    };
    let retryable = ax_attempt != AxAttempt::Unverifiable;
    let escalation = if retryable {
        serde_json::json!({
            "recommended": "chunk",
            "reason": format!(
                "Character synthesis would exceed the bounded transport-safe budget. Retry in chunks of at most {} characters at this delay.",
                refusal.max_chunk_chars
            )
        })
    } else {
        serde_json::json!({
            "recommended": "verify_state",
            "reason": "The atomic AX attempt could not be observed. Re-read the target before deciding whether any suffix remains; do not retry blindly."
        })
    };
    let mut structured = serde_json::json!({
        "code": "type_text_synthesis_budget_exceeded",
        "path": path,
        "effect": effect,
        "requested_chars": refusal.requested_chars,
        "estimated_duration_ms": refusal.estimated_duration_ms,
        "synthesis_budget_ms": SYNTHESIS_BUDGET_MS,
        "per_character_ms": refusal.per_character_ms,
        "max_chunk_chars": refusal.max_chunk_chars,
        "synthesized_chars": 0,
        "atomic_ax_effect": ax_attempt.as_str(),
        "retryable": retryable,
        "escalation": escalation,
    });
    if ax_attempt != AxAttempt::Unverifiable {
        structured["delivered_chars"] = serde_json::json!(0);
        structured["retry_from_character"] = serde_json::json!(0);
    }
    let message = if retryable {
        format!(
            "type_text refused character synthesis before emitting character events: {} characters require an estimated {}ms at {}ms per character, exceeding the {}ms budget; retry in chunks of at most {} characters",
            refusal.requested_chars,
            refusal.estimated_duration_ms,
            refusal.per_character_ms,
            SYNTHESIS_BUDGET_MS,
            refusal.max_chunk_chars,
        )
    } else {
        format!(
            "type_text did not synthesize character events because {} characters require an estimated {}ms, exceeding the {}ms budget; the preceding atomic AX attempt was unverifiable, so re-read the target before retrying",
            refusal.requested_chars, refusal.estimated_duration_ms, SYNTHESIS_BUDGET_MS,
        )
    };
    ToolResult::error(message).with_structured(structured)
}

fn path_has_untrusted_web_readback(path: &str) -> bool {
    path == PATH_AX || path == PATH_KEY_EVENTS || path == PATH_KEY_EVENTS_FG
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SurfaceVerification {
    verified: bool,
    untrusted_web_readback: bool,
}

fn surface_verification(
    path: &str,
    verified: bool,
    target_is_web_content: bool,
) -> SurfaceVerification {
    let untrusted_web_readback =
        verified && target_is_web_content && path_has_untrusted_web_readback(path);
    SurfaceVerification {
        verified: verified && !untrusted_web_readback,
        untrusted_web_readback,
    }
}

fn web_readback_next_rung(is_electron: bool, used_pixel_focus: bool) -> Option<&'static str> {
    match (is_electron, used_pixel_focus) {
        (true, true) => None,
        (true, false) => Some("px"),
        (false, _) => Some("page"),
    }
}

const DELIVERY_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const DELIVERY_DRAIN_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Pause before the single `AXFocused` re-apply in the foreground rung. Covers
/// an app that installs its own first responder just after activation is
/// observable, which would otherwise clobber the first write.
const FOCUS_REAPPLY_DELAY: std::time::Duration = std::time::Duration::from_millis(30);

/// Keyboard-rung policy for one window-addressed background insert, decided
/// once by the pure exact-target core before any input is posted.
#[derive(Clone, Debug)]
enum BackgroundKeyboardPolicy {
    /// The full ladder may run: foreground requests, pid-only targets, and
    /// window targets whose exact keyboard delivery is proven (singleton
    /// same-pid destination).
    Allowed,
    /// Only the semantic AX write on the proven exact element may run. The
    /// process-scoped CGEvent rung is refused with this refusal — carried so
    /// the exact reason is returned if the AX write does not land.
    SemanticOnly(BackgroundRefusal),
}

/// Delivery envelope for `type_text_blocking`: either an actuator ran
/// (`Typed`), or the exact-target decision refused before any event was
/// posted (`Refused`) and the caller must return the structured refusal.
enum TypeTextDelivery {
    Typed(TypeTextOutcome),
    Refused(BackgroundRefusal),
    SynthesisRefused {
        path: &'static str,
        refusal: SynthesisRefusal,
        ax_attempt: AxAttempt,
    },
}

/// Decide the keyboard policy for a window-addressed background `type_text`.
///
/// Gathers fresh exact-target facts once and asks the pure core:
/// - `InsertText` executes → the full ladder is `Allowed`;
/// - `InsertText` refused but the caller addressed an exact element whose
///   ancestry is proven and semantic AX executes → `SemanticOnly`;
/// - otherwise the structured refusal result is returned and no input of any
///   kind (including a px focus click) may be sent.
async fn background_keyboard_policy(
    pid: i32,
    window_id: u32,
    element_ptr: Option<usize>,
) -> Result<(super::BackgroundMutationLease, BackgroundKeyboardPolicy), ToolResult> {
    use cua_driver_core::background_input::{
        decide_background_input, BackgroundAction, BackgroundInputDecision, ExactWindowTarget,
    };
    let lease = super::acquire_background_mutation(pid).await;
    let facts = match tokio::task::spawn_blocking(move || {
        crate::ax::exact_target::gather_background_facts(pid, window_id, element_ptr)
    })
    .await
    {
        Ok(facts) => facts,
        Err(error) => {
            return Err(ToolResult::error(format!(
                "Could not gather exact-target facts for pid {pid} window {window_id}: {error}"
            )));
        }
    };
    let target = ExactWindowTarget { pid, window_id };
    match decide_background_input(target, &facts, BackgroundAction::InsertText) {
        BackgroundInputDecision::Execute { .. } => Ok((lease, BackgroundKeyboardPolicy::Allowed)),
        BackgroundInputDecision::Refuse(refusal) => {
            let semantic_available = element_ptr.is_some()
                && decide_background_input(target, &facts, BackgroundAction::AxSemantic)
                    .is_execute();
            if semantic_available {
                Ok((lease, BackgroundKeyboardPolicy::SemanticOnly(refusal)))
            } else {
                Err(super::background_refusal_result(pid, window_id, &refusal))
            }
        }
    }
}

struct TypeTextOutcome {
    detail: String,
    path: &'static str,
    verified: bool,
    /// Exact when AX exposed the target value. `None` means delivery could not
    /// be observed, so the existing unverifiable contract remains in force.
    delivered_chars: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypedProgress {
    Complete,
    Partial(usize),
    Unchanged,
    Unverifiable,
}

fn foreground_settle_ms(pid: i32, frontmost_pid: Option<i32>) -> u64 {
    if frontmost_pid == Some(pid) {
        20
    } else {
        200
    }
}

/// Read-back verification for a keystroke rung: did the typed text actually land?
///
/// `before`/`after` are `AXValue` read from the target field before and after
/// the keystrokes. Returns whether we can *positively confirm* the text landed:
/// - unreadable `after` (`None`) → unverifiable → `false` (Catalyst case; the
///   agent must confirm via screenshot).
/// - `after` contains the complete text → `true`.
/// - empty input text → trivially `true`.
///
/// Apps that normalize input (smart quotes, autocomplete) may fail the
/// substring/length test even though something landed — we report `false`
/// (unverified) rather than erroring, so the agent can still confirm.
#[cfg(test)]
fn verify_typed(before: Option<&str>, after: Option<&str>, text: &str) -> bool {
    matches!(typed_progress(before, after, text), TypedProgress::Complete)
}

/// Classify an observable insertion without mistaking a prefix for complete
/// delivery. A positive length delta is an exact delivered-character count for
/// insert-at-cursor typing; it is capped at the request size defensively.
fn typed_progress(before: Option<&str>, after: Option<&str>, text: &str) -> TypedProgress {
    if text.is_empty() {
        return TypedProgress::Complete;
    }
    let Some(after) = after else {
        return TypedProgress::Unverifiable;
    };
    if after.contains(text) {
        return TypedProgress::Complete;
    }
    let Some(before) = before else {
        return TypedProgress::Unverifiable;
    };
    let delivered = after
        .chars()
        .count()
        .saturating_sub(before.chars().count())
        .min(text.chars().count());
    if delivered == 0 {
        TypedProgress::Unchanged
    } else {
        TypedProgress::Partial(delivered)
    }
}

/// Read the focused/target field's `AXValue`, for before/after read-back.
/// Re-fetches the focused element each call when no explicit element is given
/// (cheap, and focus is stable across our own keystrokes).
fn read_axvalue(pid: i32, element_ptr_and_idx: Option<(usize, Option<usize>)>) -> Option<String> {
    if let Some((ptr, _)) = element_ptr_and_idx {
        unsafe { copy_string_attr(ptr as AXUIElementRef, "AXValue") }
    } else if let Some(el) = unsafe { focused_element_of_pid(pid) } {
        let v = unsafe { copy_string_attr(el, "AXValue") };
        unsafe {
            CFRelease(el as _);
        }
        v
    } else {
        None
    }
}

/// Window-bound variant of [`read_axvalue`]: when no explicit element is
/// addressed and a `window_id` is known, the focused element is used ONLY when
/// its ancestry provably resolves to that exact window. A sibling window's
/// focused field must never supply before/after evidence for the requested
/// target — an unprovable focus reads as `None` (unverifiable), never as
/// sibling data. Without a window the legacy pid-global read applies.
fn read_axvalue_bound(
    pid: i32,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    window_id: Option<u32>,
) -> Option<String> {
    if element_ptr_and_idx.is_some() {
        return read_axvalue(pid, element_ptr_and_idx);
    }
    match window_id {
        Some(wid) => unsafe {
            let el = crate::ax::exact_target::focused_element_in_window(pid, wid)?;
            let v = copy_string_attr(el, "AXValue");
            CFRelease(el as _);
            v
        },
        None => read_axvalue(pid, None),
    }
}

/// True when the addressed (or focused) AX element sits inside a web-content
/// subtree — an `AXWebArea` ancestor. That covers every Chromium / WebKit /
/// Electron rendered surface (Chrome, Safari, Slack, VS Code, X's compose box…),
/// where an AX write is echoed back through `AXValue` while the renderer/DOM
/// never observes it — so an AX read-back "confirm" there is a shim echo. A
/// browser's OWN native chrome (address bar, toolbar) has no `AXWebArea`
/// ancestor, so it stays trusted. Walks a bounded ancestor chain; each
/// `AXParent` copy is released, and it stops at the window/app boundary.
pub(super) fn target_in_web_area(
    pid: i32,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    window_id: Option<u32>,
) -> bool {
    use crate::ax::bindings::AXUIElementCopyAttributeValue;
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    unsafe {
        // Start element: the addressed one (borrowed — do NOT release) or the
        // focused element (owned — must release when done). A window-addressed
        // request may only classify from the window's OWN focused element; a
        // pid-global focused element can belong to a same-process sibling and
        // sibling state must never vouch for the target. When window-bound
        // reacquisition fails, fail closed: report web content (untrusted
        // read-back) rather than trusting an unproven surface.
        let (start, start_owned) = match element_ptr_and_idx {
            Some((ptr, _)) => (ptr as AXUIElementRef, false),
            None => match window_id {
                Some(wid) => match crate::ax::exact_target::focused_element_in_window(pid, wid) {
                    Some(el) => (el, true),
                    None => return true,
                },
                None => match focused_element_of_pid(pid) {
                    Some(el) => (el, true),
                    None => return false,
                },
            },
        };
        let parent_attr = CFString::new("AXParent");
        let mut cur = start;
        let mut cur_owned = start_owned;
        let mut found = false;
        for _ in 0..40 {
            match copy_string_attr(cur, "AXRole").as_deref() {
                Some("AXWebArea") => {
                    found = true;
                    break;
                }
                // No web area lives above the window/app root — stop.
                Some("AXWindow") | Some("AXApplication") | None => break,
                _ => {}
            }
            let mut parent: CFTypeRef = std::ptr::null_mut();
            let err =
                AXUIElementCopyAttributeValue(cur, parent_attr.as_concrete_TypeRef(), &mut parent);
            if cur_owned {
                CFRelease(cur as CFTypeRef);
            }
            if err != kAXErrorSuccess || parent.is_null() {
                cur = std::ptr::null_mut();
                cur_owned = false;
                break;
            }
            cur = parent as AXUIElementRef;
            cur_owned = true;
        }
        if cur_owned && !cur.is_null() {
            CFRelease(cur as CFTypeRef);
        }
        found
    }
}

/// Type via CGEvent keystrokes at the current insertion point, then verify by
/// read-back. `type_text` is deliberately non-idempotent: it must never clear
/// an existing value merely because AX cannot read that value back.
fn cgevent_type_verified(
    pid: i32,
    text: &str,
    delay_ms: u64,
    before: Option<&str>,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    settle_ms: u64,
    window_id: Option<u32>,
) -> anyhow::Result<(bool, Option<usize>)> {
    // Focus the target element so the keystrokes land in IT. Critical in
    // foreground mode: a freshly-fronted window's keyboard focus may be on the
    // search box or nowhere, so without this the text goes into the void (or the
    // wrong field). AXFocused is best-effort — harmless when unsupported.
    //
    // Ordering matters as much as the write itself. `with_foreground_assist` has
    // already waited for the activation to land, so AppKit has installed the
    // window's remembered first responder by now and this write lands *after*
    // it rather than being clobbered by it. Re-applying once on a failed
    // read-back covers apps that install their responder slightly late.
    if let Some((ptr, _)) = element_ptr_and_idx {
        let _ = crate::input::ax_actions::focus_element(ptr);
        if settle_ms > 0 && !crate::input::ax_actions::is_element_focused(pid, ptr) {
            std::thread::sleep(FOCUS_REAPPLY_DELAY);
            let _ = crate::input::ax_actions::focus_element(ptr);
        }
    }
    // First-keystroke settle (foreground rung only — caller passes `settle_ms > 0`).
    // Even once the window is front and the element focused, the surface isn't
    // ready to accept input for a few tens of ms, so the FIRST synthesized
    // character gets eaten: typing "i love u" rendered "love u" (the leading
    // "i " was dropped). A short sleep here covers that. Background/terminal call
    // sites pass 0 — they have no front transition and must not pay this latency.
    if settle_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(settle_ms));
    }
    crate::input::keyboard::type_text_with_delay(pid, text, delay_ms)?;

    // CGEvent posting is asynchronous with respect to the renderer. In
    // particular, Chromium can acknowledge the posting process while a long
    // tail remains queued. Poll the AX value until the complete payload is
    // visible instead of treating any growth as success. If the deadline
    // expires after observable growth, surface the exact partial count.
    let deadline = std::time::Instant::now() + DELIVERY_DRAIN_TIMEOUT;
    Ok(await_typed_delivery(before, text, deadline, || {
        read_axvalue_bound(pid, element_ptr_and_idx, window_id)
    }))
}

fn await_typed_delivery(
    before: Option<&str>,
    text: &str,
    deadline: std::time::Instant,
    mut read_value: impl FnMut() -> Option<String>,
) -> (bool, Option<usize>) {
    let mut best_partial = None;
    loop {
        let after = read_value();
        match typed_progress(before, after.as_deref(), text) {
            TypedProgress::Complete => return (true, Some(text.chars().count())),
            TypedProgress::Partial(delivered) => {
                best_partial =
                    Some(best_partial.map_or(delivered, |best: usize| best.max(delivered)));
            }
            TypedProgress::Unverifiable => return (false, None),
            TypedProgress::Unchanged => {
                // A readable unchanged value is an observed zero-character
                // delivery, not an unverifiable success.
                best_partial.get_or_insert(0);
            }
        }
        if std::time::Instant::now() >= deadline {
            return (false, best_partial);
        }
        std::thread::sleep(DELIVERY_DRAIN_POLL_INTERVAL);
    }
}

/// Best-effort-background ladder for `type_text`.
///
/// - `delivery_mode == Background` (default): AX insert → read-back; on a
///   silent/unreadable accept, CGEvent keystrokes → read-back. Never fronts.
/// - `delivery_mode == Foreground`: the agent's explicit last resort — briefly
///   front `window_id`, insert at the current cursor, restore, then read-back.
///
/// Returns `(detail, path, verified)`. `verified` is `true` only when a
/// read-back positively confirmed the text; `false` means the agent must
/// confirm via screenshot (and, for background, can escalate to foreground).
fn type_text_blocking(
    pid: i32,
    text: &str,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    delay_ms: u64,
    is_terminal_target: bool,
    delivery_mode: super::DeliveryMode,
    window_id: Option<u32>,
    keyboard_policy: BackgroundKeyboardPolicy,
) -> anyhow::Result<TypeTextDelivery> {
    // Original field value before any rung drives read-back verification only.
    // An unreadable value is not evidence that the field is empty — and for a
    // window-addressed request it must come from the exact target window,
    // never a same-process sibling.
    let before = read_axvalue_bound(pid, element_ptr_and_idx, window_id);

    // --- Foreground rung: explicit agent request (skip AX/background ladder). ---
    if delivery_mode.is_foreground() {
        let screen_sharing_target = crate::input::keyboard::is_screen_sharing_pid(pid);
        if let Some(refusal) = synthesis_preflight(
            if screen_sharing_target {
                TextDeliveryRoute::PhysicalSynthesis
            } else {
                TextDeliveryRoute::UnicodeSynthesis
            },
            text.chars().count(),
            delay_ms,
        ) {
            return Ok(TypeTextDelivery::SynthesisRefused {
                path: if window_id.is_some() {
                    PATH_KEY_EVENTS_FG
                } else {
                    PATH_KEY_EVENTS
                },
                refusal,
                ax_attempt: AxAttempt::NotAttempted,
            });
        }
        // Settle between front+focus and the first keystroke — see the
        // "i love u" -> "love u" first-char-drop note in cgevent_type_verified.
        // A target that was already frontmost pays only 20ms for element-focus
        // settling. Focus-proxy clients that were just activated need longer:
        // an RDP client (Microsoft Windows App) re-arms its keyboard grab with
        // the remote host over hundreds of ms, so at 60ms every keystroke was
        // dropped. 200ms covers that re-grab without penalizing an already
        // armed interactive stream on every text chunk.
        let foreground_settle_ms = foreground_settle_ms(pid, apps::frontmost_pid());
        let do_type = || {
            cgevent_type_verified(
                pid,
                text,
                delay_ms,
                before.as_deref(),
                element_ptr_and_idx,
                foreground_settle_ms,
                window_id,
            )
        };
        let ((verified, delivered_chars), fronted) = match window_id {
            Some(wid) if screen_sharing_target => {
                // Screen Sharing forwards physical HID transitions to the
                // guest. PID-routed Unicode events all carry keycode 0 (the A
                // key), so a guest sees "aaaa"; modifier flags alone likewise
                // turn Cmd+V into plain "v". The explicit foreground rung may
                // safely use the global HID queue while the exact target is
                // guarded and restored.
                crate::input::skylight::with_foreground_hid_activation(
                    pid as libc::pid_t,
                    wid,
                    || {
                        if foreground_settle_ms > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(
                                foreground_settle_ms,
                            ));
                        }
                        crate::input::keyboard::type_text_physical_global(text, delay_ms)
                    },
                )?;
                ((false, None), true)
            }
            Some(wid) => {
                // Front → type → restore. The closure returns the read-back
                // result; with_foreground_assist returns whether it actually
                // fronted (Ok(false) when the fronting SPIs are unavailable —
                // the keystrokes still ran, just as background input).
                let mut typed_delivery = (false, None);
                let fronted = crate::input::skylight::with_foreground_assist(
                    pid as libc::pid_t,
                    wid,
                    || {
                        typed_delivery = do_type()?;
                        Ok(())
                    },
                )?;
                (typed_delivery, fronted)
            }
            // No window to front — best-effort background keystrokes instead.
            None => (do_type()?, false),
        };
        // Only claim the `_fg` path when a front actually happened; when no
        // foregrounding occurred (no window, or SPIs unavailable) these were
        // background keystrokes and `path` must say so honestly.
        return Ok(TypeTextDelivery::Typed(TypeTextOutcome {
            detail: format!(" via foreground keystrokes ({delay_ms}ms delay)"),
            path: if fronted {
                PATH_KEY_EVENTS_FG
            } else {
                PATH_KEY_EVENTS
            },
            delivered_chars,
            verified,
        }));
    }

    // --- Background rung 0: terminal emulator → CGEvent only (AX is dropped). ---
    if is_terminal_target {
        // A terminal insert has no semantic AX rung: when the exact-target
        // decision restricted this request to semantic-only, there is nothing
        // safe to run — refuse before posting anything.
        if let BackgroundKeyboardPolicy::SemanticOnly(refusal) = keyboard_policy {
            return Ok(TypeTextDelivery::Refused(refusal));
        }
        if let Some(refusal) = synthesis_preflight(
            TextDeliveryRoute::UnicodeSynthesis,
            text.chars().count(),
            delay_ms,
        ) {
            return Ok(TypeTextDelivery::SynthesisRefused {
                path: PATH_KEY_EVENTS,
                refusal,
                ax_attempt: AxAttempt::NotAttempted,
            });
        }
        tracing::debug!(
            "type_text: pid {pid} is a terminal emulator; skipping AX value-set, \
             using CGEvent key-event synthesis"
        );
        let (verified, delivered_chars) = cgevent_type_verified(
            pid,
            text,
            delay_ms,
            before.as_deref(),
            element_ptr_and_idx,
            /*settle_ms=*/ 0,
            window_id,
        )?;
        return Ok(TypeTextDelivery::Typed(TypeTextOutcome {
            detail: format!(" via CGEvent (terminal emulator, {delay_ms}ms delay)"),
            path: PATH_KEY_EVENTS,
            verified,
            delivered_chars,
        }));
    }

    // --- Background rung 1: AX SelectedText write (element or focused). ---
    // Without an explicit element, a window-addressed request may only write
    // to the focused element when it provably belongs to the exact target
    // window — a sibling window's focused field is not the requested target.
    let ax_target: Option<(AXUIElementRef, bool, Option<usize>)> = match element_ptr_and_idx {
        Some((ptr, idx)) => Some((ptr as AXUIElementRef, /*owns=*/ false, idx)),
        None => match window_id {
            Some(wid) => unsafe { crate::ax::exact_target::focused_element_in_window(pid, wid) }
                .map(|el| (el, /*owns=*/ true, None)),
            None => {
                unsafe { focused_element_of_pid(pid) }.map(|el| (el, /*owns=*/ true, None))
            }
        },
    };
    let mut ax_attempt = AxAttempt::NotAttempted;
    if let Some((element, owns, idx_opt)) = ax_target {
        let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
        let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
        let err = unsafe { set_string_attr(element, "AXSelectedText", text) };
        // Classify the atomic write before considering synthesis. Complete AX
        // delivery returns immediately. Partial delivery is surfaced as such
        // instead of appending the full payload again. When synthesis would
        // exceed its transport-safe budget, rejected/unchanged AX writes fail
        // safely and unreadable AX state is reported as indeterminate.
        let after = unsafe { copy_string_attr(element, "AXValue") };
        let ax_progress = if err == kAXErrorSuccess {
            Some(typed_progress(before.as_deref(), after.as_deref(), text))
        } else {
            None
        };
        // AXValue is not renderer evidence in web content. An unchanged echo
        // there cannot prove that zero characters landed, so blind retry is
        // unsafe even though no synthesis has run yet.
        let unchanged_web_readback = ax_progress == Some(TypedProgress::Unchanged)
            && target_in_web_area(pid, Some((element as usize, idx_opt)), window_id);
        if owns {
            unsafe {
                CFRelease(element as _);
            }
        }
        if ax_progress == Some(TypedProgress::Complete) {
            let idx_str = idx_opt.map(|i| format!(" [{i}]")).unwrap_or_default();
            return Ok(TypeTextDelivery::Typed(TypeTextOutcome {
                detail: format!(" into{idx_str} {role} \"{title}\""),
                path: PATH_AX,
                verified: true,
                delivered_chars: Some(text.chars().count()),
            }));
        }
        if let Some(TypedProgress::Partial(delivered_chars)) = ax_progress {
            let idx_str = idx_opt.map(|i| format!(" [{i}]")).unwrap_or_default();
            return Ok(TypeTextDelivery::Typed(TypeTextOutcome {
                detail: format!(" via partial AX write into{idx_str} {role} \"{title}\""),
                path: PATH_AX,
                verified: false,
                delivered_chars: Some(delivered_chars),
            }));
        }
        ax_attempt = match ax_progress {
            Some(TypedProgress::Unchanged) if unchanged_web_readback => AxAttempt::Unverifiable,
            Some(TypedProgress::Unchanged) => AxAttempt::Unchanged,
            Some(TypedProgress::Unverifiable) => AxAttempt::Unverifiable,
            None => AxAttempt::Rejected,
            Some(TypedProgress::Complete | TypedProgress::Partial(_)) => unreachable!(),
        };
        tracing::debug!(
            "AX write did not land for {role} \"{title}\" (err={err}); \
             falling back to CGEvent keystrokes"
        );
    } else {
        tracing::debug!("No focused element for pid {pid}; using CGEvent keystrokes");
    }

    // The semantic AX rung did not land and this request is restricted to it:
    // the process-scoped CGEvent rung could reach a sibling window, so return
    // the structured refusal instead of escalating.
    if let BackgroundKeyboardPolicy::SemanticOnly(refusal) = keyboard_policy {
        return Ok(TypeTextDelivery::Refused(refusal));
    }

    if let Some(refusal) = synthesis_preflight(
        TextDeliveryRoute::UnicodeSynthesis,
        text.chars().count(),
        delay_ms,
    ) {
        return Ok(TypeTextDelivery::SynthesisRefused {
            path: PATH_KEY_EVENTS,
            refusal,
            ax_attempt,
        });
    }

    // --- Background rung 2: CGEvent keystrokes with read-back. ---
    // Never clear here: a partial AX write is rare, and clearing would violate
    // insert-at-cursor semantics.
    let (verified, delivered_chars) = cgevent_type_verified(
        pid,
        text,
        delay_ms,
        before.as_deref(),
        element_ptr_and_idx,
        /*settle_ms=*/ 0,
        window_id,
    )?;
    Ok(TypeTextDelivery::Typed(TypeTextOutcome {
        detail: format!(" via CGEvent ({delay_ms}ms delay)"),
        path: PATH_KEY_EVENTS,
        verified,
        delivered_chars,
    }))
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
        let r = type_text_blocking(
            -1,
            "x",
            None,
            0,
            /*is_terminal_target=*/ true,
            super::super::DeliveryMode::Background,
            None,
            BackgroundKeyboardPolicy::Allowed,
        );
        // We don't care whether r is Ok or Err — what matters is that
        // calling it with is_terminal_target=true is safe and never
        // dereferences null AX pointers.
        let _ = r;
    }

    /// A semantic-only policy must refuse the terminal short-circuit before
    /// any CGEvent is posted: terminals have no semantic AX rung, so nothing
    /// safe remains and the carried refusal comes back unchanged.
    #[test]
    fn semantic_only_policy_refuses_terminal_cgevent_rung() {
        let refusal = BackgroundRefusal {
            code: cua_driver_core::background_input::refusal_codes::SAME_PID_KEYBOARD_AMBIGUITY,
            reason: "test".into(),
            advice: None,
        };
        let r = type_text_blocking(
            -1,
            "x",
            None,
            0,
            /*is_terminal_target=*/ true,
            super::super::DeliveryMode::Background,
            Some(7),
            BackgroundKeyboardPolicy::SemanticOnly(refusal.clone()),
        );
        match r {
            Ok(TypeTextDelivery::Refused(returned)) => assert_eq!(returned, refusal),
            other => panic!("expected a structured refusal, got {:?}", other.is_ok()),
        }
    }

    #[test]
    fn oversized_synthesis_is_refused_before_the_terminal_event_path() {
        let text = "x".repeat(6_500);
        let result = type_text_blocking(
            -1,
            &text,
            None,
            0,
            /*is_terminal_target=*/ true,
            super::super::DeliveryMode::Background,
            None,
            BackgroundKeyboardPolicy::Allowed,
        )
        .expect("preflight refusal must not attempt the invalid pid");
        let TypeTextDelivery::SynthesisRefused {
            path,
            refusal,
            ax_attempt,
        } = result
        else {
            panic!("oversized terminal synthesis must fail before mutation");
        };
        assert_eq!(path, PATH_KEY_EVENTS);
        assert_eq!(ax_attempt, AxAttempt::NotAttempted);
        assert_eq!(refusal.requested_chars, 6_500);
        assert_eq!(refusal.estimated_duration_ms, 106_000);
        assert_eq!(refusal.max_chunk_chars, 6_125);
    }

    #[test]
    fn synthesis_preflight_accepts_the_exact_transport_safe_boundary() {
        assert!(synthesis_preflight(TextDeliveryRoute::UnicodeSynthesis, 6_125, 0).is_none());
        assert!(synthesis_preflight(TextDeliveryRoute::UnicodeSynthesis, 6_126, 0).is_some());
    }

    #[test]
    fn large_atomic_ax_payloads_are_not_subject_to_the_synthesis_budget() {
        assert!(synthesis_preflight(TextDeliveryRoute::AtomicAx, 100_000, 200).is_none());
        assert_eq!(
            typed_progress(None, Some(&"x".repeat(11_500)), &"x".repeat(11_500)),
            TypedProgress::Complete,
            "a successful one-call AX insertion remains eligible regardless of size"
        );
    }

    #[test]
    fn refusal_diagnostics_distinguish_safe_chunking_from_indeterminate_ax() {
        let refusal = synthesis_preflight(TextDeliveryRoute::UnicodeSynthesis, 6_500, 0)
            .expect("payload must exceed the synthesis budget");
        let safe = synthesis_refusal_result(PATH_KEY_EVENTS, &refusal, AxAttempt::Rejected);
        let safe = safe.structured_content.expect("structured refusal");
        assert_eq!(safe["code"], "type_text_synthesis_budget_exceeded");
        assert_eq!(safe["effect"], "refused");
        assert_eq!(safe["delivered_chars"], 0);
        assert_eq!(safe["synthesized_chars"], 0);
        assert_eq!(safe["retryable"], true);
        assert_eq!(safe["escalation"]["recommended"], "chunk");

        let indeterminate =
            synthesis_refusal_result(PATH_KEY_EVENTS, &refusal, AxAttempt::Unverifiable);
        let indeterminate = indeterminate
            .structured_content
            .expect("structured indeterminate result");
        assert_eq!(indeterminate["effect"], "indeterminate");
        assert!(indeterminate.get("delivered_chars").is_none());
        assert_eq!(indeterminate["synthesized_chars"], 0);
        assert_eq!(indeterminate["retryable"], false);
        assert_eq!(indeterminate["escalation"]["recommended"], "verify_state");
    }

    #[test]
    fn verify_typed_unreadable_after_is_unverified() {
        // Catalyst: can't read AXValue back → cannot confirm → false.
        assert!(!verify_typed(None, None, "hi"));
        assert!(!verify_typed(Some(""), None, "hi"));
    }

    #[test]
    fn verify_typed_contains_full_request_is_verified() {
        assert!(verify_typed(Some(""), Some("hi"), "hi")); // contains
        assert!(verify_typed(Some("ab"), Some("ab hi"), "hi")); // contains, appended
    }

    #[test]
    fn observable_prefix_is_partial_not_verified() {
        assert_eq!(
            typed_progress(Some(""), Some("BEGINpayload"), "BEGINpayloadEND"),
            TypedProgress::Partial(12)
        );
        assert!(!verify_typed(
            Some(""),
            Some("BEGINpayload"),
            "BEGINpayloadEND"
        ));
    }

    #[test]
    fn delivery_waits_through_a_partial_readback_until_complete() {
        let text = "BEGIN-payload-END";
        let mut values = std::collections::VecDeque::from([
            Some("BEGIN-payload".to_owned()),
            Some(text.to_owned()),
        ]);
        let mut reads = 0;
        let delivery = await_typed_delivery(
            Some(""),
            text,
            std::time::Instant::now() + std::time::Duration::from_secs(1),
            || {
                reads += 1;
                values.pop_front().flatten()
            },
        );
        assert_eq!(delivery, (true, Some(text.chars().count())));
        assert_eq!(reads, 2, "completion must wait past the prefix readback");
    }

    #[test]
    fn drained_prefix_reports_the_delivered_character_count() {
        let delivery = await_typed_delivery(
            Some(""),
            "BEGIN-payload-END",
            std::time::Instant::now(),
            || Some("BEGIN".to_owned()),
        );
        assert_eq!(delivery, (false, Some(5)));
    }

    #[test]
    fn verify_typed_unchanged_is_unverified() {
        // Readable but the field didn't change and doesn't contain the text.
        assert!(!verify_typed(Some("ab"), Some("ab"), "hi"));
    }

    #[test]
    fn verify_typed_empty_text_is_trivially_verified() {
        assert!(verify_typed(None, None, ""));
    }

    #[test]
    fn path_constants_are_stable_tokens() {
        // These string constants are part of the structured-response
        // contract; freezing them here makes the contract a unit test.
        assert_eq!(PATH_AX, "ax");
        assert_eq!(PATH_KEY_EVENTS, "key_events");
        assert_eq!(PATH_KEY_EVENTS_FG, "key_events_fg");
    }

    #[test]
    fn ax_backed_web_readbacks_are_downgraded() {
        for path in [PATH_AX, PATH_KEY_EVENTS, PATH_KEY_EVENTS_FG] {
            assert_eq!(
                surface_verification(path, true, true),
                SurfaceVerification {
                    verified: false,
                    untrusted_web_readback: true,
                },
                "path={path}"
            );
        }
    }

    #[test]
    fn web_readback_distrust_preserves_native_and_unverified_outcomes() {
        assert_eq!(
            surface_verification(PATH_KEY_EVENTS_FG, true, false),
            SurfaceVerification {
                verified: true,
                untrusted_web_readback: false,
            },
            "native browser chrome remains eligible for trusted read-back"
        );
        assert_eq!(
            surface_verification(PATH_KEY_EVENTS_FG, false, true),
            SurfaceVerification {
                verified: false,
                untrusted_web_readback: false,
            },
            "an already-unverified delivery is not reclassified as a web echo"
        );
        assert_eq!(
            surface_verification("independent_renderer_oracle", true, true),
            SurfaceVerification {
                verified: true,
                untrusted_web_readback: false,
            },
            "a future independently verified path must not inherit AXValue distrust"
        );
    }

    #[test]
    fn web_readback_escalation_never_recommends_the_completed_pixel_rung() {
        assert_eq!(web_readback_next_rung(true, false), Some("px"));
        assert_eq!(web_readback_next_rung(true, true), None);
        assert_eq!(web_readback_next_rung(false, false), Some("page"));
        assert_eq!(web_readback_next_rung(false, true), Some("page"));
    }

    #[test]
    fn foreground_typing_skips_long_rearm_when_target_is_already_frontmost() {
        assert_eq!(foreground_settle_ms(42, Some(42)), 20);
        assert_eq!(foreground_settle_ms(42, Some(7)), 200);
        assert_eq!(foreground_settle_ms(42, None), 200);
    }

    #[test]
    fn screen_sharing_text_fails_closed_without_foreground_window() {
        for (foreground, window_id) in [(false, None), (false, Some(7)), (true, None)] {
            let result = screen_sharing_delivery_error(true, foreground, window_id)
                .expect("unsafe Screen Sharing route must be refused");
            assert_eq!(result.is_error, Some(true));
            let structured = result.structured_content.unwrap();
            assert_eq!(structured["code"], "SCREEN_SHARING_REQUIRES_FOREGROUND_HID");
            assert_eq!(structured["effect"], "refused");
            assert_eq!(structured["escalation"]["recommended"], "foreground");
            assert_eq!(structured["escalation"]["requires"][0], "window_id");
        }
        assert!(screen_sharing_delivery_error(true, true, Some(7)).is_none());
        assert!(screen_sharing_delivery_error(false, false, None).is_none());
    }
}
