use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    element_target_rect, overlay_glide_to_target, pin_overlay_above, resolve_cursor_key, ToolState,
};

// ── set_value ─────────────────────────────────────────────────────────────────

pub struct SetValueTool {
    pub(crate) state: Arc<ToolState>,
}

static SET_VALUE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetValueTool {
    fn def(&self) -> &ToolDef {
        SET_VALUE_DEF.get_or_init(|| ToolDef {
            name: "set_value".into(),
            // Description ported from Swift `SetValueTool.swift` with the
            // Windows transport (UIA ValuePattern instead of AXValue).
            // Native Win32 combo boxes are handled by crate::win32::combo
            // (the analogue of Swift's AXPopUpButton special case).
            description: "Set a value on a UIA element via the ValuePattern interface.\n\n\
                Two semantic modes (matching Swift's split):\n\
                - **Standard input** (text fields, sliders, combo box edit): writes the value \
                  directly through UIA `IUIAutomationValuePattern::SetValue`. This is the \
                  canonical Windows write path, equivalent to Swift's `AXValue` write.\n\
                - **ComboBox**: on a native Win32 combo box (WinForms, MFC, dialogs) selects \
                  the option whose text matches `value` and sends the app the notifications a \
                  user's choice sends (CBN_SELENDOK, CBN_SELCHANGE), without focusing the window; \
                  an editable combo gets the text typed into its edit field. A drop-down list \
                  without that option is refused with the list of options. Other combo boxes get \
                  a UIA value write, which the app may not treat as a selection.\n\n\
                For free-form text entry that the target accepts via keystrokes only, prefer \
                `type_text` — UIA ValuePattern writes are ignored by some web inputs (same \
                caveat as Swift's `AXValue`-vs-WebKit).".into(),
            input_schema: json!({
                "type":"object","required":["value"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID. Optional with element_token (the token carries the pid)."},
                    "window_id":{"type":"integer","description":"HWND of the window. Omit when element_token is supplied (the token carries it)."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "value":{"type":"string","description":"New value. UIA will coerce to the element's native type."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            // Swift: idempotent: true (setting same value twice is idempotent).
            read_only: false, destructive: true, idempotent: true, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_key = resolve_cursor_key(&args);
        let raw_pid = args.get("pid").and_then(|v| v.as_i64());
        if raw_pid.is_none() {
            return ToolResult::error("Missing required integer field pid.");
        }
        let pid = raw_pid.unwrap() as u32;
        let value = match args.get("value").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None => return ToolResult::error("Missing required string field value."),
        };
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (index, window_id, admitted) = resolved.into_parts(None);
        let (Some(hwnd), Some(idx), Some(admitted)) = (window_id, index, admitted) else {
            return ToolResult::error("set_value requires an admitted element target.");
        };
        if !admitted.is_uia() {
            return ToolResult::error(
                "set_value requires a UIA element; MSAA value writes are unsupported.",
            );
        }

        // No-raise guard: a WPF/XAML automation peer calls UIElement.Focus() →
        // SetForegroundWindow during ValuePattern.SetValue. WS_EX_NOACTIVATE on
        // the target makes that a no-op while the value is still set, so a
        // background SetValue can't steal the user's foreground. Held across the
        // whole write. (No-op for delivery_mode:"foreground".)
        let _noact = if crate::input::delivery::DeliveryMode::from_args(&args)
            != crate::input::delivery::DeliveryMode::Foreground
        {
            Some(crate::input::NoActivateGuard::arm(
                windows::Win32::Foundation::HWND(hwnd as *mut _),
            ))
        } else {
            None
        };

        // Glide the agent cursor onto the target element before writing its
        // value, so a value write gets the same visual feedback as a click —
        // the viewer can see *where* the agent is acting. No-op when the
        // overlay is disabled or the element has no cached center.
        {
            let (cx, cy) = admitted.center;
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to_target(
                &cursor_key,
                cx as f64,
                cy as f64,
                element_target_rect(Some(&admitted), (0, 0)),
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

        let value_for_task = value.clone();
        let result = tokio::task::spawn_blocking({
            move || -> anyhow::Result<SetValueRoute> {
                let value = value_for_task;
                let _noact = _noact;
                let ptr = admitted.as_ptr();
                use windows::core::{Interface, BSTR};
                use windows::Win32::UI::Accessibility::{
                    IUIAutomationElement, IUIAutomationValuePattern, UIA_ComboBoxControlTypeId,
                    UIA_ValuePatternId,
                };
                let elem = std::mem::ManuallyDrop::new(unsafe {
                    IUIAutomationElement::from_raw(ptr as *mut _)
                });
                // A native Win32 combo box (or its edit field): select the
                // option and notify the app the way the control does. A UIA
                // value write would only change the displayed text.
                let native = unsafe { elem.CurrentNativeWindowHandle() }
                    .ok()
                    .and_then(|h| crate::win32::combo::combo_for_window(h.0 as isize));
                if let Some(combo) = native {
                    let write = crate::win32::combo::set_value(combo, &value)?;
                    return Ok(SetValueRoute::Win32Combo { combo, write });
                }
                let is_combo = unsafe { elem.CurrentControlType() }
                    .is_ok_and(|control_type| control_type == UIA_ComboBoxControlTypeId);
                // Try ValuePattern first (text inputs, editable combos, etc).
                // The SetValue is shielded by the EnableWindow bypass: a
                // Chromium/Electron (or XAML) SetValue handler self-foregrounds via
                // SetForegroundWindow, which WS_EX_NOACTIVATE alone does not stop;
                // disabling the host for the duration does, while the a11y-channel
                // write still lands. (No-op shield for non-XAML/non-Chromium hosts.)
                if let Ok(pattern) = unsafe { elem.GetCurrentPattern(UIA_ValuePatternId) } {
                    if let Ok(vp) = pattern.cast::<IUIAutomationValuePattern>() {
                        let set =
                            crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                                vp.SetValue(&BSTR::from(value.as_str()))
                            });
                        if set.is_ok() {
                            return Ok(if is_combo {
                                SetValueRoute::ComboValuePattern
                            } else {
                                SetValueRoute::Pattern("ValuePattern")
                            });
                        }
                    }
                }
                // Fall through to RangeValuePattern for Sliders / ProgressBars /
                // numeric ranges. RangeValue.SetValue takes a double, so coerce
                // the string. Documented gap-closer (PR #1699 harness exposed).
                use windows::Win32::UI::Accessibility::{
                    IUIAutomationRangeValuePattern, UIA_RangeValuePatternId,
                };
                if let Ok(pattern) = unsafe { elem.GetCurrentPattern(UIA_RangeValuePatternId) } {
                    if let Ok(rv) = pattern.cast::<IUIAutomationRangeValuePattern>() {
                        let parsed: f64 = value.parse().map_err(|_| {
                            anyhow::anyhow!(
                                "set_value: target element exposes RangeValuePattern (Slider / \
                         ProgressBar / numeric range). `value` must be a parseable f64; \
                         got {value:?}."
                            )
                        })?;
                        crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                            rv.SetValue(parsed)
                        })?;
                        return Ok(SetValueRoute::Pattern("RangeValuePattern"));
                    }
                }
                anyhow::bail!(
                    "set_value: element [{idx}] does not implement ValuePattern or \
                 RangeValuePattern. For controls with TogglePattern (CheckBox) or \
                 SelectionItemPattern (RadioButton / ComboBoxItem), use the `click` \
                 tool instead."
                );
            }
        })
        .await;
        match result {
            Ok(Ok(SetValueRoute::Pattern(pattern_name))) => {
                ToolResult::text(format!("✅ Set AXValue on [{idx}] (UIA {pattern_name})."))
            }
            Ok(Ok(SetValueRoute::ComboValuePattern)) => ToolResult::text(format!(
                "Set the UIA value of combo box [{idx}] to '{value}'. This combo box is not a \
                 native Win32 one, and a UIA value write may change the shown text without \
                 the app treating it as a selection. Check with get_window_state; if the app \
                 did not react, click the combo box and then the option."
            ))
            .with_structured(json!({ "path": "ax", "verified": false, "effect": "unverifiable" })),
            Ok(Ok(SetValueRoute::Win32Combo { combo, write })) => {
                win32_combo_set_value_result(idx, &value, combo, write, &args)
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// How `set_value` wrote the value.
enum SetValueRoute {
    /// A UIA pattern on a non-combo element (`ValuePattern`, `RangeValuePattern`).
    Pattern(&'static str),
    /// UIA `ValuePattern` on a combo box that has no native Win32 window.
    ComboValuePattern,
    /// Win32 combo messages plus the app's change notifications.
    Win32Combo {
        combo: crate::win32::combo::Combo,
        write: crate::win32::combo::ComboWrite,
    },
}

fn win32_combo_set_value_result(
    idx: usize,
    value: &str,
    combo: crate::win32::combo::Combo,
    write: crate::win32::combo::ComboWrite,
    args: &Value,
) -> ToolResult {
    use crate::win32::combo::ComboWrite;
    use cua_driver_core::action_record::{
        ActionEffect, ActionEvidence, ActionExecutionRecordBuilder, ActionTransport,
        ActualDelivery, EvidenceKind, RequestedDelivery,
    };
    let kind = if combo.editable {
        "editable combo box"
    } else {
        "drop-down combo box"
    };
    let (selected, unchanged, readback_text, readback_index) = match &write {
        ComboWrite::NoSuchOption { options } => {
            let message = format!(
                "set_value: the {kind} [{idx}] has no option '{value}'; nothing was changed. \
                 Options: {options:?}."
            );
            return ToolResult::error(message).with_structured(json!({
                "code": "option_not_found",
                "effect": "none",
                "options": options,
            }));
        }
        ComboWrite::Applied {
            selected,
            unchanged,
            readback_text,
            readback_index,
        } => (
            *selected,
            *unchanged,
            readback_text.clone(),
            *readback_index,
        ),
    };
    let verified = crate::win32::combo::write_verified(&write, value);
    let seen = readback_text.clone().unwrap_or_default();
    let how = match (combo.editable, selected) {
        (false, _) => "CB_SETCURSEL, then CBN_SELENDOK and CBN_SELCHANGE to the app",
        (true, Some(_)) => {
            "typed into its edit field, then CB_SETCURSEL with CBN_SELENDOK and CBN_SELCHANGE \
             to the app"
        }
        (true, None) => "typed into its edit field (the app gets CBN_EDITCHANGE)",
    };
    let text = if unchanged {
        format!("✅ The {kind} [{idx}] already reads '{seen}'; nothing was sent.")
    } else if verified {
        format!("✅ Set the {kind} [{idx}] to '{seen}' ({how}); read back '{seen}'.")
    } else {
        format!(
            "Sent '{value}' to the {kind} [{idx}] ({how}), but it reads back '{seen}' \
             (selected index {readback_index:?}). Check with get_window_state."
        )
    };
    let requested = match args.get("delivery_mode").and_then(Value::as_str) {
        Some("foreground") => RequestedDelivery::Foreground,
        _ => RequestedDelivery::Background,
    };
    let effect = if verified {
        ActionEffect::Confirmed
    } else {
        ActionEffect::Unverifiable
    };
    let mut record =
        ActionExecutionRecordBuilder::new(effect, ActionTransport::WindowsPostMessage, requested)
            // Window messages reach the control without focusing or raising it.
            .actual_delivery(ActualDelivery::Background)
            .detail(text.clone());
    if verified {
        record = record.evidence(ActionEvidence {
            kind: EvidenceKind::ValueReadback,
            detail: format!("combo box reads back '{seen}' (index {readback_index:?})"),
        });
    }
    match record.build() {
        Ok(record) => ToolResult::text(text).with_action_record(record),
        Err(error) => ToolResult::error(format!("set_value: invalid action record: {error:?}")),
    }
}
