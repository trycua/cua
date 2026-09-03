// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The one place a `tools/call` result is held to the contract the driver
//! advertises.
//!
//! MCP validates every `structuredContent` a tool emits against that tool's
//! advertised `outputSchema` — payloads sent with `isError: true` included —
//! and rejects a response whose payload matches neither arm (`-32602`), or a
//! successful response that declares a schema and carries no structured
//! payload at all (`-32600`). Either way the client discards the whole
//! response, so the message the driver placed in `content` never reaches the
//! agent.
//!
//! Individual result constructors cannot enforce that: `ToolResult::error`
//! leaves `structuredContent` unset, the transport paths invent diagnostics
//! like `{"exit_code": 1}`, and the proxy synthesises an empty success when the
//! daemon answers `ok` with no result. So the rule lives here instead, on the
//! single path every direct and daemon-backed `tools/call` result passes
//! through.

use serde_json::{json, Value};
use tracing::warn;

use cua_driver_contract::{
    advertises_output_schema, conforming_error_envelope, is_refusal_envelope,
    validate_success_output,
};

/// Marker code for a result the driver replaced because the tool's own payload
/// could not be advertised under its `outputSchema`.
pub const TOOL_OUTPUT_INVALID_CODE: &str = "tool_output_invalid";

/// Hold one `tools/call` result to the tool's advertised `outputSchema`.
///
/// - An error result keeps its diagnostic, normalized into the refusal arm so
///   a strict client accepts it and reads the `content` message.
/// - A successful result for a tool that advertises a schema must carry a
///   structured payload that the schema accepts. A missing or rejected payload
///   becomes a conforming internal-error result, because the alternative is a
///   response the client drops entirely.
/// - A successful result for a tool that advertises no schema is returned
///   unchanged: there is nothing for a client to validate it against.
pub fn conforming_tool_result(tool: &str, result: Value) -> Value {
    let Value::Object(mut result) = result else {
        return internal_error_result(format!(
            "internal result mismatch for {tool}: tool result is not an object"
        ));
    };

    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let structured = result.remove("structuredContent").unwrap_or(Value::Null);
        result.insert(
            "structuredContent".to_owned(),
            conforming_error_envelope(structured),
        );
        return Value::Object(result);
    }

    if !advertises_output_schema(tool) {
        return Value::Object(result);
    }

    match result.get("structuredContent") {
        None | Some(Value::Null) => {
            warn!(
                tool,
                "successful tool result declared an output schema but carried no structured content"
            );
            internal_error_result(format!(
                "internal output mismatch for {tool}: tool advertises an output schema but returned no structured content"
            ))
        }
        // A refusal payload satisfies the advertised schema's other arm. The
        // typed success validator would reject it, so ask the arm it belongs
        // to rather than replacing a payload a client accepts.
        Some(structured) if is_refusal_envelope(structured) => Value::Object(result),
        Some(structured) => match validate_success_output(tool, structured.clone()) {
            Ok(_) => Value::Object(result),
            Err(error) => {
                warn!(
                    tool,
                    "successful tool result carried structured content its output schema rejects"
                );
                internal_error_result(format!("internal output mismatch for {tool}: {error}"))
            }
        },
    }
}

/// Build the `isError: true` result for a call that produced no usable tool
/// payload, already normalized into the refusal arm.
pub fn internal_error_result(message: impl Into<String>) -> Value {
    let message = message.into();
    json!({
        "content": [{"type": "text", "text": message}],
        "isError": true,
        "structuredContent": {"code": TOOL_OUTPUT_INVALID_CODE},
    })
}

/// Build a tool-level error result carrying a caller-supplied diagnostic.
///
/// The diagnostic is passed through untouched here; [`conforming_tool_result`]
/// is what normalizes it, so every error path gets the same treatment whether
/// or not it went through this constructor.
pub fn tool_error_result(message: impl Into<String>, structured: Value) -> Value {
    let message = message.into();
    json!({
        "content": [{"type": "text", "text": message}],
        "isError": true,
        "structuredContent": structured,
    })
}

#[cfg(test)]
mod tests {
    //! The boundary is asserted the way a strict MCP client sees it: every
    //! payload leaving it is validated against the schema the tool actually
    //! advertises.

    use super::*;
    use cua_driver_contract::{advertised_tool_output_schema, TOOL_INVOCATION_FAILED_CODE};

    /// An action tool, so the advertised schema is the shared `ActionResult`
    /// one plus the refusal arm.
    const ACTION_TOOL: &str = "click";
    /// Advertises no `outputSchema` at all.
    const UNSCHEMED_TOOL: &str = "no_such_tool";

    #[track_caller]
    fn assert_conforms(tool: &str, result: &Value) {
        let schema = advertised_tool_output_schema(tool).expect("tool advertises a schema");
        let compiled = jsonschema::validator_for(&schema).expect("schema compiles");
        let structured = &result["structuredContent"];

        assert!(
            compiled.is_valid(structured),
            "advertised schema rejected {structured}"
        );
    }

    /// The shape this boundary exists for: the bare diagnostic the transport
    /// paths invent satisfies neither arm, which is what produced the `-32602`
    /// reports.
    #[test]
    fn a_bare_exit_code_diagnostic_does_not_validate() {
        let schema = advertised_tool_output_schema(ACTION_TOOL).expect("schema");
        let compiled = jsonschema::validator_for(&schema).expect("schema compiles");

        assert!(!compiled.is_valid(&json!({"exit_code": 1})));
    }

    #[test]
    fn an_error_diagnostic_gains_a_refusal_marker() {
        let result = conforming_tool_result(
            ACTION_TOOL,
            tool_error_result("daemon transport closed", json!({"exit_code": 1})),
        );

        assert_conforms(ACTION_TOOL, &result);
        // The diagnostic survives; only the marker is added.
        assert_eq!(result["structuredContent"]["exit_code"], 1);
        assert_eq!(
            result["structuredContent"]["code"],
            TOOL_INVOCATION_FAILED_CODE
        );
        assert_eq!(result["content"][0]["text"], "daemon transport closed");
    }

    /// `ToolResult::error` leaves `structuredContent` unset, and every tool
    /// that refuses through it lands here.
    #[test]
    fn an_error_without_any_payload_gains_one() {
        let result = conforming_tool_result(
            ACTION_TOOL,
            json!({"content": [{"type": "text", "text": "Unknown tool: nope"}], "isError": true}),
        );

        assert_conforms(ACTION_TOOL, &result);
        assert_eq!(
            result["structuredContent"]["code"],
            TOOL_INVOCATION_FAILED_CODE
        );
    }

    /// The guard adds a marker; it does not relabel a payload that already
    /// names its own refusal.
    #[test]
    fn an_existing_refusal_code_is_preserved() {
        let result = conforming_tool_result(
            ACTION_TOOL,
            tool_error_result("denied", json!({"code": "permission_denied"})),
        );

        assert_conforms(ACTION_TOOL, &result);
        assert_eq!(result["structuredContent"]["code"], "permission_denied");
    }

    /// The proxy's empty-success fallback: a client that was promised a schema
    /// rejects this response outright rather than reading it as an error.
    #[test]
    fn a_success_missing_its_structured_payload_becomes_an_error() {
        let result = conforming_tool_result(ACTION_TOOL, json!({"content": [], "isError": false}));

        assert_conforms(ACTION_TOOL, &result);
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["structuredContent"]["code"],
            TOOL_OUTPUT_INVALID_CODE
        );
    }

    #[test]
    fn a_success_whose_payload_the_schema_rejects_becomes_an_error() {
        let result = conforming_tool_result(
            ACTION_TOOL,
            json!({
                "content": [],
                "isError": false,
                "structuredContent": {"clicked": true},
            }),
        );

        assert_conforms(ACTION_TOOL, &result);
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["structuredContent"]["code"],
            TOOL_OUTPUT_INVALID_CODE
        );
    }

    #[test]
    fn a_conforming_success_is_returned_untouched() {
        let structured = json!({
            "effect": "confirmed",
            "route": "accessibility",
            "delivery": {"mode": "background"},
            "evidence": [{"kind": "value_readback"}],
        });
        let success = json!({
            "content": [{"type": "text", "text": "clicked"}],
            "isError": false,
            "structuredContent": structured,
        });

        let result = conforming_tool_result(ACTION_TOOL, success.clone());

        assert_conforms(ACTION_TOOL, &result);
        assert_eq!(result, success);
    }

    /// A tool with no advertised schema has no contract to violate, so its
    /// successful payload must not be second-guessed here.
    #[test]
    fn a_success_from_a_tool_without_a_schema_is_untouched() {
        let success = json!({
            "content": [{"type": "text", "text": "ok"}],
            "isError": false,
            "structuredContent": {"anything": "goes"},
        });

        assert_eq!(
            conforming_tool_result(UNSCHEMED_TOOL, success.clone()),
            success
        );
    }

    #[test]
    fn a_result_that_is_not_an_object_becomes_a_conforming_error() {
        let result = conforming_tool_result(ACTION_TOOL, json!("not a result"));

        assert_conforms(ACTION_TOOL, &result);
        assert_eq!(result["isError"], true);
    }
}
