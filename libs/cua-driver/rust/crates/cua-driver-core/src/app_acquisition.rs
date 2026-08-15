//! Shared, platform-neutral application acquisition result vocabulary.
//!
//! Platform adapters own native discovery and launch mechanics, but publish
//! the same additive `launch_state` object. Keeping its dispositions typed here
//! prevents a backend from silently inventing another spelling or weakening a
//! requested acquisition policy.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use cua_driver_contract::InstancePolicy;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessDisposition {
    Reused,
    Created,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowDisposition {
    Reused,
    Materialized,
    None,
}

/// Canonical additive state nested under a `launch_app` result's
/// `launch_state` key.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppLaunchState {
    /// Legacy compatibility field. Always identical to `request_sent`.
    pub requested: bool,
    pub process_running: bool,
    pub window_ready: bool,
    pub request_sent: bool,
    pub process_disposition: ProcessDisposition,
    pub window_disposition: WindowDisposition,
}

impl AppLaunchState {
    pub const fn new(
        request_sent: bool,
        process_running: bool,
        window_ready: bool,
        process_disposition: ProcessDisposition,
        window_disposition: WindowDisposition,
    ) -> Self {
        Self {
            requested: request_sent,
            process_running,
            window_ready,
            request_sent,
            process_disposition,
            window_disposition,
        }
    }
}

/// Build the canonical JSON value composed into platform `launch_app`
/// responses.
pub fn launch_state_json(
    request_sent: bool,
    process_running: bool,
    window_ready: bool,
    process_disposition: ProcessDisposition,
    window_disposition: WindowDisposition,
) -> Value {
    serde_json::to_value(AppLaunchState::new(
        request_sent,
        process_running,
        window_ready,
        process_disposition,
        window_disposition,
    ))
    .expect("AppLaunchState is JSON-serializable")
}

/// Parse the canonical `launch_app` acquisition policy, including the
/// deprecated boolean compatibility alias.
///
/// `creates_new_application_instance=true` maps to `new`. It may accompany an
/// explicit `new` during migration, but conflicts with any explicit non-new
/// policy. Wrong JSON types are rejected instead of silently selecting the
/// default.
#[allow(clippy::result_large_err)]
pub fn resolve_instance_policy(args: &Value) -> Result<InstancePolicy, ToolResult> {
    let explicit = match args.get("instance_policy") {
        None => None,
        Some(Value::String(value)) => InstancePolicy::parse(value).map(Some).ok_or_else(|| {
            instance_policy_error(
                "INVALID_INSTANCE_POLICY",
                format!(
                    "Unknown instance_policy '{value}'; expected reuse_or_launch, reuse_only, or new."
                ),
                json!({ "instance_policy": value }),
            )
        })?,
        Some(value) => {
            return Err(instance_policy_error(
                "INVALID_INSTANCE_POLICY",
                "instance_policy must be a string: reuse_or_launch, reuse_only, or new."
                    .to_owned(),
                json!({ "instance_policy": value }),
            ));
        }
    };

    let legacy_new = match args.get("creates_new_application_instance") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(value) => {
            return Err(instance_policy_error(
                "INVALID_INSTANCE_POLICY",
                "creates_new_application_instance must be a boolean when provided.".to_owned(),
                json!({ "creates_new_application_instance": value }),
            ));
        }
    };

    if legacy_new && explicit.is_some_and(|policy| policy != InstancePolicy::New) {
        return Err(instance_policy_error(
            "INSTANCE_POLICY_CONFLICT",
            "creates_new_application_instance=true conflicts with the explicit non-new instance_policy. Remove the deprecated alias or use instance_policy=\"new\"."
                .to_owned(),
            json!({
                "instance_policy": explicit.map(InstancePolicy::as_str),
                "creates_new_application_instance": true,
            }),
        ));
    }

    Ok(explicit.unwrap_or(if legacy_new {
        InstancePolicy::New
    } else {
        InstancePolicy::ReuseOrLaunch
    }))
}

fn instance_policy_error(code: &str, message: String, details: Value) -> ToolResult {
    let mut payload = json!({ "error": code });
    if let (Some(payload), Some(details)) = (payload.as_object_mut(), details.as_object()) {
        payload.extend(details.clone());
    }
    ToolResult::error(message).with_structured(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_state_json_keeps_legacy_fields_and_typed_dispositions() {
        assert_eq!(
            launch_state_json(
                false,
                true,
                true,
                ProcessDisposition::Reused,
                WindowDisposition::Reused,
            ),
            serde_json::json!({
                "requested": false,
                "process_running": true,
                "window_ready": true,
                "request_sent": false,
                "process_disposition": "reused",
                "window_disposition": "reused",
            })
        );

        assert_eq!(
            launch_state_json(
                true,
                true,
                false,
                ProcessDisposition::Created,
                WindowDisposition::None,
            ),
            serde_json::json!({
                "requested": true,
                "process_running": true,
                "window_ready": false,
                "request_sent": true,
                "process_disposition": "created",
                "window_disposition": "none",
            })
        );

        assert_eq!(
            serde_json::to_value(WindowDisposition::Materialized).unwrap(),
            "materialized"
        );
        assert_eq!(
            serde_json::to_value(ProcessDisposition::None).unwrap(),
            "none"
        );
    }

    #[test]
    fn instance_policy_parser_defaults_and_maps_the_legacy_alias() {
        assert_eq!(
            resolve_instance_policy(&json!({})).unwrap(),
            InstancePolicy::ReuseOrLaunch
        );
        assert_eq!(
            resolve_instance_policy(&json!({"instance_policy": "reuse_only"})).unwrap(),
            InstancePolicy::ReuseOnly
        );
        assert_eq!(
            resolve_instance_policy(&json!({"creates_new_application_instance": true})).unwrap(),
            InstancePolicy::New
        );
        assert_eq!(
            resolve_instance_policy(&json!({
                "instance_policy": "new",
                "creates_new_application_instance": true
            }))
            .unwrap(),
            InstancePolicy::New
        );
    }

    #[test]
    fn instance_policy_parser_returns_shared_structured_errors() {
        for args in [
            json!({"instance_policy": 7}),
            json!({"instance_policy": "sometimes"}),
            json!({"creates_new_application_instance": "true"}),
        ] {
            let error = resolve_instance_policy(&args).unwrap_err();
            assert_eq!(
                error
                    .structured_content
                    .as_ref()
                    .and_then(|value| value.get("error"))
                    .and_then(Value::as_str),
                Some("INVALID_INSTANCE_POLICY")
            );
        }

        let conflict = resolve_instance_policy(&json!({
            "instance_policy": "reuse_only",
            "creates_new_application_instance": true
        }))
        .unwrap_err();
        assert_eq!(
            conflict.structured_content.unwrap()["error"],
            "INSTANCE_POLICY_CONFLICT"
        );
    }
}
