use async_trait::async_trait;
use cua_driver_contract::TypeTextInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    element_target_rect, focus_by_pixel, focus_cached_element_for_foreground,
    overlay_glide_to_target, pin_overlay_above, position_keyboard_cursor, resolve_cursor_key,
    resolve_onscreen_point_with_scroll, ToolState,
};

// ── type_text ─────────────────────────────────────────────────────────────────

pub struct TypeTextTool {
    pub(crate) state: Arc<ToolState>,
}

static TYPE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for TypeTextTool {
    fn def(&self) -> &ToolDef {
        TYPE_DEF.get_or_init(|| ToolDef {
            name: "type_text".into(),
            // Description ported from Swift `TypeTextTool.swift` with the
            // Windows transport note.  Swift tries AXSetAttribute(kAXSelectedText)
            // first, falls back to character-by-character CGEvent synthesis.
            // Windows uses character-by-character PostMessage(WM_CHAR) to the
            // window's focused child — there's no UIA equivalent of
            // AXSelectedText wired up yet, so it's always the synthesis path.
            description: "Insert text into the target pid via character-by-character \
                PostMessage(WM_CHAR) to the focused window. No focus steal.\n\n\
                Special keys (Return, Escape, arrows, Tab) go through `press_key` / \
                `hotkey` — they are not text.\n\n\
                **Routing on Windows.** `element_token` selects a specific UIA element \
                and writes its ValuePattern value. For no-element foreground typing, a \
                separate capability-gated path can use the exact focused UIA Document \
                only when it is writable and empty. \
                Eligibility depends on control capabilities and live focus, not app names. \
                Chromium/Electron windows keep their existing boundary because a UIA \
                value echo does not prove that the renderer accepted the text. The \
                path reads the value and caret back; after SetValue starts, an \
                unverifiable result requires a fresh observation before any retry and \
                is never replayed through native input. This replaces the full \
                ValuePattern value, so rich-text formatting and native undo behavior \
                are not guaranteed; single-line Edit controls \
                keep the existing input route. For other XAML / WinUI3 / UWP cases, \
                use `element_token` from `get_window_state`; background WM_CHAR does \
                not reach their CoreInput dispatcher. Native ConsoleHost on Windows ARM64 is \
                hard-refused because it can \
                accept synthesized Unicode events without delivering them; use a process \
                or PTY setup channel instead. Legacy Win32 apps still use the PostMessage \
                path, preserving the no-focus-steal property.\n\n\
                `delay_ms` (0–200, default 30) spaces successive characters on the \
                PostMessage path so autocomplete and IME can keep up. Ignored on the \
                UIA path (SetValue is atomic).".into(),
            input_schema: json!({
                "type":"object","required":["text"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "scope":{"type":"string","enum":["window","desktop"],"description":"Use desktop with no pid/window_id to type into the current foreground application."},
                    "pid":{"type":"integer","description":"Target process ID."},
                    "text":{"type":"string","description":"Text to insert at the focused element's cursor."},
                    "window_id":{"type":"integer","description":"HWND of the target window. Omit when element_token is supplied (the token carries it)."},
                    "element_token":{"type":"string","pattern":"^s[0-9a-f]{8}:[0-9]+$","description":"Opaque element handle from get_window_state. When supplied, type_text writes through UIA ValuePattern and reads that exact element back by handle. The shared ActionResult is confirmed only when the complete expected value is synchronously visible. If SetValue succeeds but read-back is stale or unavailable, the result is unverifiable with no escalation; take a fresh snapshot before retrying because deferred providers may publish only after this call returns."},
                    "x":{"type":"number","description":"Window-local screenshot-pixel X of the field to type into — the element px action form. Pass x,y (no element_token) and the tool pixel-clicks there to establish real renderer focus, then types. Use for Chromium/Electron inputs the UIA/WM_CHAR path can't reach. Read straight off the get_window_state PNG, same convention as click."},
                    "y":{"type":"number","description":"Window-local screenshot-pixel Y of the field (see x)."},
                    "delay_ms":{"type":"integer","minimum":0,"maximum":200,"description":"Milliseconds between characters. Default 30."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_key = resolve_cursor_key(&args);
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
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
            let hwnd = match crate::input::mouse::foreground_window() {
                Ok(hwnd) => hwnd,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            let text_len = text.chars().count();
            if args.get("element_token").is_none()
                && args.get("x").is_none()
                && args.get("y").is_none()
            {
                if let Some(result) = try_focused_document_route(hwnd, None, &text, text_len).await
                {
                    return result;
                }
            }
            return match tokio::task::spawn_blocking(move || {
                crate::input::send_text_synthesized(hwnd, &text)
            })
            .await
            {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) into the foreground application (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SendInput", "characters": text_len,
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("desktop type task failed: {error}")),
            };
        }
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Strip trailing agent-protocol closing tags before delivery —
        // catches the case where an LLM hallucinated its own tool-
        // invocation tags into the text param (see text_sanitize docs).
        // Returns Cow::Borrowed on the no-match fast path so the common
        // case is allocation-free.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        let raw_pid = match args.require_i64("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pid = raw_pid as u32;
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (elem_idx, hwnd_opt, admitted) = resolved.into_parts(args.opt_u64("window_id"));
        let delivery = DeliveryMode::from_args(&args);

        // ── px form: focus by pixel-click, then type into the focused element ──
        // Pass x,y (no element_token) for an *element px action*: pixel-click the
        // field to give the Chromium/Electron renderer the real keyboard focus the
        // UIA/WM_CHAR path can't establish on its own, then fall through to the
        // focused-window WM_CHAR path below (which lands once focused). Reuses
        // ClickTool's exact coordinate translation + delivery_mode. Mirrors macOS.
        {
            let px = args.get("x").and_then(|v| v.as_f64());
            let py = args.get("y").and_then(|v| v.as_f64());
            if let (Some(cx), Some(cy)) = (px, py) {
                if elem_idx.is_some() {
                    return ToolResult::error(
                        "Pass either element_token (ax) or x,y (px) to type_text, not both.",
                    );
                }
                let from_zoom = args
                    .get("from_zoom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Err(e) = focus_by_pixel(
                    &self.state,
                    pid,
                    hwnd_opt,
                    cx,
                    cy,
                    delivery.is_foreground(),
                    args.opt_str("session"),
                    args.opt_str("_session_id"),
                    from_zoom,
                    false,
                )
                .await
                {
                    return e;
                }
                // elem_idx stays None → the type path below writes to the now-
                // focused element via the WM_CHAR (key_events) rung.
            }
        }

        // Same no-raise guard as click: a XAML/WPF ValuePattern.SetValue handler
        // calls UIElement.Focus()→SetForegroundWindow; WS_EX_NOACTIVATE on the
        // target makes that a no-op while the value still gets set. Safe because
        // type_text never uses the SendInput foreground-swap path. No-op for
        // delivery_mode:"foreground" and when no window_id was given.
        let _noact = match (delivery != DeliveryMode::Foreground, hwnd_opt) {
            (true, Some(h)) => Some(crate::input::NoActivateGuard::arm(
                windows::Win32::Foundation::HWND(h as *mut _),
            )),
            _ => None,
        };
        let _delay_ms = args.u64_or("delay_ms", 30);
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::win32::list_windows(Some(pid))
                    }
                })
                .await
                .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };
        if let Some(element) = admitted.as_ref() {
            let (cx, cy) = element.center;
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to_target(
                &cursor_key,
                cx as f64,
                cy as f64,
                element_target_rect(Some(element), (0, 0)),
            )
            .await;
            self.state
                .cursor_registry
                .update_position(&cursor_key, cx as f64, cy as f64);
        } else if args.get("x").and_then(Value::as_f64).is_none()
            || args.get("y").and_then(Value::as_f64).is_none()
        {
            // Untargeted text still gives a named session visible feedback:
            // its remembered position, or the window centre on the first action.
            position_keyboard_cursor(&self.state, &args, hwnd, None).await;
        }
        let text_len = text.chars().count();

        // Native ConsoleHost can accept every synthesized Unicode event while
        // dropping the text at the prompt (observed on Windows 11 ARM64).
        // Classify terminal delivery before the foreground SendInput branch so
        // an explicit foreground retry cannot bypass the hard refusal.
        match crate::terminal::type_text_policy(hwnd, delivery.is_foreground()) {
            crate::terminal::TypeTextPolicy::Unsupported => {
                return crate::terminal::native_consolehost_type_text_error(hwnd);
            }
            crate::terminal::TypeTextPolicy::ForegroundRequired => {
                return crate::input::delivery::background_unavailable_error(
                    hwnd,
                    EventKind::TextInput,
                );
            }
            crate::terminal::TypeTextPolicy::Supported => {}
        }

        // Refuse known background drops before the final WM_CHAR path. WPF is
        // conditional: indexed text still has a working UIA ValuePattern route,
        // while unindexed text would be posted to the top-level and disappear.
        if delivery == DeliveryMode::Background
            && (crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::TextInput)
                || (elem_idx.is_none() && crate::input::delivery::is_wpf_target_window(hwnd)))
        {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::TextInput,
            );
        }
        // delivery_mode:"foreground" — explicit SendInput Unicode with a brief
        // foreground swap (send_text_synthesized), symmetric with press_key's
        // foreground path. This is the rung that lands on VCL/LibreOffice
        // document grids and other targets where PostMessage WM_CHAR is
        // silently dropped. Honest by construction: if the foreground swap is
        // rejected (daemon not at UIAccess integrity), it returns an error
        // rather than a false success.
        if delivery == DeliveryMode::Foreground {
            if elem_idx.is_none()
                && args.get("element_token").is_none()
                && args.get("x").is_none()
                && args.get("y").is_none()
            {
                if let Some(result) =
                    try_focused_document_route(hwnd, Some(pid), &text, text_len).await
                {
                    return result;
                }
            }
            // Resolve the optional click fallback before entering the atomic
            // activation/focus/input transaction. The focus itself happens
            // only after exact top-level foreground is confirmed.
            let focus_target = if let Some(idx) = elem_idx {
                let (cx, cy) = match admitted.as_ref().map(|element| element.center) {
                    Some(center) => center,
                    None => {
                        return ToolResult::error(format!(
                        "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                    ))
                    }
                };
                let (cx, cy) = match resolve_onscreen_point_with_scroll(
                    &admitted,
                    hwnd,
                    idx as usize,
                    cx,
                    cy,
                    "foreground typing",
                ) {
                    Ok(point) => point,
                    Err(result) => return result,
                };
                Some((idx as usize, (cx, cy)))
            } else {
                None
            };
            let text_fg = text.clone();
            let r = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    crate::input::send_text_synthesized_after_focus(hwnd, &text_fg, || {
                        if let Some((idx, point)) = focus_target {
                            focus_cached_element_for_foreground(&admitted, hwnd, idx, Some(point))?;
                        }
                        Ok(())
                    })
                }
            })
            .await;
            return match r {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Typed {text_len} char(s) on pid {raw_pid} via SendInput (delivery_mode:foreground)."
                ))
                .with_structured(serde_json::json!({
                    "path": "key_events",
                    "characters": text_len,
                    // Foreground SendInput delivered, but with no read-back the
                    // driver can't confirm the effect → unverifiable. No escalation:
                    // foreground IS the escalated rung (mirrors macOS PATH_KEY_EVENTS_FG).
                    "effect": "unverifiable",
                })),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Pin the agent-cursor overlay above the target window so the synthetic
        // cursor stays sandwiched at z+1 of the type target for the full
        // duration of the keystrokes (both XAML/UIA and PostMessage paths).
        pin_overlay_above(&cursor_key, hwnd);

        // Glide the agent cursor onto the field being typed into, so the viewer
        // can see *where* the agent is typing — same visual feedback as a click.
        // Only when an element_index is supplied (we have its cached center);
        // the focused-element path has no resolvable position to point at.
        if let Some(element) = admitted.as_ref() {
            let (cx, cy) = element.center;
            overlay_glide_to_target(
                &cursor_key,
                cx as f64,
                cy as f64,
                element_target_rect(Some(element), (0, 0)),
            )
            .await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: cx as f64,
                    y: cy as f64,
                },
            );
        }

        // CUA-543 routing: PostMessage WM_CHAR doesn't reach modern
        // XAML / WinUI3 hosts. When the target is one of those AND the
        // caller has supplied an element_index, route through UIA
        // `ValuePattern.SetValue` — same code path the `set_value` tool
        // uses, which we've verified works on modern Notepad / WinUI3.
        // Legacy Win32 stays on the PostMessage path so the no-focus-
        // steal property is preserved.
        // ── Automatic routing — the caller need not know the framework. ──
        // 1. With an element_index, try UIA ValuePattern.SetValue first: it works
        //    for WPF / WinForms / UWP / XAML and many web inputs, sets the value
        //    through the accessibility channel (no keystrokes), and the `_noact`
        //    guard blocks any self-foreground — so it never raises. Auto-falls-
        //    back to the WM_CHAR path below if the element has no ValuePattern
        //    (most legacy Win32 EDITs consume WM_CHAR fine without focus steal).
        if let Some(idx) = elem_idx {
            let text_for_uia = text.clone();
            let set_result = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    // Retain the element under the cache lock so a concurrent
                    // get_window_state snapshot-replace on the same (pid, hwnd)
                    // can't Release it to zero while this SetValue is in flight.
                    // The guard is held for the whole closure.
                    let element_guard = admitted.as_ref().filter(|element| element.is_uia())?;
                    let ptr = element_guard.as_ptr();
                    use windows::core::{Interface, BSTR};
                    use windows::Win32::UI::Accessibility::{
                        IUIAutomationElement, IUIAutomationValuePattern, UIA_ValuePatternId,
                    };
                    let elem: IUIAutomationElement =
                        unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
                    let result = (|| -> anyhow::Result<(String, String)> {
                        let pattern = unsafe { elem.GetCurrentPattern(UIA_ValuePatternId) }?;
                        let vp: IUIAutomationValuePattern = pattern.cast()?;
                        // Shield the SetValue against host self-foreground. A
                        // Chromium/Electron (or XAML) ValuePattern.SetValue handler
                        // calls SetForegroundWindow(self), which WS_EX_NOACTIVATE
                        // does NOT stop; the EnableWindow shield does (disabled
                        // top-level can't be foregrounded) while the a11y-channel
                        // SetValue still lands. Same fix as the UIA Invoke path.
                        crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                            // ValuePattern has no caret/selection insertion API. Preserve
                            // the current document and append as the deterministic
                            // background fallback; replacing the whole value violates
                            // type_text's insertion contract for editors.
                            let current = vp
                                .CurrentValue()
                                .map(|value| value.to_string())
                                .unwrap_or_default();
                            let inserted = format!("{current}{text_for_uia}");
                            vp.SetValue(&BSTR::from(inserted.as_str()))?;
                            Ok((current, inserted))
                        })
                    })()
                    .ok();
                    std::mem::forget(elem);
                    result
                }
            })
            .await
            .unwrap_or(None);
            if let Some((before, expected)) = set_result {
                // Read the value back by element handle — focus-independent
                // (works regardless of which window is foreground), proving the
                // a11y write actually took rather than trusting SetValue's
                // return alone.
                let verify = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        let value = read_cached_element_value(&admitted);
                        classify_value_write_readback(value.as_deref(), &before, &expected)
                    }
                })
                .await
                .unwrap_or("pending");
                // SURFACE-AWARE VERIFICATION (mirrors macOS type_text). On
                // Electron/Chromium web inputs the UIA layer accepts a
                // `ValuePattern.SetValue` and echoes it straight back through
                // `ValuePattern.CurrentValue` while the renderer/DOM never observes
                // it — so a read-back "confirm" is a shim echo, not ground truth
                // (the Windows analogue of the Slack-search AX false-confirm).
                // Refuse to claim verified there: the screenshot is the only truth
                // and the next rung is the element px action. Probe ONLY when it
                // would otherwise confirm via the UIA value path, so native UIA
                // types (WPF / WinForms / UWP / XAML) pay nothing. The signal is
                // the window class — Chromium and every Electron host expose
                // `Chrome_WidgetWin_*` (see `is_chromium_target_window`).
                let ax_echo_surface =
                    verify == "confirmed" && crate::input::is_chromium_target_window(hwnd);
                let verified = verify == "confirmed" && !ax_echo_surface;
                let (mark, note) = if verified {
                    ("✅ Wrote", String::new())
                } else if ax_echo_surface {
                    (
                        "📨 Sent (unverified)",
                        " — Electron/web surface: the UIA layer accepts and echoes the \
                      write but the renderer may not have observed it, so the driver \
                      cannot confirm via UIA. Verify via the screenshot; if it didn't \
                      land, re-type with the px form (pass x,y to pixel-focus the \
                      field)."
                            .to_string(),
                    )
                } else {
                    (
                        "📨 Accepted (verification pending)",
                        " — the provider may publish its new value only after this call returns. Take a fresh snapshot before retrying."
                            .to_string(),
                    )
                };
                return ToolResult::text(format!(
                    "{mark} {text_len} char(s) on pid {raw_pid} via UIA ValuePattern \
                     (element_index=[{idx}]; verify: {verify}).{note}"
                ))
                .with_structured({
                    // `effect` mirrors the UIA read-back: a TRUSTED positive
                    // read-back is "confirmed"; a deferred provider or an Electron UIA echo
                    // that we refuse to trust is "unverifiable". Only the echo
                    // recommends pixel escalation.
                    let mut s = value_write_structured_result(text_len, verify, verified);
                    if ax_echo_surface {
                        // Electron UIA echo → the element px action is the next rung,
                        // not foreground (it's a renderer-focus problem, not an
                        // activation one).
                        s["escalation"] = serde_json::json!({
                            "recommended": "px",
                            "reason": "Electron/web surface — the UIA write was echoed \
                                       but the renderer may not have observed it. \
                                       Confirm via the screenshot; if it didn't land, \
                                       re-type with the element px action (pass x,y to \
                                       pixel-focus the field, then type)."
                        });
                    }
                    s
                });
            }
            // ValuePattern unavailable → fall through to the WM_CHAR path.
        }

        // 2. No element_index on a WPF target: WM_CHAR is dropped and there's no
        //    element to SetValue. The only background path was a cloaked focus
        //    grab (SendInput), which the macOS-aligned contract forbids in
        //    background. Prefer supplying an element_index (path 1, UIA SetValue —
        //    never raises); otherwise surface background_unavailable so the agent
        //    escalates to delivery_mode:"foreground".
        if elem_idx.is_none() && crate::input::delivery::is_wpf_target_window(hwnd) {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::TextInput,
            );
        }

        // 3. Legacy Win32 / GDI / Chromium-IME — PostMessage WM_CHAR, no focus steal.
        //
        // Read-back verification (mirrors macOS type_text): WM_CHAR is silently
        // dropped by some focused targets (notably a VCL/LibreOffice Calc grid
        // not in cell-edit mode), and PostMessage reports success regardless. So
        // snapshot the focused element's UIA value before and after; if it did
        // not change we can't confirm the text landed and report "Sent
        // (unverified)" with the foreground-escalation hint, instead of a blind
        // success. If the focused element exposes no ValuePattern we likewise
        // can't confirm (conservative: under-claim rather than false-succeed).
        let text_for_post = text.clone();
        let verify_pid = pid;
        let verify_idx = elem_idx.map(|i| i as usize);
        let result = tokio::task::spawn_blocking({
            let admitted = admitted.clone();
            move || {
                let _admission = &admitted;
                // Prefer a focus-independent read of the *specific* cached element
                // when we have its index; only fall back to the (flaky, focus-
                // dependent) system focused element when typing into "whatever is
                // focused" with no element_token.
                let read = |idx: Option<usize>| match idx {
                    Some(_) => read_cached_element_value(&admitted),
                    None => read_focused_value_uia(verify_pid),
                };
                let before = read(verify_idx);
                let post_res = crate::input::post_type_text(hwnd, &text_for_post);
                std::thread::sleep(std::time::Duration::from_millis(40));
                let after = read(verify_idx);
                let observed = post_message_readback_observed(
                    before.as_deref(),
                    after.as_deref(),
                    &text_for_post,
                );
                (post_res, before, after, observed)
            }
        })
        .await;
        match result {
            Ok((Ok(()), before, after, observed)) => {
                // Three-way verdict. Crucially, "couldn't read the field" is NOT
                // the same as "the text didn't land": on Windows, UIA's
                // GetFocusedElement is system-wide (unlike macOS's per-app
                // AXFocusedUIElement), so when the target isn't the foreground
                // app we cannot read it back even though a PostMessage WM_CHAR
                // may have landed fine. Treating unreadable as failure would
                // spuriously push agents to foreground and break the
                // background-first contract. A changed value containing the
                // requested text is useful evidence, but the insertion point is
                // unknowable on WM_CHAR. Keep the action unverifiable so callers
                // prove final state from a fresh snapshot before any retry.
                match (&before, &after) {
                    (Some(_), Some(_)) if observed => ToolResult::text(format!(
                        "📨 Sent {text_len} char(s) to pid {raw_pid} via PostMessage \
                         ({_delay_ms}ms delay; the field changed and now contains the \
                         requested text, but the insertion point is not known). Take a \
                         fresh snapshot to verify the intended final value before retrying."
                    ))
                    .with_structured(changed_contains_post_message_result(text_len)),
                    (Some(_), Some(_)) => ToolResult::text(format!(
                        "📨 Sent {text_len} char(s) to pid {raw_pid} via PostMessage, but \
                         the focused field's value did not contain the requested text \
                         after dispatch — delivery was incomplete or dropped (e.g. a \
                         VCL/LibreOffice grid not in cell-edit mode). \
                         Retry with delivery_mode:\"foreground\"."
                    ))
                    .with_structured(serde_json::json!({
                        "path": "key_events", "characters": text_len,
                        "verified": false, "verify": "unchanged",
                        "effect": "unverifiable",
                        "escalation": {
                            "recommended": "foreground",
                            "reason": "background insert could not be confirmed — re-call \
                                       with delivery_mode:\"foreground\" if a screenshot \
                                       shows the text didn't land."
                        },
                    })),
                    _ => ToolResult::text(format!(
                        "📨 Sent {text_len} char(s) to pid {raw_pid} via PostMessage \
                         ({_delay_ms}ms delay; not verified — could not read \
                         the focused field back, e.g. the target isn't foreground or \
                         exposes no ValuePattern). If it didn't land, retry with \
                         delivery_mode:\"foreground\"."
                    ))
                    .with_structured(serde_json::json!({
                        "path": "key_events", "characters": text_len,
                        "verified": false, "verify": "unreadable",
                        "effect": "unverifiable",
                        "escalation": {
                            "recommended": "foreground",
                            "reason": "background insert could not be confirmed — re-call \
                                       with delivery_mode:\"foreground\" if a screenshot \
                                       shows the text didn't land."
                        },
                    })),
                }
            }
            Ok((Err(e), _, _, _)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

async fn try_focused_document_route(
    hwnd: u64,
    expected_pid: Option<u32>,
    text: &str,
    text_len: usize,
) -> Option<ToolResult> {
    if crate::input::is_chromium_target_window(hwnd) {
        return None;
    }
    let text = text.to_owned();
    let outcome = tokio::task::spawn_blocking(move || {
        crate::uia::focused_document::try_type_focused_document_append(hwnd, expected_pid, &text)
    })
    .await
    .unwrap_or(
        crate::uia::focused_document::FocusedDocumentTypeOutcome::Unknown(
            "focused UIA task failed",
        ),
    );
    focused_document_tool_result(outcome, text_len)
}

fn focused_document_tool_result(
    outcome: crate::uia::focused_document::FocusedDocumentTypeOutcome,
    text_len: usize,
) -> Option<ToolResult> {
    use crate::uia::focused_document::FocusedDocumentTypeOutcome;

    match outcome {
        FocusedDocumentTypeOutcome::NotApplicable => None,
        FocusedDocumentTypeOutcome::Confirmed => Some(
            ToolResult::text(format!(
                "Wrote and verified {text_len} character(s) in the focused Document; caret is at the document end."
            ))
            .with_structured(json!({
                "path": "ax",
                "characters": text_len,
                "verified": true,
                "verify": "confirmed",
                "effect": "confirmed",
            })),
        ),
        FocusedDocumentTypeOutcome::Unknown(stage) => Some(
            ToolResult::text(format!(
                "Focused Document text may have been written but could not be verified ({stage}). Inspect the current document before retrying."
            ))
            .with_structured(json!({
                "path": "ax",
                "characters": text_len,
                "verified": false,
                "verify": "unverifiable",
                "effect": "unverifiable",
                "retryable": false,
            })),
        ),
    }
}

#[cfg(test)]
mod focused_document_result_tests {
    use super::focused_document_tool_result;
    use crate::uia::focused_document::FocusedDocumentTypeOutcome;
    use cua_driver_contract::{ActionEffect, ActionRoute};
    use cua_driver_core::action_record::{ActionExecutionRecord, ActionTransport};
    use cua_driver_core::protocol::ToolResult;
    use serde_json::json;

    fn normalize(result: ToolResult) -> cua_driver_contract::ActionResult {
        let structured = result
            .structured_content
            .expect("focused-document result includes structured evidence");
        let record = ActionExecutionRecord::from_legacy(
            "type_text",
            &json!({"scope": "desktop"}),
            &structured,
        )
        .expect("the action dispatcher recognizes the UIA value transport");
        assert_eq!(record.transport, ActionTransport::WindowsUiaValue);
        record
            .public_result()
            .expect("focused-document action result normalizes")
    }

    #[test]
    fn focused_document_results_normalize_to_accessibility_with_truthful_effects() {
        let confirmed = focused_document_tool_result(FocusedDocumentTypeOutcome::Confirmed, 12)
            .expect("confirmed focused-document output");
        let confirmed = normalize(confirmed);
        assert_eq!(confirmed.route, ActionRoute::Accessibility);
        assert_eq!(confirmed.effect, ActionEffect::Confirmed);

        let unknown = focused_document_tool_result(
            FocusedDocumentTypeOutcome::Unknown("value read-back failed"),
            12,
        )
        .expect("unknown focused-document output");
        assert_eq!(
            unknown.structured_content.as_ref().unwrap()["retryable"],
            false
        );
        let unknown = normalize(unknown);
        assert_eq!(unknown.route, ActionRoute::Accessibility);
        assert_eq!(unknown.effect, ActionEffect::Unverifiable);
    }
}

fn post_message_readback_observed(
    before: Option<&str>,
    after: Option<&str>,
    requested_text: &str,
) -> bool {
    matches!(
        (before, after),
        (Some(before), Some(after))
            if before != after && after.contains(requested_text)
    )
}

fn value_write_structured_result(
    text_len: usize,
    verify: &str,
    verified: bool,
) -> serde_json::Value {
    serde_json::json!({
        "path": "ax",
        "characters": text_len,
        "verified": verified,
        "verify": verify,
        "effect": if verified { "confirmed" } else { "unverifiable" },
    })
}

fn changed_contains_post_message_result(text_len: usize) -> serde_json::Value {
    serde_json::json!({
        "path": "key_events",
        "characters": text_len,
        "verified": false,
        "verify": "changed_contains",
        "effect": "unverifiable",
    })
}

#[cfg(test)]
mod post_message_readback_tests;

fn classify_value_write_readback(
    value: Option<&str>,
    before: &str,
    expected: &str,
) -> &'static str {
    match value {
        Some(value) if value == expected && value != before => "confirmed",
        // SetValue returned success. Some providers publish their new Value only
        // after this tool call unwinds, so an old or unreadable synchronous value
        // is pending verification, not evidence that the write failed.
        _ => "pending",
    }
}

/// Read a **specific cached element's** text value by its `element_index`,
/// for type_text read-back verification. Tries `ValuePattern.CurrentValue`
/// then falls back to `TextPattern` document text. Unlike
/// [`read_focused_value_uia`] this is **focus-independent**: it reads the exact
/// element the caller typed into (by handle, from the per-(pid,window) cache),
/// so it works whether or not the target is the foreground window. Returns
/// `None` if the index isn't cached or the element exposes no readable text
/// pattern. Mirrors the cache-retain + `mem::forget` discipline of the
/// ValuePattern.SetValue path so the cached COM ref isn't released.
fn read_cached_element_value(
    admitted: &Option<crate::uia::snapshot::RetainedElement>,
) -> Option<String> {
    use windows::core::Interface;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationValuePattern,
        UIA_TextPatternId, UIA_ValuePatternId,
    };
    let guard = admitted.as_ref().filter(|element| element.is_uia())?;
    let ptr = guard.as_ptr();
    let elem: IUIAutomationElement = unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
    let val = unsafe {
        elem.GetCurrentPattern(UIA_ValuePatternId)
            .ok()
            .and_then(|p| p.cast::<IUIAutomationValuePattern>().ok())
            .and_then(|vp| vp.CurrentValue().ok())
            .map(|b| b.to_string())
            .or_else(|| {
                elem.GetCurrentPattern(UIA_TextPatternId)
                    .ok()
                    .and_then(|p| p.cast::<IUIAutomationTextPattern>().ok())
                    .and_then(|tp| tp.DocumentRange().ok())
                    .and_then(|r| r.GetText(-1).ok())
                    .map(|b| b.to_string())
            })
    };
    // The cache guard owns the ref; don't let `elem`'s Drop release it.
    std::mem::forget(elem);
    val
}

/// Read the currently-focused UIA element's `ValuePattern` value **iff it
/// belongs to `expect_pid`**, for type_text read-back verification.
/// Self-contained COM (STA) init/teardown so it can be called from a
/// `spawn_blocking` worker. Returns `None` if UIA is unreachable, nothing is
/// focused, the focused element is owned by a different process (e.g. the
/// target is backgrounded and system focus is on the user's foreground app —
/// we must NOT read that element's value), or the focused element exposes no
/// `ValuePattern` (most rich-text/document surfaces). Callers treat `None` as
/// "could not confirm" rather than failure. Mirrors the focused-element read in
/// the `debug_window_info` tool.
fn read_focused_value_uia(expect_pid: u32) -> Option<String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationValuePattern, UIA_ValuePatternId,
    };

    let coinit_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    let value = (|| -> Option<String> {
        let uia: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.ok()?;
        let focused = unsafe { uia.GetFocusedElement() }.ok()?;
        // Scope to the target: system focus may be on a different app when the
        // target is backgrounded; reading that element would give a bogus verdict.
        if unsafe { focused.CurrentProcessId() }.ok()? as u32 != expect_pid {
            return None;
        }
        let pattern = unsafe { focused.GetCurrentPattern(UIA_ValuePatternId) }.ok()?;
        let vp: IUIAutomationValuePattern = pattern.cast().ok()?;
        Some(unsafe { vp.CurrentValue() }.ok()?.to_string())
    })();
    if coinit_ok {
        unsafe {
            CoUninitialize();
        }
    }
    value
}

#[cfg(test)]
mod value_write_readback_tests;
