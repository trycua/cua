// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-call input delivery mode and the shared `background_unavailable`
//! refusal envelope.
//!
//! Every platform accepts the same two `delivery_mode` rungs, parsed the same
//! way, and refuses an impossible background delivery with the same
//! foreground-escalation contract. Platform adapters keep only the
//! platform-specific reason text and fields.

use serde_json::{json, Value};

use crate::protocol::ToolResult;

/// Input delivery modality: the agent-selected rung of the
/// best-effort-background ladder, passed per call (never a stored or
/// configured setting).
///
/// - `BackgroundPreferred` (default, `delivery_mode` omitted): inject without
///   activating or raising the target. When the platform cannot do that, the
///   tool refuses with [`background_unavailable_result`] instead of fronting,
///   unless the operator allowed a brief foreground step or `run_steps` was
///   given `foreground_fallback`.
/// - `BackgroundStrict` (explicit `"background"`): the same injection, and the
///   call opts out of every escalation: no brief foreground step and no
///   `run_steps` foreground fallback.
/// - `Foreground`: activate the target for the action. This is the agent's
///   explicit last resort.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum DeliveryMode {
    #[default]
    BackgroundPreferred,
    BackgroundStrict,
    Foreground,
}

impl DeliveryMode {
    /// Parse the per-call `delivery_mode` argument. Only an explicit
    /// case-insensitive `"foreground"` fronts. An explicit `"background"` is
    /// strict; anything else, omitted, garbage or a removed legacy value
    /// (`"auto"`), is preferred background, so it never silently fronts.
    pub fn parse(arg: Option<&str>) -> Self {
        match arg {
            Some(s) if s.eq_ignore_ascii_case("foreground") => Self::Foreground,
            Some(s) if s.eq_ignore_ascii_case("background") => Self::BackgroundStrict,
            _ => Self::BackgroundPreferred,
        }
    }

    /// Parse from a tool's JSON args, reading the `delivery_mode` field.
    pub fn from_args(args: &Value) -> Self {
        Self::parse(args.get("delivery_mode").and_then(Value::as_str))
    }

    pub fn is_foreground(self) -> bool {
        matches!(self, Self::Foreground)
    }

    pub fn is_background(self) -> bool {
        !self.is_foreground()
    }

    /// Whether a background refusal may escalate to the foreground on the
    /// call's behalf: the operator's brief foreground step, or `run_steps`
    /// `foreground_fallback`. Only a call that left `delivery_mode` out may.
    pub fn allows_escalation(self) -> bool {
        matches!(self, Self::BackgroundPreferred)
    }
}

/// Environment variable an operator sets when starting the daemon to allow a
/// brief foreground step (trycua/cua#5478). Off by default.
pub const BRIEF_FOREGROUND_ENV: &str = "CUA_DRIVER_ALLOW_BRIEF_FOREGROUND";

/// Whether the operator allowed a brief foreground step for actions that
/// cannot finish in the background.
///
/// When it is on, a tool whose background attempt hits a known platform limit
/// (today: a macOS drag-and-drop session in a native window) repeats only that
/// step in the foreground, then hands the frontmost app and the pointer back,
/// and says so in its result. It is read from the daemon's environment, which
/// the agent can't change, and a call that passes `delivery_mode:"background"`
/// explicitly is never escalated ([`DeliveryMode::allows_escalation`]).
pub fn brief_foreground_allowed() -> bool {
    brief_foreground_flag(std::env::var(BRIEF_FOREGROUND_ENV).ok().as_deref())
}

/// Parse [`BRIEF_FOREGROUND_ENV`]: only `1`, `true`, `yes` or `on`
/// (case-insensitive) turn it on.
pub fn brief_foreground_flag(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// The `suggestion` every background refusal carries.
pub const FOREGROUND_RETRY_SUGGESTION: &str =
    "Retry this action with delivery_mode:\"foreground\".";

/// Build a structured background-delivery refusal.
///
/// The result is an error with `message` as its text. Its structured content
/// holds `code`, the shared [`FOREGROUND_RETRY_SUGGESTION`], and an
/// `escalation` that recommends `foreground` for `escalation_reason`, plus
/// every field of the `fields` object. `fields` cannot override those
/// contract keys. `effect` is `refused` unless `fields` names the effect of
/// input that was already sent.
pub fn background_unavailable_result(
    message: impl Into<String>,
    code: &str,
    escalation_reason: impl Into<String>,
    fields: Value,
) -> ToolResult {
    background_unavailable_result_with_suggestion(
        message,
        code,
        escalation_reason,
        fields,
        FOREGROUND_RETRY_SUGGESTION,
    )
}

/// Like [`background_unavailable_result`], but `suggestion` replaces the
/// foreground retry text. Use it when a working background route exists, so the
/// refusal leads the caller to that route instead of implying the task is
/// blocked. The foreground `escalation` is still included.
pub fn background_unavailable_result_with_suggestion(
    message: impl Into<String>,
    code: &str,
    escalation_reason: impl Into<String>,
    fields: Value,
    suggestion: &str,
) -> ToolResult {
    let mut structured = match fields {
        Value::Object(fields) => fields,
        _ => serde_json::Map::new(),
    };
    structured
        .entry("effect")
        .or_insert_with(|| json!("refused"));
    structured.insert("code".to_owned(), json!(code));
    structured.insert("suggestion".to_owned(), json!(suggestion));
    structured.insert(
        "escalation".to_owned(),
        json!({
            "recommended": "foreground",
            "reason": escalation_reason.into(),
        }),
    );
    ToolResult::error(message).with_structured(Value::Object(structured))
}

#[cfg(test)]
mod tests {
    #[test]
    fn brief_foreground_is_off_unless_explicitly_enabled() {
        for off in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("maybe"),
        ] {
            assert!(!super::brief_foreground_flag(off), "{off:?}");
        }
        for on in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(super::brief_foreground_flag(Some(on)), "{on:?}");
        }
    }

    use super::*;

    #[test]
    fn delivery_mode_parse_fronts_only_on_explicit_foreground() {
        use DeliveryMode::{BackgroundPreferred, BackgroundStrict, Foreground};
        for (args, expected) in [
            (json!({"delivery_mode": "background"}), BackgroundStrict),
            (json!({"delivery_mode": "Background"}), BackgroundStrict),
            (json!({"delivery_mode": "foreground"}), Foreground),
            (json!({"delivery_mode": "Foreground"}), Foreground),
            (json!({"delivery_mode": "FOREGROUND"}), Foreground),
            (json!({}), BackgroundPreferred),
            (json!({"delivery_mode": "garbage"}), BackgroundPreferred),
            (json!({"delivery_mode": "auto"}), BackgroundPreferred),
            (json!({"delivery_mode": null}), BackgroundPreferred),
            (json!({"delivery_mode": 1}), BackgroundPreferred),
        ] {
            let mode = DeliveryMode::from_args(&args);
            assert_eq!(mode, expected, "{args}");
            assert_eq!(mode.is_foreground(), expected == Foreground, "{args}");
            assert_eq!(mode.is_background(), expected != Foreground, "{args}");
        }
    }

    #[test]
    fn only_an_omitted_delivery_mode_allows_escalation() {
        // Omitted: the brief foreground step and run_steps foreground_fallback
        // may escalate a background refusal.
        assert!(DeliveryMode::from_args(&json!({})).allows_escalation());
        assert_eq!(DeliveryMode::default(), DeliveryMode::BackgroundPreferred);
        // Explicit background opts out of both; explicit foreground has
        // nothing to escalate to.
        assert!(
            !DeliveryMode::from_args(&json!({"delivery_mode": "background"})).allows_escalation()
        );
        assert!(
            !DeliveryMode::from_args(&json!({"delivery_mode": "foreground"})).allows_escalation()
        );
    }

    #[test]
    fn a_custom_suggestion_replaces_only_the_foreground_retry_text() {
        let result = background_unavailable_result_with_suggestion(
            "refused",
            "background_unavailable",
            "why",
            json!({}),
            "use get_window_state",
        );
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["suggestion"], "use get_window_state");
        assert_eq!(structured["code"], "background_unavailable");
        assert_eq!(structured["escalation"]["recommended"], "foreground");
    }

    #[test]
    fn background_unavailable_result_is_recorded_as_a_refusal() {
        let result = background_unavailable_result(
            "Background delivery is not available.",
            "background_unavailable",
            "background input is dropped by this surface",
            json!({"event_kind": "keystroke"}),
        );
        let record = crate::action_record::ActionExecutionRecord::from_legacy(
            "press_key",
            &json!({"pid": 1, "window_id": 2, "key": "f5", "delivery_mode": "background"}),
            result.structured_content.as_ref().unwrap(),
        )
        .expect("a background refusal normalizes");
        let public = record.public_result().expect("a refusal projects");
        assert_eq!(public.effect, cua_driver_contract::ActionEffect::Refused);
        assert_eq!(
            public.error.map(|error| error.code),
            Some("background_unavailable".to_owned())
        );
    }

    #[test]
    fn background_unavailable_result_carries_the_foreground_escalation() {
        let result = background_unavailable_result(
            "Background delivery is not available.",
            "background_occluded",
            "the target is occluded",
            json!({
                "event_kind": "mouse_click",
                "code": "forged",
                "suggestion": "forged",
            }),
        );
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content,
            Some(json!({
                "effect": "refused",
                "code": "background_occluded",
                "event_kind": "mouse_click",
                "suggestion": "Retry this action with delivery_mode:\"foreground\".",
                "escalation": {
                    "recommended": "foreground",
                    "reason": "the target is occluded",
                },
            }))
        );
        match &result.content[0] {
            crate::protocol::Content::Text { text, .. } => {
                assert_eq!(text, "Background delivery is not available.")
            }
            _ => panic!("expected text content"),
        }
    }
}
