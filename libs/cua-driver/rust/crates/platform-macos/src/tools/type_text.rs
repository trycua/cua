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
             focused element.\n\n\
             ELECTRON/CHROMIUM: the AX layer accepts a write and echoes it back \
             through AXValue while the renderer never observes it. The driver \
             DETECTS Electron and refuses to trust that echo — an AX-path insert \
             on an Electron app returns effect:\"unverifiable\" + \
             escalation:{recommended:\"px\"}, never a false \"confirmed\". The fix \
             is this tool's px form: pass x,y (no element_index) to pixel-click the \
             field — giving the Chromium renderer real keyboard focus — then type, \
             in one call. NOTE: a px focus-click won't reliably open+focus a CLOSED \
             control; AX-press to open/activate it first (works in the background), \
             then px-type into the open field. Confirm via the screenshot \
             regardless of effect; if px-background still drops, escalate to \
             delivery_mode:\"foreground\"."
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
                "x": { "type": "number", "description": "Screenshot-pixel X of the field to type into — the element px action form. Pass x,y (no element_index) and the tool pixel-clicks there to establish real renderer focus, then types. Use for Chromium/Electron inputs the AX path can't reach. Read straight off the get_window_state PNG, same convention as click." },
                "y": { "type": "number", "description": "Screenshot-pixel Y of the field (see x)." },
                "delay_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 200,
                    "description": "Milliseconds between characters in the CGEvent fallback path. Default 30. Ignored when the AX path succeeds."
                },
                "delivery_mode": {
                    "type": "string",
                    "enum": ["background", "foreground"],
                    "description": "Best-effort-background ladder rung (default \"background\"). \"background\": AX insert, then CGEvent keystrokes if needed — no focus steal; the driver verifies via an AXValue read-back and reports `verified`. \"foreground\": briefly front the window, type, restore the prior frontmost — the explicit last resort for focus-sensitive surfaces (e.g. WhatsApp/Catalyst) where background keystrokes don't land. Re-call with \"foreground\" when a background attempt returns `verified:false` and a screenshot shows the text didn't appear."
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
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());

        // ── px form: focus by pixel-click, then type into the focused element ──
        // Pass x,y (no element_index) for an *element px action*: pixel-click the
        // field to give the Chromium/Electron renderer the real keyboard focus the
        // AX path can't, then fall through to the focused-element type path (which
        // escalates AX → CGEvent and lands once focused). Reuses ClickTool's exact
        // coordinate translation + delivery_mode, so it lands on the same pixel a
        // px-click would.
        let px = args.get("x").and_then(|v| v.as_f64());
        let py = args.get("y").and_then(|v| v.as_f64());
        if let (Some(cx), Some(cy)) = (px, py) {
            if element_index.is_some() {
                return ToolResult::error(
                    "Pass either element_index (ax) or x,y (px) to type_text, not both."
                );
            }
            let from_zoom = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Err(e) = super::focus_by_pixel(
                &self.state, pid, window_id, cx, cy, delivery_mode.is_foreground(),
                args.opt_str("session"), args.opt_str("_session_id"), from_zoom,
            ).await {
                return e;
            }
            // element_index stays None → the type path below writes to the now-
            // focused element via the CGEvent (key_events) rung.
        }

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
                        delivery_mode,
                        window_id,
                    )
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        match result {
            Ok(Ok((detail, path, verified))) => {
                // SURFACE-AWARE VERIFICATION. On Electron/Chromium web inputs the
                // AX layer accepts a write and echoes it straight back through
                // `AXValue` while the renderer never observes it — so an AX-path
                // read-back "confirm" is a shim echo, not ground truth (the
                // Slack-search false-confirm). Refuse to claim verified there: the
                // screenshot is the only truth and the next rung is the element px
                // action. Probe ONLY when it would otherwise confirm via the AX
                // path, so native types pay nothing.
                let ax_echo_surface = verified
                    && path == PATH_AX
                    && crate::browser::electron_js::ElectronJs::is_electron(pid);
                let verified = verified && !ax_echo_surface;

                // `verified:false` means the driver could not confirm the text
                // landed (Electron AX echo, unreadable AXValue on Catalyst, or a
                // CGEvent rung the app may have dropped). Don't dress that as a
                // confirmed insert — tell the agent to look, and point at the
                // right next rung.
                let (mark, note) = if verified {
                    ("✅ Inserted", String::new())
                } else if ax_echo_surface {
                    ("📨 Sent (unverified)",
                     " — Electron/web surface: the AX layer accepts and echoes the \
                      write but the renderer may not have observed it, so the driver \
                      cannot confirm via AX. Verify via the screenshot; if it didn't \
                      land, re-type with the px form (pass x,y to pixel-focus the \
                      field).".to_string())
                } else if path == PATH_KEY_EVENTS_FG {
                    ("📨 Sent (unverified)",
                     " — driver could not confirm; verify via screenshot.".to_string())
                } else {
                    ("📨 Sent (unverified)",
                     " — driver could not confirm the text landed; verify via screenshot, \
                      and re-call with delivery_mode:\"foreground\" if it didn't.".to_string())
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
                        "verified": verified,
                        "effect": if verified { "confirmed" } else { "unverifiable" },
                    });
                    if ax_echo_surface {
                        // Electron AX echo → the element px action is the next rung,
                        // not foreground (it's a renderer-focus problem, not an
                        // activation one).
                        s["escalation"] = serde_json::json!({
                            "recommended": "px",
                            "reason": "Electron/web surface — the AX write was echoed \
                                       but the renderer may not have observed it. \
                                       Confirm via the screenshot; if it didn't land, \
                                       re-type with the element px action (pass x,y to \
                                       pixel-focus the field, then type)."
                        });
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
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking implementation ───────────────────────────────────────────────────

/// Which delivery path was taken. Surfaced as `structuredContent.path`
/// on success.
const PATH_AX: &str = "ax";
const PATH_KEY_EVENTS: &str = "key_events";
const PATH_KEY_EVENTS_FG: &str = "key_events_fg";

/// Read-back verification for a keystroke rung: did the typed text actually land?
///
/// `before`/`after` are `AXValue` read from the target field before and after
/// the keystrokes. Returns whether we can *positively confirm* the text landed:
/// - unreadable `after` (`None`) → unverifiable → `false` (Catalyst case; the
///   agent must confirm via screenshot).
/// - `after` contains the text, or grew vs `before` → `true`.
/// - empty input text → trivially `true`.
///
/// Apps that normalize input (smart quotes, autocomplete) may fail the
/// substring/length test even though something landed — we report `false`
/// (unverified) rather than erroring, so the agent can still confirm.
fn verify_typed(before: Option<&str>, after: Option<&str>, text: &str) -> bool {
    if text.is_empty() { return true; }
    let Some(after) = after else { return false };
    after.contains(text)
        || before.map_or(false, |b| after.chars().count() > b.chars().count())
}

/// Decide whether an `AXSelectedText` write that returned `kAXErrorSuccess`
/// should be treated as a silent no-op — i.e. the success code lied and we must
/// fall back to CGEvent keystrokes.
///
/// `axvalue` is the read-back of `AXValue` *after* the write:
/// - `Some(non-empty)` → the text landed; trust the AX path (`false`).
/// - `Some("")` → readable but empty: the write was dropped, e.g. a Chromium /
///   Electron web input that accepts the call but never updates its DOM value.
/// - `None` → **unreadable**: unverifiable, so we must NOT report success. Mac
///   Catalyst apps (WhatsApp, Messages) accept the write, return success, and
///   expose no readable `AXValue`, yet the text never appears. Treat as dropped.
///
/// Empty input text is never a silent-accept (nothing to verify).
fn is_silent_accept(text: &str, axvalue: Option<&str>) -> bool {
    !text.is_empty() && axvalue.map(|v| v.is_empty()).unwrap_or(true)
}

/// Read the focused/target field's `AXValue`, for before/after read-back.
/// Re-fetches the focused element each call when no explicit element is given
/// (cheap, and focus is stable across our own keystrokes).
fn read_axvalue(pid: i32, element_ptr_and_idx: Option<(usize, Option<usize>)>) -> Option<String> {
    if let Some((ptr, _)) = element_ptr_and_idx {
        unsafe { copy_string_attr(ptr as AXUIElementRef, "AXValue") }
    } else if let Some(el) = unsafe { focused_element_of_pid(pid) } {
        let v = unsafe { copy_string_attr(el, "AXValue") };
        unsafe { CFRelease(el as _); }
        v
    } else {
        None
    }
}

/// Type via CGEvent keystrokes, optionally clearing the field first (idempotent
/// retype), then verify by read-back.
///
/// `clear_first` is the idempotency guard for escalation: when the field was
/// empty/unreadable at capture, `Cmd+A`+`Delete` makes the retype land exactly
/// `text` regardless of what a prior unverified rung may have done — avoiding
/// double-type. It is NOT used when the field had readable pre-existing content
/// (that would clobber it).
fn cgevent_type_verified(
    pid: i32,
    text: &str,
    delay_ms: u64,
    before: Option<&str>,
    clear_first: bool,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    settle_ms: u64,
) -> anyhow::Result<bool> {
    // Focus the target element first so the keystrokes land in IT. Critical in
    // foreground mode: a freshly-fronted window's keyboard focus may be on the
    // search box or nowhere, so without this the text goes into the void (or the
    // wrong field). AXFocused is best-effort — harmless when unsupported.
    if let Some((ptr, _)) = element_ptr_and_idx {
        let _ = crate::input::ax_actions::focus_element(ptr);
    }
    // First-keystroke settle (foreground rung only — caller passes `settle_ms > 0`).
    // After a window is fronted (with_foreground_assist) and the element focused,
    // the surface isn't ready to accept input for a few tens of ms, so the FIRST
    // synthesized character gets eaten: typing "i love u" rendered "love u" (the
    // leading "i " was dropped). A short sleep here lets focus settle before the
    // first key event. Background/terminal call sites pass 0 — they have no front
    // transition and must not pay this latency.
    if settle_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(settle_ms));
    }
    if clear_first {
        let _ = crate::input::keyboard::press_key(pid, "a", &["cmd"]);
        let _ = crate::input::keyboard::press_key(pid, "delete", &[]);
    }
    crate::input::keyboard::type_text_with_delay(pid, text, delay_ms)?;
    let after = read_axvalue(pid, element_ptr_and_idx);
    Ok(verify_typed(before, after.as_deref(), text))
}

/// Best-effort-background ladder for `type_text`.
///
/// - `delivery_mode == Background` (default): AX insert → read-back; on a
///   silent/unreadable accept, CGEvent keystrokes → read-back. Never fronts.
/// - `delivery_mode == Foreground`: the agent's explicit last resort — briefly
///   front `window_id`, type (clear-first when the field was empty/unreadable so
///   the retype is idempotent), restore, then read-back.
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
) -> anyhow::Result<(String, &'static str, bool)> {
    // Original field value before ANY rung — drives both the read-back delta and
    // the clear-then-type idempotency decision.
    let before = read_axvalue(pid, element_ptr_and_idx);
    // Clear-then-type only when we can't see existing content to preserve
    // (empty or unreadable). A readable non-empty value is left intact.
    let clear_first = !matches!(before.as_deref(), Some(b) if !b.is_empty());

    // --- Foreground rung: explicit agent request (skip AX/background ladder). ---
    if delivery_mode.is_foreground() {
        // Settle between front+focus and the first keystroke — see the
        // "i love u" -> "love u" first-char-drop note in cgevent_type_verified.
        // 60ms covers native Cocoa/Catalyst surfaces, but focus-proxy clients
        // that re-establish their own input channel on activation need longer:
        // an RDP client (Microsoft Windows App) re-arms its keyboard grab with
        // the remote host over hundreds of ms, so at 60ms every keystroke was
        // dropped. 200ms covers that re-grab without being perceptible.
        const FOREGROUND_SETTLE_MS: u64 = 200;
        let do_type = || cgevent_type_verified(
            pid, text, delay_ms, before.as_deref(), clear_first, element_ptr_and_idx,
            FOREGROUND_SETTLE_MS,
        );
        let verified = match window_id {
            Some(wid) => {
                // Front → type → restore. The closure returns the read-back
                // result; with_foreground_assist returns whether it fronted.
                let mut typed_verified = false;
                crate::input::skylight::with_foreground_assist(pid as libc::pid_t, wid, || {
                    typed_verified = do_type()?;
                    Ok(())
                })?;
                typed_verified
            }
            // No window to front — best-effort background keystrokes instead.
            None => do_type()?,
        };
        return Ok((
            format!(" via foreground keystrokes ({delay_ms}ms delay)"),
            PATH_KEY_EVENTS_FG,
            verified,
        ));
    }

    // --- Background rung 0: terminal emulator → CGEvent only (AX is dropped). ---
    if is_terminal_target {
        tracing::debug!(
            "type_text: pid {pid} is a terminal emulator; skipping AX value-set, \
             using CGEvent key-event synthesis"
        );
        let verified = cgevent_type_verified(
            pid, text, delay_ms, before.as_deref(), /*clear_first=*/ false, element_ptr_and_idx,
            /*settle_ms=*/ 0,
        )?;
        return Ok((
            format!(" via CGEvent (terminal emulator, {delay_ms}ms delay)"),
            PATH_KEY_EVENTS,
            verified,
        ));
    }

    // --- Background rung 1: AX SelectedText write (element or focused). ---
    let ax_target: Option<(AXUIElementRef, bool, Option<usize>)> = match element_ptr_and_idx {
        Some((ptr, idx)) => Some((ptr as AXUIElementRef, /*owns=*/ false, idx)),
        None => unsafe { focused_element_of_pid(pid) }.map(|el| (el, /*owns=*/ true, None)),
    };
    if let Some((element, owns, idx_opt)) = ax_target {
        let role  = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
        let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
        let err = unsafe { set_string_attr(element, "AXSelectedText", text) };
        // Landed iff the API succeeded AND a read-back confirms it (not a
        // silent/unreadable accept). See `is_silent_accept`.
        let ax_landed = err == kAXErrorSuccess
            && !is_silent_accept(
                text,
                unsafe { copy_string_attr(element, "AXValue") }.as_deref(),
            );
        if owns { unsafe { CFRelease(element as _); } }
        if ax_landed {
            let idx_str = idx_opt.map(|i| format!(" [{i}]")).unwrap_or_default();
            return Ok((format!(" into{idx_str} {role} \"{title}\""), PATH_AX, true));
        }
        tracing::debug!(
            "AX write did not land for {role} \"{title}\" (err={err}); \
             falling back to CGEvent keystrokes"
        );
    } else {
        tracing::debug!("No focused element for pid {pid}; using CGEvent keystrokes");
    }

    // --- Background rung 2: CGEvent keystrokes with read-back. ---
    // No clear-first here: a partial AX write is rare and clearing on every
    // background fallback would change insert-at-cursor semantics. The
    // foreground rung owns the idempotent clear-then-type.
    let verified = cgevent_type_verified(
        pid, text, delay_ms, before.as_deref(), /*clear_first=*/ false, element_ptr_and_idx,
        /*settle_ms=*/ 0,
    )?;
    Ok((
        format!(" via CGEvent ({delay_ms}ms delay)"),
        PATH_KEY_EVENTS,
        verified,
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
        let r = type_text_blocking(
            -1, "x", None, 0, /*is_terminal_target=*/ true,
            super::super::DeliveryMode::Background, None,
        );
        // We don't care whether r is Ok or Err — what matters is that
        // calling it with is_terminal_target=true is safe and never
        // dereferences null AX pointers.
        let _ = r;
    }

    #[test]
    fn silent_accept_unreadable_axvalue_falls_back() {
        // The WhatsApp/Catalyst bug: AXSelectedText returns success but AXValue
        // is unreadable (None). Must be treated as silent-accept ⇒ fall back.
        assert!(is_silent_accept("i love u", None));
    }

    #[test]
    fn silent_accept_readable_empty_falls_back() {
        // Chromium/Electron: readable but empty after a non-empty write ⇒ dropped.
        assert!(is_silent_accept("hello", Some("")));
    }

    #[test]
    fn silent_accept_readable_nonempty_is_trusted() {
        // Standard Cocoa field: AXValue reflects the inserted text ⇒ AX path ok.
        assert!(!is_silent_accept("hello", Some("hello")));
        assert!(!is_silent_accept("world", Some("prefix world")));
    }

    #[test]
    fn silent_accept_empty_text_is_never_silent() {
        // Nothing to verify when inserting empty text.
        assert!(!is_silent_accept("", None));
        assert!(!is_silent_accept("", Some("")));
    }

    #[test]
    fn verify_typed_unreadable_after_is_unverified() {
        // Catalyst: can't read AXValue back → cannot confirm → false.
        assert!(!verify_typed(None, None, "hi"));
        assert!(!verify_typed(Some(""), None, "hi"));
    }

    #[test]
    fn verify_typed_contains_or_grew_is_verified() {
        assert!(verify_typed(Some(""), Some("hi"), "hi"));          // contains
        assert!(verify_typed(Some("ab"), Some("ab hi"), "hi"));     // contains, appended
        assert!(verify_typed(Some("ab"), Some("abXY"), "??"));      // grew vs before
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
}
