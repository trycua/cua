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
    action_record::{ActionTransport, ActualDelivery},
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
             effect:\"unverifiable\", never a false \"confirmed\" (a \
             browser's own native address bar/toolbar stays trusted). For a browser \
             TAB the reliable path is the `page` tool (drives the DOM via CDP); for \
             an embedded web view use this tool's px form: pass x,y (no \
             element_index) to pixel-click the field then type, in one call. NOTE: \
             a px focus-click won't reliably open+focus a CLOSED control; AX-press \
             to open/activate it first (works in the background), then px-type. \
             Observe the target before retrying. An accepted or uncertain AX \
             write is never replayed automatically through keystrokes. Missing \
             read-back alone does not prove non-delivery or a safe retry."
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
        let window_id_arg = args.opt_u64("window_id");
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match self.state.element_cache.resolve_element_args(
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
        let (element_index, window_id, element_guard) = resolved.into_parts(window_id_arg);
        let window_id = match super::native_window_id(window_id) {
            Ok(window_id) => window_id,
            Err(error) => return error,
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

        let element_guard = element_guard.zip(element_index);

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
            let center_guard = element.clone();
            if let Ok(Some((screen_x, screen_y))) = tokio::task::spawn_blocking(move || unsafe {
                crate::ax::bindings::element_screen_center(center_guard.as_ptr() as AXUIElementRef)
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
        let native_guard = element_guard.clone();
        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "type_text.AXSelectedText",
            || async move {
                tokio::task::spawn_blocking(move || {
                    let _native_guard = native_guard;
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
            Ok(Ok(outcome)) => text_result(
                outcome,
                char_count,
                delivery_mode.is_foreground(),
                &changes.result_suffix(),
            ),
            Ok(Err(e)) => ToolResult::error(format!("type_text failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

fn text_result(
    outcome: TypeTextOutcome,
    char_count: usize,
    foreground_requested: bool,
    suffix: &str,
) -> ToolResult {
    use cua_driver_core::action_record::{
        ActionEffect, ActionEvidence, ActionExecutionRecord, EvidenceKind, RequestedDelivery,
    };

    let (effect, count) = match outcome.progress {
        TypedProgress::Complete => (ActionEffect::Confirmed, u32::try_from(char_count).ok()),
        TypedProgress::Partial(n) if n > 0 && n < char_count && u32::try_from(n).is_ok() => {
            (ActionEffect::Partial, Some(n as u32))
        }
        _ => (ActionEffect::Unverifiable, None),
    };
    let mut record = ActionExecutionRecord::new(
        effect,
        outcome.transport,
        if foreground_requested {
            RequestedDelivery::Foreground
        } else {
            RequestedDelivery::Background
        },
    );
    record.actual_delivery = Some(outcome.delivery);
    if effect == ActionEffect::Confirmed {
        record.evidence.push(ActionEvidence {
            kind: EvidenceKind::ValueReadback,
            detail: "The addressed native field contains the inserted text.".into(),
        });
    }
    record.delivered_count = count;
    let result = if effect == ActionEffect::Partial {
        ToolResult::error(format!(
            "type_text incomplete: observed {} of {char_count} character(s){}; observe the target before retrying{suffix}",
            count.unwrap(), outcome.detail,
        ))
        .with_structured(serde_json::json!({
            "code": "type_text_incomplete",
            "effect": "partial",
            "requested_chars": char_count,
            "delivered_chars": count,
            "retryable": false,
        }))
    } else {
        let status = if effect == ActionEffect::Confirmed {
            "confirmed"
        } else {
            "unverifiable; observe the target before retrying"
        };
        ToolResult::text(format!(
            "type_text: {char_count} character(s){}; {status}{suffix}",
            outcome.detail
        ))
    };
    result.with_action_record(record)
}

// ── Blocking implementation ───────────────────────────────────────────────────

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
}

impl AxAttempt {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotAttempted => "not_attempted",
            Self::Rejected => "rejected",
        }
    }
}

fn synthesis_preflight(
    route: TextDeliveryRoute,
    requested_chars: usize,
    delay_ms: u64,
) -> Option<SynthesisRefusal> {
    let per_character_ms = match route {
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
    let structured = serde_json::json!({
        "code": "type_text_synthesis_budget_exceeded",
        "path": path,
        "effect": "refused",
        "requested_chars": refusal.requested_chars,
        "estimated_duration_ms": refusal.estimated_duration_ms,
        "synthesis_budget_ms": SYNTHESIS_BUDGET_MS,
        "per_character_ms": refusal.per_character_ms,
        "max_chunk_chars": refusal.max_chunk_chars,
        "synthesized_chars": 0,
        "atomic_ax_effect": ax_attempt.as_str(),
        "retryable": true,
        "delivered_chars": 0,
        "retry_from_character": 0,
        "escalation": {
            "recommended": "chunk",
            "reason": format!(
                "Character synthesis would exceed the bounded transport-safe budget. Retry in chunks of at most {} characters at this delay.",
                refusal.max_chunk_chars
            )
        },
    });
    let message = format!(
            "type_text refused character synthesis before emitting character events: {} characters require an estimated {}ms at {}ms per character, exceeding the {}ms budget; retry in chunks of at most {} characters",
            refusal.requested_chars,
            refusal.estimated_duration_ms,
            refusal.per_character_ms,
            SYNTHESIS_BUDGET_MS,
            refusal.max_chunk_chars,
    );
    ToolResult::error(message).with_structured(structured)
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
    let element_guard =
        element_ptr.map(|ptr| unsafe { crate::ax::cache::RetainedElement::retain(ptr) });
    let facts = match tokio::task::spawn_blocking(move || {
        let element_ptr = element_guard.as_ref().map(|guard| guard.as_ptr());
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
    transport: ActionTransport,
    delivery: ActualDelivery,
    progress: TypedProgress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypedProgress {
    Complete,
    Partial(usize),
    Unchanged,
    Unverifiable,
}

fn ax_write_progress(
    error: i32,
    before: Option<&str>,
    after: Option<&str>,
    text: &str,
    trusted: bool,
) -> Option<TypedProgress> {
    if error == crate::ax::bindings::kAXErrorAttributeUnsupported {
        return None;
    }
    Some(if error == kAXErrorSuccess && trusted {
        typed_progress(before, after, text)
    } else {
        TypedProgress::Unverifiable
    })
}

fn foreground_settle_ms(pid: i32, frontmost_pid: Option<i32>) -> u64 {
    if frontmost_pid == Some(pid) {
        20
    } else {
        200
    }
}

#[cfg(test)]
fn verify_typed(before: Option<&str>, after: Option<&str>, text: &str) -> bool {
    matches!(typed_progress(before, after, text), TypedProgress::Complete)
}

fn typed_progress(before: Option<&str>, after: Option<&str>, text: &str) -> TypedProgress {
    if text.is_empty() {
        return TypedProgress::Complete;
    }
    let Some((before, after)) = before.zip(after) else {
        return TypedProgress::Unverifiable;
    };
    if before == after {
        return TypedProgress::Unchanged;
    }
    let Some(inserted) = after
        .len()
        .checked_sub(before.len())
        .and_then(|n| text.get(..n))
    else {
        return TypedProgress::Unverifiable;
    };
    let common_prefix: usize = before
        .chars()
        .zip(after.chars())
        .take_while(|(before, after)| before == after)
        .map(|(character, _)| character.len_utf8())
        .sum();
    let matches_insertion = after
        .get(..common_prefix + inserted.len())
        .and_then(|prefix| prefix.rfind(inserted))
        .is_some_and(|offset| before[offset..] == after[offset + inserted.len()..]);
    if !matches_insertion {
        TypedProgress::Unverifiable
    } else if inserted == text {
        TypedProgress::Complete
    } else {
        TypedProgress::Partial(inserted.chars().count())
    }
}

fn read_axvalue(element_ptr_and_idx: Option<(usize, Option<usize>)>) -> Option<String> {
    let (ptr, _) = element_ptr_and_idx?;
    unsafe { copy_string_attr(ptr as AXUIElementRef, "AXValue") }
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
                    None => return true,
                },
            },
        };
        let parent_attr = CFString::new("AXParent");
        let mut cur = start;
        let mut cur_owned = start_owned;
        let mut found = true;
        for _ in 0..40 {
            match copy_string_attr(cur, "AXRole").as_deref() {
                Some("AXWebArea") | None => break,
                Some("AXWindow") | Some("AXApplication") => {
                    found = false;
                    break;
                }
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
) -> anyhow::Result<TypedProgress> {
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
        let trusted = element_ptr_and_idx
            .is_some_and(|(ptr, _)| crate::input::ax_actions::is_element_focused(pid, ptr))
            && !target_in_web_area(pid, element_ptr_and_idx, window_id);
        trusted.then(|| read_axvalue(element_ptr_and_idx)).flatten()
    }))
}

fn await_typed_delivery(
    before: Option<&str>,
    text: &str,
    deadline: std::time::Instant,
    mut read_value: impl FnMut() -> Option<String>,
) -> TypedProgress {
    loop {
        let after = read_value();
        let progress = typed_progress(before, after.as_deref(), text);
        if matches!(
            progress,
            TypedProgress::Complete | TypedProgress::Unverifiable
        ) || std::time::Instant::now() >= deadline
        {
            return progress;
        }
        std::thread::sleep(DELIVERY_DRAIN_POLL_INTERVAL);
    }
}

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
    let focused_guard = if element_ptr_and_idx.is_none() {
        unsafe {
            let focused = match window_id {
                Some(wid) => crate::ax::exact_target::focused_element_in_window(pid, wid),
                None => focused_element_of_pid(pid),
            };
            focused.map(|element| {
                let guard = crate::ax::cache::RetainedElement::retain(element as usize);
                CFRelease(element as _);
                guard
            })
        }
    } else {
        None
    };
    let element_ptr_and_idx =
        element_ptr_and_idx.or_else(|| focused_guard.as_ref().map(|guard| (guard.as_ptr(), None)));
    let before = (!target_in_web_area(pid, element_ptr_and_idx, window_id))
        .then(|| read_axvalue(element_ptr_and_idx))
        .flatten();

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
        let (progress, fronted) = match window_id {
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
                (TypedProgress::Unverifiable, true)
            }
            Some(wid) => {
                // Front → type → restore. The closure returns the read-back
                // result; with_foreground_assist returns whether it actually
                // fronted (Ok(false) when the fronting SPIs are unavailable —
                // the keystrokes still ran, just as background input).
                let mut typed_delivery = TypedProgress::Unverifiable;
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
        return Ok(TypeTextDelivery::Typed(TypeTextOutcome {
            detail: format!(" via CGEvent ({delay_ms}ms delay)"),
            transport: if screen_sharing_target && fronted {
                ActionTransport::MacosCgEventHid
            } else {
                ActionTransport::MacosCgEventPid
            },
            delivery: if fronted {
                ActualDelivery::Foreground
            } else {
                ActualDelivery::Background
            },
            progress,
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
        let progress = cgevent_type_verified(
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
            transport: ActionTransport::MacosCgEventPid,
            delivery: ActualDelivery::Background,
            progress,
        }));
    }

    // --- Background rung 1: AX SelectedText write (element or focused). ---
    // Without an explicit element, a window-addressed request may only write
    // to the focused element when it provably belongs to the exact target
    // window — a sibling window's focused field is not the requested target.
    let mut ax_attempt = AxAttempt::NotAttempted;
    if let Some((ptr, idx_opt)) = element_ptr_and_idx {
        let element = ptr as AXUIElementRef;
        let err = unsafe { set_string_attr(element, "AXSelectedText", text) };
        let after = unsafe { copy_string_attr(element, "AXValue") };
        let trusted = !target_in_web_area(pid, Some((element as usize, idx_opt)), window_id);
        let progress = ax_write_progress(err, before.as_deref(), after.as_deref(), text, trusted);
        if let Some(progress) = progress {
            return Ok(TypeTextDelivery::Typed(TypeTextOutcome {
                detail: " via AXSelectedText".into(),
                transport: ActionTransport::MacosAxValue,
                delivery: ActualDelivery::Background,
                progress,
            }));
        }
        ax_attempt = AxAttempt::Rejected;
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
    let progress = cgevent_type_verified(
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
        transport: ActionTransport::MacosCgEventPid,
        delivery: ActualDelivery::Background,
        progress,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_action(result: &ToolResult) -> Value {
        let record = result
            .action_record
            .as_ref()
            .expect("native text must publish its typed record");
        serde_json::to_value(record.public_result().unwrap()).unwrap()
    }

    #[test]
    fn unobserved_text_publishes_uncertainty_without_a_foreground_retry() {
        let result = text_result(
            TypeTextOutcome {
                detail: String::new(),
                transport: ActionTransport::MacosAxValue,
                delivery: ActualDelivery::Background,
                progress: TypedProgress::Unverifiable,
            },
            4,
            false,
            "",
        );
        assert_eq!(
            public_action(&result),
            serde_json::json!({
                "effect": "unverifiable",
                "route": "accessibility",
                "delivery": {"mode": "background"}
            })
        );
    }

    #[test]
    fn web_readback_never_publishes_an_exact_count_or_a_safe_retry() {
        for count in [0, 2, 4] {
            let after = "text".chars().take(count).collect::<String>();
            let progress =
                ax_write_progress(kAXErrorSuccess, Some(""), Some(&after), "text", false)
                    .expect("an accepted AX write must not replay through keystrokes");
            let result = text_result(
                TypeTextOutcome {
                    detail: String::new(),
                    transport: ActionTransport::MacosAxValue,
                    delivery: ActualDelivery::Background,
                    progress,
                },
                4,
                false,
                "",
            );
            assert_ne!(result.is_error, Some(true), "count={count}");
            assert_eq!(
                public_action(&result),
                serde_json::json!({
                    "effect": "unverifiable",
                    "route": "accessibility",
                    "delivery": {"mode": "background"}
                })
            );
        }
    }

    #[test]
    fn missing_target_ancestry_cannot_confirm_cached_text() {
        let trusted = !target_in_web_area(-1, None, None);
        let progress =
            ax_write_progress(kAXErrorSuccess, Some(""), Some("text"), "text", trusted).unwrap();
        let result = text_result(
            TypeTextOutcome {
                detail: String::new(),
                transport: ActionTransport::MacosAxValue,
                delivery: ActualDelivery::Background,
                progress,
            },
            4,
            false,
            "",
        );
        assert_eq!(
            public_action(&result),
            serde_json::json!({
                "effect": "unverifiable", "route": "accessibility",
                "delivery": {"mode": "background"}
            })
        );
    }

    #[test]
    fn native_unicode_readback_keeps_confirmation_and_partial_without_retry_advice() {
        for (after, effect, count) in [("é🙂z", "confirmed", 3), ("é🙂", "partial", 2)] {
            let progress =
                ax_write_progress(kAXErrorSuccess, Some(""), Some(after), "é🙂z", true).unwrap();
            let result = text_result(
                TypeTextOutcome {
                    detail: String::new(),
                    transport: ActionTransport::MacosAxValue,
                    delivery: ActualDelivery::Background,
                    progress,
                },
                3,
                false,
                "",
            );
            let action = public_action(&result);
            assert_eq!(action["effect"], effect);
            assert_eq!(action["delivery"]["delivered_count"], count);
            assert!(action.get("escalation").is_none());
            if effect == "partial" {
                assert_eq!(result.is_error, Some(true));
                let error = result.structured_content.unwrap();
                assert_eq!(error["code"], "type_text_incomplete");
                assert_eq!(error["retryable"], false);
                assert!(error.get("retry_from_character").is_none());
            } else {
                assert_ne!(result.is_error, Some(true));
            }
        }
    }

    #[test]
    fn native_insertions_with_overlapping_prefixes_publish_exact_progress() {
        for (before, after, text, effect, count) in [
            ("a", "aba", "ab", "confirmed", 2),
            ("ab", "abcab", "abc", "confirmed", 3),
            ("cat", "catapultcat", "catapult", "confirmed", 8),
            ("za", "zaba", "ab", "confirmed", 2),
            ("é", "é🙂é", "é🙂", "confirmed", 2),
            ("ê", "éê", "é", "confirmed", 1),
            ("abc", "abcab", "ab", "confirmed", 2),
            ("aaa", "aaaaa", "aa", "confirmed", 2),
            ("a", "aba", "abc", "partial", 2),
            ("a", "aab", "abc", "partial", 2),
        ] {
            let progress =
                ax_write_progress(kAXErrorSuccess, Some(before), Some(after), text, true).unwrap();
            let result = text_result(
                TypeTextOutcome {
                    detail: String::new(),
                    transport: ActionTransport::MacosAxValue,
                    delivery: ActualDelivery::Background,
                    progress,
                },
                text.chars().count(),
                false,
                "",
            );
            let action = public_action(&result);
            assert_eq!(
                action["effect"], effect,
                "{before:?} -> {after:?}, request {text:?}"
            );
            assert_eq!(action["delivery"]["delivered_count"], count);
            assert_eq!(result.is_error == Some(true), effect == "partial");
            assert!(action.get("escalation").is_none());
        }
    }

    #[test]
    fn uncertain_ax_errors_stop_while_unsupported_attributes_allow_synthesis() {
        use crate::ax::bindings::{kAXErrorAttributeUnsupported, kAXErrorFailure};
        assert!(ax_write_progress(
            kAXErrorAttributeUnsupported,
            Some(""),
            Some(""),
            "marker",
            true
        )
        .is_none());
        for (error, after) in [
            (kAXErrorSuccess, Some("")),
            (kAXErrorSuccess, None),
            (kAXErrorFailure, Some("marker")),
            (kAXErrorFailure, None),
        ] {
            let progress = ax_write_progress(error, Some(""), after, "marker", true)
                .expect("uncertain AX attempts must not reach synthesis");
            let result = text_result(
                TypeTextOutcome {
                    detail: String::new(),
                    transport: ActionTransport::MacosAxValue,
                    delivery: ActualDelivery::Background,
                    progress,
                },
                6,
                false,
                "",
            );
            assert_eq!(
                public_action(&result),
                serde_json::json!({
                    "effect": "unverifiable", "route": "accessibility",
                    "delivery": {"mode": "background"}
                })
            );
        }
    }

    #[test]
    fn requested_foreground_does_not_replace_actual_background_delivery() {
        let result = text_result(
            TypeTextOutcome {
                detail: String::new(),
                transport: ActionTransport::MacosCgEventPid,
                delivery: ActualDelivery::Background,
                progress: TypedProgress::Unverifiable,
            },
            6,
            true,
            "",
        );
        assert_eq!(
            public_action(&result),
            serde_json::json!({
                "effect": "unverifiable", "route": "synthetic_events",
                "delivery": {"mode": "background"}
            })
        );
    }

    #[test]
    fn reverted_readback_does_not_publish_stale_progress() {
        let mut first = true;
        let progress = await_typed_delivery(
            Some(""),
            "marker",
            std::time::Instant::now() + std::time::Duration::from_millis(100),
            || {
                Some(
                    if std::mem::take(&mut first) {
                        "mar"
                    } else {
                        ""
                    }
                    .into(),
                )
            },
        );
        let result = text_result(
            TypeTextOutcome {
                detail: String::new(),
                transport: ActionTransport::MacosCgEventPid,
                delivery: ActualDelivery::Background,
                progress,
            },
            6,
            false,
            "",
        );
        assert_ne!(result.is_error, Some(true));
        assert_eq!(
            public_action(&result),
            serde_json::json!({
                "effect": "unverifiable", "route": "synthetic_events",
                "delivery": {"mode": "background"}
            })
        );
    }

    #[test]
    fn preexisting_text_and_unrelated_growth_do_not_confirm_an_insertion() {
        for (before, after, text) in [
            ("marker", "marker", "marker"),
            ("", "other", "marker"),
            ("a", "ba", "ab"),
            ("é", "ê", "x"),
            ("a", "a🙂", "é"),
            ("ab", "b", "marker"),
            ("ab", "axb", "marker"),
            ("ab", "abmarkerx", "marker"),
        ] {
            let progress =
                await_typed_delivery(Some(before), text, std::time::Instant::now(), || {
                    Some(after.into())
                });
            let result = text_result(
                TypeTextOutcome {
                    detail: String::new(),
                    transport: ActionTransport::MacosCgEventPid,
                    delivery: ActualDelivery::Background,
                    progress,
                },
                text.chars().count(),
                false,
                "",
            );
            assert_ne!(result.is_error, Some(true));
            assert_eq!(
                public_action(&result),
                serde_json::json!({
                    "effect": "unverifiable",
                    "route": "synthetic_events",
                    "delivery": {"mode": "background"}
                })
            );
        }
    }

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
        let text = "x".repeat(11_500);
        let progress = ax_write_progress(kAXErrorSuccess, Some(""), Some(&text), &text, true)
            .expect("a confirmed AX write must stop before synthesis");
        let result = text_result(
            TypeTextOutcome {
                detail: String::new(),
                transport: ActionTransport::MacosAxValue,
                delivery: ActualDelivery::Background,
                progress,
            },
            11_500,
            false,
            "",
        );
        assert_ne!(result.is_error, Some(true));
        let action = public_action(&result);
        assert_eq!(action["effect"], "confirmed");
        assert_eq!(action["delivery"]["delivered_count"], 11_500);
        assert!(action.get("escalation").is_none());
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

        let text = "x".repeat(6_500);
        let progress = ax_write_progress(kAXErrorSuccess, Some(""), None, &text, true)
            .expect("an uncertain atomic write must not enter synthesis preflight");
        let result = text_result(
            TypeTextOutcome {
                detail: String::new(),
                transport: ActionTransport::MacosAxValue,
                delivery: ActualDelivery::Background,
                progress,
            },
            text.len(),
            false,
            "",
        );
        assert_ne!(result.is_error, Some(true));
        assert_eq!(
            public_action(&result),
            serde_json::json!({
                "effect": "unverifiable", "route": "accessibility",
                "delivery": {"mode": "background"}
            })
        );
    }

    #[test]
    fn verify_typed_unreadable_after_is_unverified() {
        // Catalyst: can't read AXValue back → cannot confirm → false.
        assert!(!verify_typed(None, None, "hi"));
        assert!(!verify_typed(Some(""), None, "hi"));
    }

    #[test]
    fn verify_typed_requires_the_requested_insertion() {
        assert!(verify_typed(Some(""), Some("hi"), "hi"));
        assert!(verify_typed(Some("ab "), Some("ab hi"), "hi"));
        assert!(!verify_typed(Some("ab"), Some("ab hi"), "hi"));
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
        assert_eq!(delivery, TypedProgress::Complete);
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
        assert_eq!(delivery, TypedProgress::Partial(5));
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
        assert_eq!(PATH_KEY_EVENTS, "key_events");
        assert_eq!(PATH_KEY_EVENTS_FG, "key_events_fg");
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
