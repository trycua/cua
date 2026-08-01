// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Deterministic, bounded state verification.
//!
//! This module deliberately consumes the existing platform observation tools
//! directly. It never re-enters `ToolRegistry::invoke`, so one outer
//! `verify_state` admission owns authorization for the whole polling window.

use async_trait::async_trait;
use cua_driver_contract::{
    ElementPredicate, PredicateOutcome, StatePredicate, ToolInput, UnknownReason,
    VerificationStatus, VerifyStateInput, VerifyStateOutput, WindowPredicate,
    VERIFY_STATE_DEFAULT_TIMEOUT_MS,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::{
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};

const MAX_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_STABLE_SAMPLES: u64 = 2;
const MAX_STABLE_SAMPLES: u64 = 5;
const MAX_PREDICATES: usize = 8;
const POLL_INTERVAL_MS: u64 = 100;

#[derive(Debug, Clone, Default)]
pub struct ObservationSnapshot {
    pub window: Option<Value>,
    pub elements: Option<Vec<Value>>,
    pub element_source_trusted: bool,
    /// True only when the platform proved that its structured element walk
    /// completed without truncation. Negative element existence needs this.
    pub elements_complete: bool,
}

#[derive(Debug, Default)]
pub struct ObservationSample {
    pub snapshot: ObservationSnapshot,
    pub visual_evidence: Vec<Content>,
}

#[async_trait]
pub trait ObservationProvider: Send + Sync {
    async fn observe(
        &self,
        pid: i64,
        window_id: u64,
        include_elements: bool,
        include_screenshot: bool,
    ) -> Result<ObservationSample, String>;
}

/// Adapter over the already-registered platform observation implementations.
///
/// Platform registries construct this with fresh tool instances sharing the
/// same platform state as their public `get_window_state` tool.
pub struct ToolObservationProvider {
    list_windows: Arc<dyn Tool>,
    get_window_state: Arc<dyn Tool>,
}

impl ToolObservationProvider {
    pub fn new(list_windows: Arc<dyn Tool>, get_window_state: Arc<dyn Tool>) -> Self {
        Self {
            list_windows,
            get_window_state,
        }
    }
}

#[async_trait]
impl ObservationProvider for ToolObservationProvider {
    async fn observe(
        &self,
        pid: i64,
        window_id: u64,
        include_elements: bool,
        include_screenshot: bool,
    ) -> Result<ObservationSample, String> {
        let listed = self
            .list_windows
            .invoke(json!({"pid": pid, "on_screen_only": false}))
            .await;
        if listed.is_error == Some(true) {
            return Err(tool_error_text(&listed, "list_windows failed"));
        }
        let window = listed
            .structured_content
            .as_ref()
            .and_then(|value| value.get("windows"))
            .and_then(Value::as_array)
            .and_then(|windows| {
                windows
                    .iter()
                    .find(|window| {
                        window.get("window_id").and_then(Value::as_u64) == Some(window_id)
                            && window.get("pid").and_then(Value::as_i64) == Some(pid)
                    })
                    .cloned()
            });
        if window.is_none()
            && listed
                .structured_content
                .as_ref()
                .and_then(|value| value.get("windows"))
                .and_then(Value::as_array)
                .is_some_and(|windows| {
                    windows.iter().any(|candidate| {
                        candidate.get("window_id").and_then(Value::as_u64) == Some(window_id)
                            && candidate.get("pid").is_none_or(Value::is_null)
                    })
                })
        {
            return Err(format!(
                "window {window_id} was listed without a trustworthy owning pid"
            ));
        }

        if window.is_none() || (!include_elements && !include_screenshot) {
            return Ok(ObservationSample {
                snapshot: ObservationSnapshot {
                    window,
                    elements: None,
                    element_source_trusted: false,
                    elements_complete: false,
                },
                visual_evidence: Vec::new(),
            });
        }

        let state = self
            .get_window_state
            .invoke(json!({
                "pid": pid,
                "window_id": window_id,
                "include_screenshot": include_screenshot,
                // Internal direct-tool flag: verification must not refresh the
                // shared action index/token cache.
                // Registry ingress removes underscore-prefixed arguments before
                // public dispatch. Direct platform-tool invocation is the
                // trusted in-process channel for this non-mutating mode.
                "_observation_only": true,
            }))
            .await;
        if state.is_error == Some(true) {
            return Err(tool_error_text(&state, "get_window_state failed"));
        }
        let structured = state.structured_content.as_ref();
        let elements = structured
            .and_then(|value| value.get("elements"))
            .and_then(Value::as_array)
            .cloned();
        let trusted = structured
            .and_then(|value| value.get("degraded"))
            .and_then(Value::as_bool)
            != Some(true);
        let complete = structured
            .and_then(|value| value.get("elements_complete"))
            .and_then(Value::as_bool)
            == Some(true);
        let visual_evidence = state
            .content
            .into_iter()
            .filter(|content| matches!(content, Content::Image { .. }))
            .collect();

        Ok(ObservationSample {
            snapshot: ObservationSnapshot {
                window,
                elements,
                element_source_trusted: trusted,
                elements_complete: complete,
            },
            visual_evidence,
        })
    }
}

fn tool_error_text(result: &ToolResult, fallback: &str) -> String {
    result
        .content
        .iter()
        .find_map(|content| match content {
            Content::Text { text, .. } => Some(text.clone()),
            Content::Image { .. } => None,
        })
        .unwrap_or_else(|| fallback.to_owned())
}

pub struct VerifyStateTool {
    provider: Arc<dyn ObservationProvider>,
}

impl VerifyStateTool {
    pub fn new(provider: Arc<dyn ObservationProvider>) -> Self {
        Self { provider }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| {
        let contract = cua_driver_contract::tool_contract(VerifyStateInput::TOOL_NAME)
            .expect("verify_state contract must be published");
        ToolDef {
            name: contract.name,
            description: contract.description,
            input_schema: contract.input_schema,
            read_only: contract.annotations.read_only,
            destructive: contract.annotations.destructive,
            idempotent: contract.annotations.idempotent,
            open_world: contract.annotations.open_world,
        }
    })
}

#[async_trait]
impl Tool for VerifyStateTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input: VerifyStateInput = match parse_typed_input("verify_state", args) {
            Ok(input) => input,
            Err(error) => return error,
        };
        if input.pid <= 0 {
            return ToolResult::error("verify_state requires pid > 0");
        }
        if input.expect.is_empty() || input.expect.len() > MAX_PREDICATES {
            return ToolResult::error("verify_state requires between 1 and 8 predicates");
        }
        if input.timeout_ms.is_some_and(|value| value > MAX_TIMEOUT_MS) {
            return ToolResult::error("verify_state timeout_ms must be between 0 and 10000");
        }
        if input
            .stable_samples
            .is_some_and(|value| !(1..=MAX_STABLE_SAMPLES).contains(&value))
        {
            return ToolResult::error("verify_state stable_samples must be between 1 and 5");
        }
        let timeout_ms = input.timeout_ms.unwrap_or(VERIFY_STATE_DEFAULT_TIMEOUT_MS);
        if timeout_ms == 0 && input.stable_samples.is_some_and(|samples| samples > 1) {
            return ToolResult::error(
                "verify_state timeout_ms=0 requires stable_samples=1 because only one sample is possible",
            );
        }
        let stable_samples = if timeout_ms == 0 {
            1
        } else {
            input.stable_samples.unwrap_or(DEFAULT_STABLE_SAMPLES)
        };
        if let Some(error) = invalid_predicate_message(&input.expect) {
            return ToolResult::error(error);
        }
        let include_elements = input
            .expect
            .iter()
            .any(|predicate| predicate.element.is_some());
        let started = tokio::time::Instant::now();
        let deadline = started + Duration::from_millis(timeout_ms);
        let mut samples = 0u64;
        let mut consecutive_satisfied = 0u64;
        let mut stable = false;

        let mut last_outcomes = loop {
            samples += 1;
            let outcomes = match self
                .provider
                .observe(input.pid, input.window_id, include_elements, false)
                .await
            {
                Ok(sample) => evaluate_predicates(&input.expect, &sample.snapshot),
                Err(error) => unavailable_outcomes(&input.expect, &error),
            };
            let status = aggregate_status(&outcomes);
            if status == VerificationStatus::Satisfied {
                consecutive_satisfied += 1;
                if consecutive_satisfied >= stable_samples {
                    stable = true;
                    break outcomes;
                }
            } else {
                consecutive_satisfied = 0;
            }
            if timeout_ms == 0 || tokio::time::Instant::now() >= deadline {
                break outcomes;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            tokio::time::sleep(remaining.min(Duration::from_millis(POLL_INTERVAL_MS))).await;
        };

        let last_status = aggregate_status(&last_outcomes);
        if last_status == VerificationStatus::Satisfied && !stable {
            for outcome in &mut last_outcomes {
                if outcome.status == VerificationStatus::Satisfied {
                    outcome.status = VerificationStatus::Unknown;
                    outcome.unknown_reason = Some(UnknownReason::StabilityUnproven);
                }
            }
        }
        let status = aggregate_status(&last_outcomes);
        let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        let output = VerifyStateOutput {
            status,
            stable,
            elapsed_ms,
            samples,
            predicates: last_outcomes,
        };
        let mut result = ToolResult::text(format!(
            "verify_state: {} after {} sample(s) in {} ms",
            status_label(status),
            samples,
            elapsed_ms
        ))
        .with_structured(serde_json::to_value(&output).expect("VerifyStateOutput must serialize"));

        if input.include_screenshot == Some(true) {
            if let Ok(evidence) = self
                .provider
                .observe(input.pid, input.window_id, false, true)
                .await
            {
                result.content.extend(evidence.visual_evidence);
            }
        }
        result
    }
}

fn invalid_predicate_message(expect: &[StatePredicate]) -> Option<String> {
    for (index, predicate) in expect.iter().enumerate() {
        if let Some(element) = predicate.element.as_ref() {
            if element.exists == Some(false) {
                return Some(format!(
                    "verify_state predicate {index} element.exists=false is unsupported because element snapshots are not exhaustive"
                ));
            }
            for (name, value) in [
                ("role", element.selector.role.as_deref()),
                ("label_contains", element.selector.label_contains.as_deref()),
            ] {
                if value.is_some_and(|value| value.trim().is_empty()) {
                    return Some(format!(
                        "verify_state predicate {index} selector {name} must not be empty"
                    ));
                }
            }
        }
    }
    None
}

fn status_label(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Satisfied => "satisfied",
        VerificationStatus::Unsatisfied => "unsatisfied",
        VerificationStatus::Unknown => "unknown",
    }
}

fn unavailable_outcomes(expect: &[StatePredicate], error: &str) -> Vec<PredicateOutcome> {
    expect
        .iter()
        .enumerate()
        .map(|(index, _)| PredicateOutcome {
            index: index as u64,
            status: VerificationStatus::Unknown,
            unknown_reason: Some(UnknownReason::ObservationUnavailable),
            observed_json: Some(bounded_json(&json!({"error": error}))),
        })
        .collect()
}

pub fn evaluate_predicates(
    expect: &[StatePredicate],
    snapshot: &ObservationSnapshot,
) -> Vec<PredicateOutcome> {
    expect
        .iter()
        .enumerate()
        .map(|(index, predicate)| {
            let (status, reason, observed) = match (&predicate.window, &predicate.element) {
                (Some(window), None) => evaluate_window(window, snapshot.window.as_ref()),
                (None, Some(element)) => evaluate_element(element, snapshot),
                _ => (
                    VerificationStatus::Unknown,
                    Some(UnknownReason::InvalidPredicate),
                    None,
                ),
            };
            PredicateOutcome {
                index: index as u64,
                status,
                unknown_reason: reason,
                observed_json: observed.map(|value| bounded_json(&value)),
            }
        })
        .collect()
}

fn evaluate_window(
    predicate: &WindowPredicate,
    window: Option<&Value>,
) -> (VerificationStatus, Option<UnknownReason>, Option<Value>) {
    if predicate.exists.is_none() && predicate.bounds.is_none() {
        return (
            VerificationStatus::Unknown,
            Some(UnknownReason::InvalidPredicate),
            None,
        );
    }
    let exists = window.is_some();
    if let Some(expected_exists) = predicate.exists {
        if exists != expected_exists {
            return (
                VerificationStatus::Unsatisfied,
                None,
                Some(json!({"exists": exists})),
            );
        }
    }
    let Some(window) = window else {
        return if predicate.exists == Some(false) && predicate.bounds.is_none() {
            (
                VerificationStatus::Satisfied,
                None,
                Some(json!({"exists": false})),
            )
        } else {
            (
                VerificationStatus::Unknown,
                Some(UnknownReason::TargetMissing),
                Some(json!({"exists": false})),
            )
        };
    };
    if let Some(expected) = &predicate.bounds {
        let Some(bounds) = window.get("bounds") else {
            return (
                VerificationStatus::Unknown,
                Some(UnknownReason::UnsupportedPredicate),
                Some(json!({"exists": true})),
            );
        };
        let tolerance = expected.tolerance_px.unwrap_or(1.0);
        let observed = [
            bounds.get("x").and_then(Value::as_f64),
            bounds.get("y").and_then(Value::as_f64),
            bounds.get("width").and_then(Value::as_f64),
            bounds.get("height").and_then(Value::as_f64),
        ];
        if observed.iter().any(Option::is_none) {
            return (
                VerificationStatus::Unknown,
                Some(UnknownReason::UnsupportedPredicate),
                Some(json!({"exists": true, "bounds": bounds})),
            );
        }
        let observed = [
            observed[0].unwrap(),
            observed[1].unwrap(),
            observed[2].unwrap(),
            observed[3].unwrap(),
        ];
        let expected_values = [expected.x, expected.y, expected.width, expected.height];
        if observed
            .iter()
            .zip(expected_values)
            .any(|(actual, expected)| (actual - expected).abs() > tolerance)
        {
            return (
                VerificationStatus::Unsatisfied,
                None,
                Some(json!({"exists": true, "bounds": bounds})),
            );
        }
    }
    (
        VerificationStatus::Satisfied,
        None,
        Some(json!({"exists": true, "bounds": window.get("bounds")})),
    )
}

fn evaluate_element(
    predicate: &ElementPredicate,
    snapshot: &ObservationSnapshot,
) -> (VerificationStatus, Option<UnknownReason>, Option<Value>) {
    if predicate.exists == Some(false) {
        return (
            VerificationStatus::Unknown,
            Some(UnknownReason::InvalidPredicate),
            None,
        );
    }
    if (predicate.selector.role.is_none() && predicate.selector.label_contains.is_none())
        || predicate
            .selector
            .role
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || predicate
            .selector
            .label_contains
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return (
            VerificationStatus::Unknown,
            Some(UnknownReason::InvalidPredicate),
            None,
        );
    }
    if snapshot.window.is_none() {
        return (
            VerificationStatus::Unknown,
            Some(UnknownReason::TargetMissing),
            Some(json!({"window_exists": false})),
        );
    }
    if !snapshot.element_source_trusted {
        return (
            VerificationStatus::Unknown,
            Some(UnknownReason::UntrustedSource),
            None,
        );
    }
    let Some(elements) = snapshot.elements.as_ref() else {
        return (
            VerificationStatus::Unknown,
            Some(UnknownReason::ObservationUnavailable),
            None,
        );
    };
    let matches: Vec<&Value> = elements
        .iter()
        .filter(|element| selector_matches(&predicate.selector, element))
        .collect();
    if matches.is_empty() {
        let contains_untrusted_region = elements.iter().any(|element| {
            element.get("in_web_content").and_then(Value::as_bool) == Some(true)
                || target_is_in_web_area(element, elements)
        });
        if !snapshot.elements_complete || contains_untrusted_region {
            return (
                VerificationStatus::Unknown,
                Some(if contains_untrusted_region {
                    UnknownReason::UntrustedSource
                } else {
                    UnknownReason::ObservationUnavailable
                }),
                Some(json!({
                    "matches": 0,
                    "elements_complete": snapshot.elements_complete,
                    "contains_untrusted_region": contains_untrusted_region
                })),
            );
        }
        return match predicate.exists {
            Some(true) => (
                VerificationStatus::Unsatisfied,
                None,
                Some(json!({"matches": 0})),
            ),
            _ => (
                VerificationStatus::Unknown,
                Some(UnknownReason::TargetMissing),
                Some(json!({"matches": 0})),
            ),
        };
    }
    let trusted_matches: Vec<&Value> = matches
        .iter()
        .copied()
        .filter(|element| !target_is_in_web_area(element, elements))
        .collect();
    if trusted_matches.is_empty() {
        return (
            VerificationStatus::Unknown,
            Some(UnknownReason::UntrustedSource),
            Some(json!({"matches": matches.len()})),
        );
    }
    if trusted_matches.len() > 1
        && (predicate.value_equals.is_some()
            || predicate.enabled.is_some()
            || predicate.selected.is_some())
    {
        return (
            VerificationStatus::Unknown,
            Some(UnknownReason::MultiMatch),
            Some(json!({"matches": trusted_matches.len()})),
        );
    }
    if trusted_matches.len() > 1 {
        return (
            VerificationStatus::Satisfied,
            None,
            Some(json!({"matches": trusted_matches.len()})),
        );
    }
    let element = trusted_matches[0];
    for (field, expected) in [
        (
            "value",
            predicate
                .value_equals
                .as_ref()
                .map(|value| Value::String(value.clone())),
        ),
        ("enabled", predicate.enabled.map(Value::Bool)),
        ("selected", predicate.selected.map(Value::Bool)),
    ] {
        let Some(expected) = expected else {
            continue;
        };
        let Some(actual) = element.get(field) else {
            return (
                VerificationStatus::Unknown,
                Some(UnknownReason::UnsupportedPredicate),
                Some(project_element(element)),
            );
        };
        if actual != &expected {
            return (
                VerificationStatus::Unsatisfied,
                None,
                Some(project_element(element)),
            );
        }
    }
    (
        VerificationStatus::Satisfied,
        None,
        Some(project_element(element)),
    )
}

fn selector_matches(selector: &cua_driver_contract::ElementSelector, element: &Value) -> bool {
    if let Some(role) = selector.role.as_deref() {
        if !element
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|actual| normalized_role(actual) == normalized_role(role))
        {
            return false;
        }
    }
    if let Some(label) = selector.label_contains.as_deref() {
        let needle = label.to_lowercase();
        if !element
            .get("label")
            .and_then(Value::as_str)
            .is_some_and(|actual| actual.to_lowercase().contains(&needle))
        {
            return false;
        }
    }
    true
}

fn normalized_role(role: &str) -> String {
    let normalized: String = role
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let normalized = normalized
        .strip_prefix("ax")
        .unwrap_or(&normalized)
        .to_owned();
    match normalized.as_str() {
        "pushbutton" => "button".to_owned(),
        "pagetab" | "tabitem" => "tab".to_owned(),
        _ => normalized,
    }
}

fn target_is_in_web_area(target: &Value, elements: &[Value]) -> bool {
    let by_index: HashMap<u64, &Value> = elements
        .iter()
        .filter_map(|element| {
            element
                .get("element_index")
                .and_then(Value::as_u64)
                .map(|index| (index, element))
        })
        .collect();
    let mut current = Some(target);
    for _ in 0..32 {
        let Some(element) = current else {
            break;
        };
        if element.get("in_web_content").and_then(Value::as_bool) == Some(true) {
            return true;
        }
        if element
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|role| {
                let normalized = role.to_ascii_lowercase().replace(['_', ' '], "");
                normalized.contains("webarea")
                    || normalized.contains("document")
                    || normalized == "embedded"
            })
        {
            return true;
        }
        current = element
            .get("parent_index")
            .and_then(Value::as_u64)
            .and_then(|index| by_index.get(&index).copied());
    }
    false
}

fn project_element(element: &Value) -> Value {
    let mut projection = serde_json::Map::new();
    for key in [
        "element_index",
        "role",
        "label",
        "value",
        "enabled",
        "selected",
        "frame",
    ] {
        if let Some(value) = element.get(key) {
            projection.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(projection)
}

fn bounded_json(value: &Value) -> String {
    let mut encoded = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned());
    if encoded.len() > 2_000 {
        let mut boundary = 1_997;
        while !encoded.is_char_boundary(boundary) {
            boundary -= 1;
        }
        encoded.truncate(boundary);
        encoded.push_str("...");
    }
    encoded
}

fn aggregate_status(outcomes: &[PredicateOutcome]) -> VerificationStatus {
    if outcomes
        .iter()
        .any(|outcome| outcome.status == VerificationStatus::Unsatisfied)
    {
        VerificationStatus::Unsatisfied
    } else if outcomes
        .iter()
        .any(|outcome| outcome.status == VerificationStatus::Unknown)
    {
        VerificationStatus::Unknown
    } else {
        VerificationStatus::Satisfied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_contract::{
        BoundsExpectation, ElementSelector, VerificationStatus, WindowPredicate,
    };

    fn window() -> Value {
        json!({
            "window_id": 7,
            "pid": 42,
            "bounds": {"x": 10.0, "y": 20.0, "width": 300.0, "height": 400.0}
        })
    }

    fn element_predicate(label: &str) -> StatePredicate {
        StatePredicate {
            window: None,
            element: Some(ElementPredicate {
                selector: ElementSelector {
                    role: Some("checkbox".into()),
                    label_contains: Some(label.into()),
                },
                exists: Some(true),
                value_equals: Some("1".into()),
                enabled: None,
                selected: None,
            }),
        }
    }

    #[test]
    fn evaluates_window_bounds_with_tolerance() {
        let predicates = vec![StatePredicate {
            window: Some(WindowPredicate {
                exists: Some(true),
                bounds: Some(BoundsExpectation {
                    x: 10.5,
                    y: 20.0,
                    width: 300.0,
                    height: 400.0,
                    tolerance_px: Some(1.0),
                }),
            }),
            element: None,
        }];
        let outcomes = evaluate_predicates(
            &predicates,
            &ObservationSnapshot {
                window: Some(window()),
                elements: None,
                element_source_trusted: false,
                elements_complete: false,
            },
        );
        assert_eq!(outcomes[0].status, VerificationStatus::Satisfied);
    }

    #[test]
    fn unique_element_value_is_satisfied() {
        let outcomes = evaluate_predicates(
            &[element_predicate("updates")],
            &ObservationSnapshot {
                window: Some(window()),
                elements: Some(vec![json!({
                    "element_index": 3,
                    "role": "checkbox",
                    "label": "Automatically check for updates",
                    "value": "1"
                })]),
                element_source_trusted: true,
                elements_complete: true,
            },
        );
        assert_eq!(outcomes[0].status, VerificationStatus::Satisfied);
    }

    #[test]
    fn duplicate_property_target_is_unknown_not_first_match_wins() {
        let element = json!({
            "element_index": 3,
            "role": "checkbox",
            "label": "Updates",
            "value": "1"
        });
        let outcomes = evaluate_predicates(
            &[element_predicate("updates")],
            &ObservationSnapshot {
                window: Some(window()),
                elements: Some(vec![element.clone(), element]),
                element_source_trusted: true,
                elements_complete: true,
            },
        );
        assert_eq!(outcomes[0].status, VerificationStatus::Unknown);
        assert_eq!(outcomes[0].unknown_reason, Some(UnknownReason::MultiMatch));
    }

    #[test]
    fn web_area_readback_is_unknown() {
        let outcomes = evaluate_predicates(
            &[element_predicate("updates")],
            &ObservationSnapshot {
                window: Some(window()),
                elements: Some(vec![
                    json!({
                        "element_index": 1,
                        "role": "AXWebArea",
                        "label": "Page"
                    }),
                    json!({
                        "element_index": 3,
                        "parent_index": 1,
                        "role": "checkbox",
                        "label": "Updates",
                        "value": "1"
                    }),
                ]),
                element_source_trusted: true,
                elements_complete: true,
            },
        );
        assert_eq!(outcomes[0].status, VerificationStatus::Unknown);
        assert_eq!(
            outcomes[0].unknown_reason,
            Some(UnknownReason::UntrustedSource)
        );
    }

    #[test]
    fn web_area_existence_is_unknown_even_with_multiple_matches() {
        let mut predicate = element_predicate("updates");
        predicate.element.as_mut().unwrap().value_equals = None;
        let outcomes = evaluate_predicates(
            &[predicate],
            &ObservationSnapshot {
                window: Some(window()),
                elements: Some(vec![
                    json!({"element_index": 1, "role": "WebArea", "label": "Page"}),
                    json!({
                        "element_index": 2,
                        "parent_index": 1,
                        "role": "checkbox",
                        "label": "Updates"
                    }),
                    json!({
                        "element_index": 3,
                        "parent_index": 1,
                        "role": "checkbox",
                        "label": "Updates"
                    }),
                ]),
                element_source_trusted: true,
                elements_complete: true,
            },
        );
        assert_eq!(outcomes[0].status, VerificationStatus::Unknown);
        assert_eq!(
            outcomes[0].unknown_reason,
            Some(UnknownReason::UntrustedSource)
        );
    }

    #[test]
    fn normalizes_native_roles_across_platforms() {
        for native_role in ["AXCheckBox", "CheckBox", "check box"] {
            let outcomes = evaluate_predicates(
                &[element_predicate("updates")],
                &ObservationSnapshot {
                    window: Some(window()),
                    elements: Some(vec![json!({
                        "element_index": 3,
                        "role": native_role,
                        "label": "Updates",
                        "value": "1"
                    })]),
                    element_source_trusted: true,
                    elements_complete: true,
                },
            );
            assert_eq!(
                outcomes[0].status,
                VerificationStatus::Satisfied,
                "native role {native_role} should normalize to checkbox"
            );
        }
        assert_eq!(normalized_role("AXButton"), normalized_role("push button"));
        assert_eq!(normalized_role("TabItem"), normalized_role("page tab"));
    }

    #[test]
    fn explicit_windows_and_linux_web_markers_are_untrusted() {
        for native_role in ["Document", "document web"] {
            let outcomes = evaluate_predicates(
                &[element_predicate("updates")],
                &ObservationSnapshot {
                    window: Some(window()),
                    elements: Some(vec![json!({
                        "element_index": 3,
                        "role": native_role,
                        "label": "Updates",
                        "value": "1",
                        "in_web_content": true
                    })]),
                    element_source_trusted: true,
                    elements_complete: true,
                },
            );
            assert_eq!(outcomes[0].status, VerificationStatus::Unknown);
            assert_eq!(
                outcomes[0].unknown_reason,
                Some(UnknownReason::UntrustedSource)
            );
        }
    }

    #[test]
    fn negative_element_existence_is_invalid_even_for_incomplete_walks() {
        let predicate = StatePredicate {
            window: None,
            element: Some(ElementPredicate {
                selector: ElementSelector {
                    role: Some("button".into()),
                    label_contains: Some("Missing".into()),
                },
                exists: Some(false),
                value_equals: None,
                enabled: None,
                selected: None,
            }),
        };
        let outcomes = evaluate_predicates(
            &[predicate],
            &ObservationSnapshot {
                window: Some(window()),
                elements: Some(vec![]),
                element_source_trusted: true,
                elements_complete: false,
            },
        );
        assert_eq!(outcomes[0].status, VerificationStatus::Unknown);
        assert_eq!(
            outcomes[0].unknown_reason,
            Some(UnknownReason::InvalidPredicate)
        );
    }

    #[test]
    fn empty_selector_value_is_invalid() {
        let mut predicate = element_predicate("");
        predicate.element.as_mut().unwrap().selector.role = Some(" ".into());
        let outcomes = evaluate_predicates(
            &[predicate],
            &ObservationSnapshot {
                window: Some(window()),
                elements: Some(vec![]),
                element_source_trusted: true,
                elements_complete: true,
            },
        );
        assert_eq!(outcomes[0].status, VerificationStatus::Unknown);
        assert_eq!(
            outcomes[0].unknown_reason,
            Some(UnknownReason::InvalidPredicate)
        );
    }

    #[test]
    fn missing_optional_property_is_unknown_not_false() {
        let mut predicate = element_predicate("updates");
        let element = predicate.element.as_mut().unwrap();
        element.value_equals = None;
        element.enabled = Some(true);
        let outcomes = evaluate_predicates(
            &[predicate],
            &ObservationSnapshot {
                window: Some(window()),
                elements: Some(vec![json!({
                    "element_index": 3,
                    "role": "checkbox",
                    "label": "Updates"
                })]),
                element_source_trusted: true,
                elements_complete: true,
            },
        );
        assert_eq!(outcomes[0].status, VerificationStatus::Unknown);
        assert_eq!(
            outcomes[0].unknown_reason,
            Some(UnknownReason::UnsupportedPredicate)
        );
    }

    struct FakeProvider {
        calls: std::sync::atomic::AtomicUsize,
        snapshot: ObservationSnapshot,
    }

    #[async_trait]
    impl ObservationProvider for FakeProvider {
        async fn observe(
            &self,
            _pid: i64,
            _window_id: u64,
            _include_elements: bool,
            include_screenshot: bool,
        ) -> Result<ObservationSample, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ObservationSample {
                snapshot: self.snapshot.clone(),
                visual_evidence: include_screenshot
                    .then(|| Content::image_png("cG5n".into()))
                    .into_iter()
                    .collect(),
            })
        }
    }

    #[tokio::test]
    async fn verify_tool_waits_for_stability_and_returns_visual_evidence() {
        let provider = Arc::new(FakeProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
            snapshot: ObservationSnapshot {
                window: Some(window()),
                elements: None,
                element_source_trusted: false,
                elements_complete: false,
            },
        });
        let tool = VerifyStateTool::new(provider.clone());
        let result = tool
            .invoke(json!({
                "pid": 42,
                "window_id": 7,
                "expect": [{
                    "window": {
                        "exists": true,
                        "bounds": {
                            "x": 10,
                            "y": 20,
                            "width": 300,
                            "height": 400,
                            "tolerance_px": 0
                        }
                    }
                }],
                "timeout_ms": 500,
                "stable_samples": 2,
                "include_screenshot": true
            }))
            .await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.as_ref().unwrap()["status"],
            json!("satisfied")
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["samples"],
            json!(2)
        );
        assert_eq!(
            provider.calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "two verification samples plus one final visual-evidence capture"
        );
        assert!(result
            .content
            .iter()
            .any(|content| matches!(content, Content::Image { .. })));
    }

    #[tokio::test]
    async fn verify_tool_ignores_trusted_transport_metadata() {
        let provider = Arc::new(FakeProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
            snapshot: ObservationSnapshot {
                window: Some(window()),
                elements: None,
                element_source_trusted: false,
                elements_complete: false,
            },
        });
        let result = VerifyStateTool::new(provider)
            .invoke(json!({
                "pid": 42,
                "window_id": 7,
                "expect": [{"window": {"exists": true}}],
                "timeout_ms": 0,
                "stable_samples": 1,
                "_session_id": "injected-by-daemon",
                "_transport_session_id": "injected-by-daemon"
            }))
            .await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.as_ref().unwrap()["status"],
            json!("satisfied")
        );
    }

    #[tokio::test]
    async fn verify_tool_rejects_out_of_contract_polling_bounds() {
        let provider = Arc::new(FakeProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
            snapshot: ObservationSnapshot::default(),
        });
        let tool = VerifyStateTool::new(provider.clone());
        for invalid in [
            json!({
                "pid": 42, "window_id": 7,
                "expect": [{"window": {"exists": true}}],
                "timeout_ms": 10_001
            }),
            json!({
                "pid": 42, "window_id": 7,
                "expect": [{"window": {"exists": true}}],
                "stable_samples": 0
            }),
            json!({
                "pid": 42, "window_id": 7,
                "expect": [{"window": {"exists": true}}],
                "timeout_ms": 0,
                "stable_samples": 2
            }),
            json!({
                "pid": 42, "window_id": 7,
                "expect": [{"element": {
                    "selector": {"label_contains": ""}
                }}]
            }),
            json!({
                "pid": 42, "window_id": 7,
                "expect": [{"element": {
                    "selector": {"role": "button"},
                    "exists": false
                }}]
            }),
        ] {
            assert_eq!(tool.invoke(invalid).await.is_error, Some(true));
        }
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn satisfied_last_sample_is_not_success_without_required_stability() {
        let provider = Arc::new(FakeProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
            snapshot: ObservationSnapshot {
                window: Some(window()),
                elements: None,
                element_source_trusted: false,
                elements_complete: false,
            },
        });
        let result = VerifyStateTool::new(provider)
            .invoke(json!({
                "pid": 42,
                "window_id": 7,
                "expect": [{"window": {"exists": true}}],
                "timeout_ms": 1,
                "stable_samples": 5
            }))
            .await;
        assert_eq!(
            result.structured_content.as_ref().unwrap()["status"],
            json!("unknown")
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["stable"],
            json!(false)
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["predicates"][0]["status"],
            json!("unknown")
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["predicates"][0]["unknown_reason"],
            json!("stability_unproven")
        );
    }

    struct TransientProvider {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl ObservationProvider for TransientProvider {
        async fn observe(
            &self,
            _pid: i64,
            _window_id: u64,
            _include_elements: bool,
            _include_screenshot: bool,
        ) -> Result<ObservationSample, String> {
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if call == 0 {
                Err("transient observation failure".into())
            } else {
                Ok(ObservationSample {
                    snapshot: ObservationSnapshot {
                        window: Some(window()),
                        elements: None,
                        element_source_trusted: false,
                        elements_complete: false,
                    },
                    visual_evidence: vec![],
                })
            }
        }
    }

    #[tokio::test]
    async fn transient_observation_failure_is_retried_until_stable() {
        let provider = Arc::new(TransientProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let result = VerifyStateTool::new(provider.clone())
            .invoke(json!({
                "pid": 42,
                "window_id": 7,
                "expect": [{"window": {"exists": true}}],
                "timeout_ms": 500,
                "stable_samples": 2
            }))
            .await;
        assert_eq!(
            result.structured_content.as_ref().unwrap()["status"],
            json!("satisfied")
        );
        assert_eq!(
            provider.calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "one transient error plus two stable samples"
        );
    }

    #[test]
    fn observed_json_utf8_truncation_is_bounded_and_valid() {
        let encoded = bounded_json(&json!({"label": "é".repeat(2_000)}));
        assert!(encoded.len() <= 2_000);
        assert!(encoded.ends_with("..."));
    }
}
