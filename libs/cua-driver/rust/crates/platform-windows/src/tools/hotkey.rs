use async_trait::async_trait;
use cua_driver_contract::HotkeyInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    focus_by_pixel, focus_cached_element_for_foreground, position_keyboard_cursor, ToolState,
};

// ── hotkey ────────────────────────────────────────────────────────────────────

fn is_modifier(k: &str) -> bool {
    matches!(
        k.to_lowercase().as_str(),
        "ctrl" | "control" | "shift" | "alt" | "win" | "windows" | "cmd" | "command"
    )
}

pub struct HotkeyTool {
    pub(crate) state: Arc<ToolState>,
}
static HOTKEY_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for HotkeyTool {
    fn def(&self) -> &ToolDef {
        HOTKEY_DEF.get_or_init(|| ToolDef {
            name: "hotkey".into(),
            // Description ported from Swift `HotkeyTool.swift` with the
            // Windows-specific dispatch (UIA accelerator for XAML; SendInput
            // for legacy Win32 with modifiers per #1614 / #1618; PostMessage
            // for modifier-less keys).
            description: "Press a combination of keys simultaneously — e.g. `[\"ctrl\", \"c\"]` \
                for Copy, `[\"ctrl\", \"shift\", \"t\"]` for reopen-closed-tab. Dispatch is \
                target-aware:\n\n\
                - **Modern XAML / WinUI / UWP targets** route through UI Automation: the driver \
                walks the target's accessibility subtree, finds a descendant whose AcceleratorKey \
                matches the combo, and invokes it. No focus steal, no system-queue input.\n\n\
                - **Legacy Win32 targets with modifiers** (Ctrl+S, Alt+Tab, etc.) route through \
                `SendInput` against the system input queue, with a brief `SetForegroundWindow` \
                swap so the events land on the target. This path is necessary because \
                PostMessage(WM_KEYDOWN, VK_CONTROL) does NOT update the OS-wide modifier state \
                visible to `GetKeyState` / `TranslateAccelerator`, so Win32 apps that bind \
                accelerators via `TranslateAccelerator` (LibreOffice, FAR, classic Notepad, etc.) \
                never see the combo as a real accelerator on the PostMessage-only path. \
                Trade-off: brief foreground swap (mitigated by restoring the previous foreground \
                after the keystrokes flush). Requires the daemon to have UIAccess integrity so \
                `SetForegroundWindow` is permitted — the MCP proxy auto-prefers the \
                `cua-driver-uia.exe` worker pipe when both daemons are running.\n\n\
                - **Legacy Win32 targets without modifiers** (plain `enter`, `tab`, `f5`, etc.) \
                route through `PostMessage(WM_KEYDOWN/UP)` — no focus steal, no need to update \
                modifier state.\n\n\
                The target does NOT need to be frontmost in any branch.\n\n\
                **`window_id`** (optional): explicit HWND when the pid owns more than one \
                window; otherwise the pid's first visible window is used.\n\n\
                Recognized modifiers: ctrl/control, shift, alt, win/windows. Non-modifier \
                keys use the same vocabulary as `press_key` (return, tab, escape, \
                up/down/left/right, space, delete, home, end, pageup, pagedown, f1-f12, \
                letters, digits). Order: modifiers first, one non-modifier last.".into(),
            input_schema: json!({
                "type":"object","required":["keys"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "scope":{"type":"string","enum":["window","desktop"],"description":"Use desktop with no pid/window_id to send the hotkey to the current foreground application."},
                    "pid":{"type":"integer","description":"Target process ID."},
                    "keys":{"type":"array","items":{"type":"string"},"minItems":2,
                        "description":"Modifier(s) and one non-modifier key, e.g. [\"ctrl\", \"c\"]."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "x":{"type":"number","description":"Window-local screenshot-pixel X — the element px action form: pixel-click there to focus, then send the combo (so e.g. Ctrl+V pastes into that field). Pass with y. Use for Chromium/Electron surfaces the background combo can't reach."},
                    "y":{"type":"number","description":"Window-local screenshot-pixel Y (see x)."},
                    "window_id":{"type":"integer","description":"Explicit HWND when the pid owns multiple windows."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<HotkeyInput>("hotkey", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            if input.keys.len() < 2 {
                return ToolResult::error("hotkey.keys must contain at least two keys.")
                    .with_structured(json!({ "code": "invalid_arguments" }));
            }
            let mods: Vec<String> = input
                .keys
                .iter()
                .filter(|key| is_modifier(key))
                .cloned()
                .collect();
            let Some(key) = input
                .keys
                .iter()
                .rev()
                .find(|key| !is_modifier(key))
                .cloned()
            else {
                return ToolResult::error("keys must include at least one non-modifier key.");
            };
            let full_keys = input.keys;
            let hwnd = match crate::input::mouse::foreground_window() {
                Ok(hwnd) => hwnd,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            let key_display = full_keys.join("+");
            return match tokio::task::spawn_blocking(move || {
                let modifiers: Vec<&str> = mods.iter().map(String::as_str).collect();
                crate::input::send_key_synthesized(hwnd, &key, &modifiers)
            })
            .await
            {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Sent {key_display} to the foreground application (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SendInput", "keys": full_keys,
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("desktop hotkey task failed: {error}")),
            };
        }
        // Parse keys array (Swift's only path).  Legacy key+modifiers shape
        // still accepted as a Rust-side convenience.
        let (key, mods, full_keys) = if let Some(arr) = args.get("keys") {
            let raw = match arr.as_array() {
                Some(r) => r,
                None => return ToolResult::error("Missing required array field keys."),
            };
            let keys: Vec<String> = raw
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if keys.len() != raw.len() || keys.is_empty() {
                return ToolResult::error("keys must be a non-empty array of strings.");
            }
            let modifiers: Vec<String> = keys.iter().filter(|k| is_modifier(k)).cloned().collect();
            let non_mods: Vec<String> = keys.iter().filter(|k| !is_modifier(k)).cloned().collect();
            if non_mods.is_empty() {
                return ToolResult::error("keys must include at least one non-modifier key.");
            }
            (non_mods.last().unwrap().clone(), modifiers, keys)
        } else if let Some(k) = args.get("key").and_then(|v| v.as_str()) {
            let m: Vec<String> = args
                .get("modifiers")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let mut full = m.clone();
            full.push(k.to_owned());
            (k.to_owned(), m, full)
        } else {
            return ToolResult::error("Missing required array field keys.");
        };
        let raw_pid = match args.require_i64("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pid = raw_pid as u32;
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(resolved) => resolved,
            Err(error) => return error,
        };
        let (elem_idx, hwnd_opt, admitted) = resolved.into_parts(args.opt_u64("window_id"));
        let delivery = DeliveryMode::from_args(&args);

        // Resolve HWND: use window_id if given, else pick first window for pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let pid2 = pid;
                let windows = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::win32::list_windows(Some(pid2))
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

        // Match Swift's text format exactly: `"✅ Pressed K1+K2+... on pid X."`
        // where K1+K2+... is `keys.joined(separator: "+")` of the FULL keys
        // array as the caller supplied it (modifiers + non-modifier in order).
        let key_display = full_keys.join("+");

        // px form: pixel-click to focus, then the combo acts on the focused field
        // (e.g. Ctrl+V into a Chromium input). A background pixel focus does not
        // make Chromium's modifier PostMessage path reliable, so it still goes
        // through the capability guard below; foreground remains explicit.
        let px_focus = {
            let px = args.get("x").and_then(|v| v.as_f64());
            let py = args.get("y").and_then(|v| v.as_f64());
            if let (Some(cx), Some(cy)) = (px, py) {
                if elem_idx.is_some() {
                    return ToolResult::error(
                        "Pass either element_token (ax) or x,y (px) to hotkey, not both.",
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
                true
            } else {
                false
            }
        };

        // The px form's focus click already moved the cursor. Otherwise place
        // a named session's cursor so a keyboard-first session stays visible.
        if !px_focus {
            let element_center = admitted
                .as_ref()
                .map(|element| (element.center.0 as f64, element.center.1 as f64));
            position_keyboard_cursor(&self.state, &args, hwnd, element_center).await;
        }

        if !px_focus && crate::input::is_xaml_host_hwnd(hwnd) {
            let mut accelerator_keys = mods.clone();
            accelerator_keys.push(key.clone());
            let accelerator_combo = accelerator_keys.join("+");
            let key_display_for_result = key_display.clone();
            // UIA cross-process subtree walks can wedge on a bad provider —
            // bound the call so a hung provider returns an error instead of
            // blocking the daemon indefinitely. 4 s matches the budget the
            // rest of this file uses for similar UIA scans.
            let result = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    crate::uia::windows_enum::try_invoke_accelerator_in_window(
                        hwnd as isize,
                        &accelerator_combo,
                    )
                }
            })
            .await;
            return match result {
                Ok(Ok((true, _))) => ToolResult::text(format!(
                    "✅ Pressed {key_display_for_result} on pid {raw_pid} via UIA \
                     InvokePattern/TogglePattern (XAML / UWP target)."
                )),
                Ok(Ok((false, scanned))) => ToolResult::error(format!(
                    "hotkey on a modern XAML / UWP target (pid {raw_pid}, hwnd {hwnd}) \
                     could not find a UIA AcceleratorKey or `(Ctrl+X)`-style name hint \
                     matching `{key_display_for_result}` (scanned {scanned} element(s)). \
                     PostMessage WM_KEYDOWN/UP is ignored by this target's input pipeline. \
                     Common cause: the action is nested behind a closed menu (e.g. modern \
                     Notepad's Save under the File menu) — its element isn't in the visible \
                     subtree until the menu is opened. Call \
                     `get_window_state(pid={raw_pid}, window_id={hwnd})` to inspect available \
                     UIA actions, then use an exposed accelerator or `click` the matching \
                     element directly."
                )),
                Ok(Err(e)) => ToolResult::error(format!("hotkey (UIA path): {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Non-XAML target. Three routing decisions:
        //   1. PostMessage WM_KEYDOWN/UP — no foreground swap, no focus
        //      theft. Modifier state is encoded in the LPARAM bits, not
        //      in GetKeyState, so apps that read modifier state directly
        //      from the message (Chromium's `Browser::HandleKeyboardEvent`,
        //      Chromium renderer's input handler, every web app) see
        //      Ctrl+X correctly. Apps that consult `GetKeyState` via
        //      `TranslateAccelerator` (LibreOffice, FAR, classic Notepad)
        //      DON'T fire the accelerator — the modifier looks unset to
        //      them.
        //   2. SendInput synthesized hotkey — pushes events onto the
        //      *system input queue*. Updates GetKeyState system-wide, so
        //      TranslateAccelerator-based apps see Ctrl+S as a real
        //      accelerator. Trade-off: brief foreground swap so SendInput
        //      lands on the right HWND. **This is the only path that
        //      violates the no-foreground contract** — we minimise its
        //      use to apps that genuinely require it.
        //
        // Routing:
        //   - No modifiers → PostMessage (no accelerator concern).
        //   - Modifiers + Chromium-family target → PostMessage. Chromium
        //     processes accelerators in `Browser::HandleKeyboardEvent`,
        //     which reads modifier state from the WM_KEYDOWN LPARAM
        //     directly (not via GetKeyState). Every Chromium browser
        //     (Chrome/Edge/Brave/Arc/Vivaldi) AND every Electron app
        //     (Slack/VS Code/Discord/Teams/Notion) detects as Chromium
        //     via `is_chromium_target_window` and gets the no-foreground
        //     path.
        //   - Modifiers + non-Chromium Win32 → SendInput (the
        //     TranslateAccelerator path). The foreground swap stays.
        let has_modifiers = !mods.is_empty();
        // delivery_mode:"background" — refuse if PostMessage of the key combo
        // would be silently dropped on this target (Chromium with modifiers).
        let event_kind = if has_modifiers {
            EventKind::KeyCombo
        } else {
            EventKind::Keystroke
        };
        if delivery == DeliveryMode::Background
            && (px_focus || crate::input::delivery::would_be_silently_dropped(hwnd, event_kind))
        {
            // macOS-aligned: a `background` hotkey never fronts. This combo would
            // be silently dropped (VCL/classic-Win32 TranslateAccelerator, or
            // Chromium key-combos) and only a foreground/focus grab lands it —
            // which background must not do. Surface background_unavailable; the
            // agent escalates to delivery_mode:"foreground".
            return crate::input::delivery::background_unavailable_error(hwnd, event_kind);
        }
        // delivery_mode:"foreground" — explicit SendInput swap (the path that
        // unblocks TranslateAccelerator-style apps). A Background call that
        // reaches here means the key combo is NOT silently dropped on this
        // target (the drop-check above returned early otherwise), so it stays
        // on PostMessage and the no-foreground contract holds.
        // Foreground is an explicit request for system-queue delivery. This is
        // still required after a PX focus click: PostMessage does not update
        // global modifier state, so Chromium never observes Ctrl+Shift+H as a
        // chord even though the renderer control is focused.
        let use_send_input = delivery == DeliveryMode::Foreground;
        let focus_target = elem_idx.map(|idx| {
            let point = admitted.as_ref().map(|element| element.center);
            (idx, point)
        });
        let result = tokio::task::spawn_blocking({
            let admitted = admitted.clone();
            move || {
                let _admission = &admitted;
                let m: Vec<&str> = mods.iter().map(String::as_str).collect();
                if use_send_input {
                    crate::input::send_key_synthesized_after_focus(hwnd, &key, &m, || {
                        if let Some((idx, point)) = focus_target {
                            focus_cached_element_for_foreground(&admitted, hwnd, idx, point)?;
                        }
                        Ok(())
                    })
                } else {
                    crate::input::post_key(hwnd, &key, &m)
                }
            }
        })
        .await;
        let path = if use_send_input {
            "SendInput"
        } else {
            "PostMessage"
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Pressed {key_display} on pid {raw_pid} via {path} \
                 (Win32 target)."
            )),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
