// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Turn an action the driver could not confirm into an error, per call.
//!
//! Every action result carries an `effect`: `confirmed` when a readback, a
//! window change or a native API result proved it, and `unverifiable`,
//! `suspected_noop` or `partial` when nothing did. Those results are still
//! successes, because many input routes (a background pixel click, synthesized
//! keys into a web view) have no way to read their effect back. An agent that
//! only checks `isError` then carries on as if the action had worked.
//!
//! The optional argument [`REQUIRE_VERIFIED_ARG`] lets a caller ask for the
//! strict reading: with `true`, an action whose effect is anything but
//! `confirmed` comes back as an error with code [`ACTION_UNVERIFIED_CODE`].
//! The input was still delivered, so the error says so (`execution_state:
//! "delivered"`) and keeps the effect, the summary and the recovery hint;
//! the caller reads the target before deciding to repeat it. Omitted or
//! `false` keeps the current behavior.
//!
//! The dispatch boundary reads the argument on every action tool and applies
//! it to the published result, so it means the same on every platform and
//! inside `run_steps` and `run_script`, where a strict step stops the batch.

use serde_json::{json, Value};

/// The per-call argument name.
pub const REQUIRE_VERIFIED_ARG: &str = "require_verified";

/// The error code of an action that ran but was not confirmed.
pub const ACTION_UNVERIFIED_CODE: &str = "action_unverified";

/// Whether `tool` accepts [`REQUIRE_VERIFIED_ARG`]: every tool whose success
/// is the shared action result.
pub fn accepts(tool: &str) -> bool {
    cua_driver_contract::is_action_result_tool(tool)
}

/// The JSON schema of [`REQUIRE_VERIFIED_ARG`].
pub fn schema() -> Value {
    json!({
        "type": "boolean",
        "description": "When true, an action the driver could not confirm (effect unverifiable, \
                        suspected_noop or partial) returns an error with code action_unverified \
                        instead of a success. The input was still sent: read the target before \
                        repeating it. Default false."
    })
}

/// Read the caller's choice from a `tool` call's arguments. `Err` carries the
/// message and structured content of the `invalid_arguments` refusal.
pub fn from_args(tool: &str, args: &Value) -> Result<bool, (String, Value)> {
    match args.get(REQUIRE_VERIFIED_ARG) {
        None => Ok(false),
        Some(Value::Bool(required)) => Ok(*required),
        Some(other) => {
            let detail = format!("require_verified must be true or false, got {other}");
            Err((
                format!("{tool}: {detail}"),
                json!({ "code": "invalid_arguments", "tool": tool, "detail": detail }),
            ))
        }
    }
}

/// The error for a published action result whose effect is not `confirmed`,
/// or `None` when it was confirmed or refused (a refusal is already an
/// error). `published` is the action result's structured content.
pub fn unverified_error(tool: &str, published: &Value) -> Option<(String, Value)> {
    let effect = published.get("effect").and_then(Value::as_str)?;
    if matches!(effect, "confirmed" | "refused") {
        return None;
    }
    let mut message = format!(
        "{tool}: the input was sent but the driver could not confirm its effect ({effect}), and \
         require_verified is set. Read the target before repeating it."
    );
    if let Some(summary) = published.get("summary").and_then(Value::as_str) {
        message.push_str("\n\n");
        message.push_str(summary);
    }
    let mut structured = json!({
        "code": ACTION_UNVERIFIED_CODE,
        "execution_state": "delivered",
        "tool": tool,
        "effect": effect,
    });
    for key in ["route", "delivery", "summary", "hint", "next_call"] {
        if let Some(value) = published.get(key) {
            structured[key] = value.clone();
        }
    }
    Some((message, structured))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_and_false_keep_unverified_successes() {
        for args in [json!({}), json!({"require_verified": false})] {
            assert!(!from_args("click", &args).unwrap());
        }
        assert!(from_args("click", &json!({"require_verified": true})).unwrap());
    }

    #[test]
    fn other_values_are_invalid() {
        for value in [json!(null), json!("true"), json!(1)] {
            let (_, structured) =
                from_args("click", &json!({"require_verified": value})).unwrap_err();
            assert_eq!(structured["code"], "invalid_arguments", "{value}");
        }
    }

    #[test]
    fn only_a_confirmed_or_refused_effect_passes() {
        for effect in ["confirmed", "refused"] {
            assert_eq!(unverified_error("click", &json!({"effect": effect})), None);
        }
        for effect in ["unverifiable", "suspected_noop", "partial"] {
            let (message, structured) = unverified_error(
                "type_text",
                &json!({
                    "effect": effect,
                    "route": "synthetic_events",
                    "summary": "Sent 3 char(s)",
                    "hint": "Read the field.",
                }),
            )
            .unwrap();
            assert_eq!(structured["code"], ACTION_UNVERIFIED_CODE);
            assert_eq!(structured["execution_state"], "delivered");
            assert_eq!(structured["effect"], effect);
            assert_eq!(structured["route"], "synthetic_events");
            assert_eq!(structured["hint"], "Read the field.");
            assert!(message.contains(effect) && message.ends_with("Sent 3 char(s)"));
        }
    }

    #[test]
    fn every_action_result_tool_accepts_it() {
        for tool in cua_driver_contract::ACTION_RESULT_TOOLS {
            assert!(accepts(tool), "{tool}");
        }
        assert!(!accepts("get_window_state"));
    }
}
