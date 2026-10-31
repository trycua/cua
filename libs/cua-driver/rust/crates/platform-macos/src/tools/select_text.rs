// SPDX-License-Identifier: MIT
//! select_text tool: move a text element's selection or caret onto a string.
//!
//! The matching rules (exact text, `prefix`/`suffix` disambiguation, UTF-16
//! ranges) live in `cua_driver_core::text_selection`; this adapter resolves the
//! element token, writes `AXSelectedTextRange` and reads it back.

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    text_selection::{self as sel, SelectionType, Utf16Range},
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::ax::bindings::{
    copy_range_attr, copy_string_attr, is_attribute_settable, kAXErrorSuccess, set_range_attr,
    AXUIElementRef, CFRangeValue,
};

use super::ToolState;

const SELECTED_RANGE: &str = "AXSelectedTextRange";

pub struct SelectTextTool {
    state: Arc<ToolState>,
}

impl SelectTextTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| sel::tool_def(sel::MACOS_DESCRIPTION.to_owned()))
}

struct Request {
    text: String,
    prefix: Option<String>,
    suffix: Option<String>,
    selection_type: SelectionType,
}

#[async_trait]
impl Tool for SelectTextTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match super::target_pid(&self.state, &args) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let text = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let selection_type =
            match SelectionType::parse(args.get("selection_type").and_then(Value::as_str)) {
                Ok(kind) => kind,
                Err(message) => return ToolResult::error(message),
            };
        let request = Request {
            text,
            prefix: args
                .get("prefix")
                .and_then(Value::as_str)
                .map(str::to_owned),
            suffix: args
                .get("suffix")
                .and_then(Value::as_str)
                .map(str::to_owned),
            selection_type,
        };

        let (element_index, window_id, element_guard) =
            match self.state.snapshots.resolve(pid, &args) {
                Ok(cua_driver_core::element_token::ResolvedElement::None) => {
                    return ToolResult::error(
                        "select_text requires element_token to address the target element.",
                    )
                }
                Ok(cua_driver_core::element_token::ResolvedElement::Element {
                    window_id,
                    element_index,
                    element,
                }) => match u32::try_from(window_id) {
                    Ok(window_id) => (element_index, window_id, element),
                    Err(_) => return ToolResult::error("window_id is out of range for macOS."),
                },
                Err(refusal) => return refusal,
            };

        // Like set_value: an always-background semantic AX mutation. Re-prove
        // the retained element still belongs to the exact window first.
        let _mutation_lease = match super::gate_background_window_action(
            pid,
            window_id,
            Some(element_guard.as_ptr()),
            cua_driver_core::background_input::BackgroundAction::AxSemantic,
        )
        .await
        {
            Ok(lease) => lease,
            Err(refusal_result) => return refusal_result,
        };

        match tokio::task::spawn_blocking(move || unsafe {
            select_blocking(element_guard.as_ptr() as AXUIElementRef, &request)
        })
        .await
        {
            Ok(Ok(mut structured)) => {
                structured["pid"] = json!(pid);
                structured["window_id"] = json!(window_id);
                structured["element_index"] = json!(element_index);
                let range = &structured["range"];
                let msg = format!(
                    "Selected {} at UTF-16 range {{location: {}, length: {}}} ({}); read back and verified.",
                    match structured["selection_type"].as_str() {
                        Some("text") => format!("{:?}", structured["selected_text"].as_str().unwrap_or_default()),
                        Some(kind) => format!("caret ({kind})"),
                        None => String::new(),
                    },
                    range["location"],
                    range["length"],
                    structured["role"].as_str().unwrap_or("element"),
                );
                ToolResult::text(msg).with_structured(structured)
            }
            Ok(Err(refusal)) => refusal,
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

fn range_json(range: Utf16Range) -> Value {
    json!({ "location": range.location, "length": range.length })
}

fn cf_range_json(range: Option<CFRangeValue>) -> Value {
    match range {
        Some(r) => json!({ "location": r.location, "length": r.length }),
        None => Value::Null,
    }
}

/// Locate, write and read back the selection.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
unsafe fn select_blocking(element: AXUIElementRef, request: &Request) -> Result<Value, ToolResult> {
    let role = copy_string_attr(element, "AXRole");
    let role_json = json!(role.as_deref());
    if !is_attribute_settable(element, SELECTED_RANGE) {
        return Err(sel::refusal(
            sel::CODE_SELECTION_NOT_SETTABLE,
            format!(
                "the element ({}) has no settable AXSelectedTextRange; target a text field or text area",
                role.as_deref().unwrap_or("unknown role")
            ),
            json!({ "path": "ax", "role": role_json }),
        ));
    }
    let Some(value) = copy_string_attr(element, "AXValue") else {
        return Err(sel::refusal(
            sel::CODE_SELECTION_NOT_SETTABLE,
            "the element exposes no text AXValue to search",
            json!({ "path": "ax", "role": role_json }),
        ));
    };
    let has_context = request.prefix.as_deref().is_some_and(|p| !p.is_empty())
        || request.suffix.as_deref().is_some_and(|s| !s.is_empty());
    let located = sel::locate_selection(
        &value,
        &request.text,
        request.prefix.as_deref(),
        request.suffix.as_deref(),
        request.selection_type,
    )
    .map_err(|error| sel::locate_refusal(&error, has_context))?;

    let previous = copy_range_attr(element, SELECTED_RANGE);
    let wanted = CFRangeValue {
        location: located.range.location as isize,
        length: located.range.length as isize,
    };
    let err = set_range_attr(element, SELECTED_RANGE, wanted);
    let read_back = copy_range_attr(element, SELECTED_RANGE);
    if err != kAXErrorSuccess || read_back != Some(wanted) {
        return Err(sel::refusal(
            sel::CODE_SELECTION_NOT_APPLIED,
            format!(
                "AXSelectedTextRange write returned {err}; the read-back {} does not match the requested range",
                cf_range_json(read_back)
            ),
            json!({
                "path": "ax",
                "role": role_json,
                "ax_error": err,
                "requested_range": range_json(located.range),
                "read_back_range": cf_range_json(read_back),
                "previous_range": cf_range_json(previous),
            }),
        ));
    }
    let selected_text = sel::utf16_slice(&value, located.range).unwrap_or_default();
    Ok(json!({
        "path": "ax",
        "effect": "confirmed",
        "verified": true,
        "role": role_json,
        "selection_type": request.selection_type.as_str(),
        "range": range_json(located.range),
        "match_range": range_json(located.match_range),
        "selected_text": selected_text,
        "previous_range": cf_range_json(previous),
        "value_length": sel::utf16_len(&value),
    }))
}
