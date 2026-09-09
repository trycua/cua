// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Experimental Android requests, independent of the desktop platform contract.
//! Session ownership, snapshot freshness, and live coordinate bounds remain runtime checks.

use schemars::{JsonSchema, Schema};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

pub const CONTRACT_VERSION: &str = "cua.android.v0";
const PACKAGE_PATTERN: &str = "^[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z0-9_]+)+$";

#[derive(Debug, Deserialize, JsonSchema)]
enum ContractVersion {
    #[serde(rename = "cua.android.v0")]
    V0,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Request<P> {
    contract_version: ContractVersion,
    #[schemars(length(min = 1, max = 128))]
    request_id: String,
    operation: String,
    #[serde(default)]
    #[schemars(schema_with = "id_schema")]
    session_id: Option<String>,
    params: P,
}

fn id_schema(_: &mut schemars::SchemaGenerator) -> Schema {
    json!({"type":"string", "minLength":1, "maxLength":128})
        .try_into()
        .unwrap()
}

fn label_schema(_: &mut schemars::SchemaGenerator) -> Schema {
    json!({"type":"string", "maxLength":128})
        .try_into()
        .unwrap()
}

fn packages_schema(_: &mut schemars::SchemaGenerator) -> Schema {
    json!({"type":"array", "minItems":1, "maxItems":8,
        "items":{"type":"string", "pattern":PACKAGE_PATTERN}})
    .try_into()
    .unwrap()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyParams {}

fn default_width() -> u32 {
    1080
}
fn default_height() -> u32 {
    1920
}
fn default_density() -> u32 {
    320
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionParams {
    #[serde(default = "default_width")]
    #[schemars(range(min = 320, max = 1920))]
    pub width: u32,
    #[serde(default = "default_height")]
    #[schemars(range(min = 320, max = 2400))]
    pub height: u32,
    #[serde(default = "default_density")]
    #[schemars(range(min = 120, max = 640))]
    pub density: u32,
    #[schemars(schema_with = "packages_schema")]
    pub allowed_apps: Vec<String>,
    #[serde(default)]
    #[schemars(schema_with = "label_schema")]
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchAppParams {
    #[schemars(regex(pattern = "^[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z0-9_]+)+$"))]
    pub package: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotParams {
    #[schemars(length(min = 1, max = 128))]
    pub target_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TapParams {
    #[schemars(length(min = 1, max = 128))]
    pub snapshot_id: String,
    #[schemars(range(min = 0))]
    pub x: f64,
    #[schemars(range(min = 0))]
    pub y: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwipeParams {
    #[schemars(length(min = 1, max = 128))]
    pub snapshot_id: String,
    #[schemars(range(min = 0))]
    pub from_x: f64,
    #[schemars(range(min = 0))]
    pub from_y: f64,
    #[schemars(range(min = 0))]
    pub to_x: f64,
    #[schemars(range(min = 0))]
    pub to_y: f64,
    #[schemars(range(min = 1, max = 1000))]
    pub duration_ms: u32,
}

fn check(ok: bool, message: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn id(value: &str) -> Result<(), String> {
    check(
        (1..=128).contains(&value.chars().count()),
        "IDs must contain 1..128 characters",
    )
}

fn package(value: &str) -> Result<(), String> {
    let mut segments = value.split('.');
    let first = segments.next().unwrap_or_default();
    let valid_segment =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    let mut rest = segments.peekable();
    check(
        first
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && valid_segment(first)
            && rest.peek().is_some()
            && rest.all(valid_segment),
        "invalid Android package",
    )
}

fn coordinates(values: &[f64]) -> Result<(), String> {
    check(
        values.iter().all(|v| v.is_finite() && *v >= 0.0),
        "coordinates must be finite and nonnegative",
    )
}

fn params<P: DeserializeOwned>(value: &Value) -> Result<P, String> {
    serde_json::from_value(value.clone()).map_err(|e| e.to_string())
}

/// Validate the closed wire envelope and operation parameters before dispatch.
pub fn validate_request(value: &Value) -> Result<(), String> {
    let request: Request<Value> = params(value)?;
    let ContractVersion::V0 = request.contract_version;
    id(&request.request_id)?;
    if value.get("session_id").is_some() {
        id(request
            .session_id
            .as_deref()
            .ok_or("session_id must be a string")?)?;
    }
    let op = request.operation.as_str();
    if op == "session.create" {
        check(
            request.session_id.is_none(),
            "session.create cannot reuse session_id",
        )?;
    } else if !matches!(op, "doctor" | "capabilities") {
        check(
            request.session_id.is_some(),
            "session_id is required for this operation",
        )?;
    }
    check(request.params.is_object(), "params must be an object")?;
    match op {
        "doctor" | "capabilities" | "session.inspect" | "session.renew" | "session.stop" => {
            let _: EmptyParams = params(&request.params)?;
        }
        "session.create" => {
            let p: CreateSessionParams = params(&request.params)?;
            check(
                (320..=1920).contains(&p.width)
                    && (320..=2400).contains(&p.height)
                    && (120..=640).contains(&p.density),
                "invalid session geometry",
            )?;
            check(
                (1..=8).contains(&p.allowed_apps.len()),
                "allowed_apps must contain 1..8 packages",
            )?;
            for app in &p.allowed_apps {
                package(app)?;
            }
            if request.params.get("label").is_some() {
                let label = p.label.as_deref().ok_or("label must be a string")?;
                check(
                    label.chars().count() <= 128,
                    "label must contain at most 128 characters",
                )?;
            }
        }
        "app.launch" => package(&params::<LaunchAppParams>(&request.params)?.package)?,
        "snapshot" | "preview" => id(&params::<SnapshotParams>(&request.params)?.target_id)?,
        "tap" => {
            let p: TapParams = params(&request.params)?;
            id(&p.snapshot_id)?;
            coordinates(&[p.x, p.y])?;
        }
        "gesture.swipe" => {
            let p: SwipeParams = params(&request.params)?;
            id(&p.snapshot_id)?;
            coordinates(&[p.from_x, p.from_y, p.to_x, p.to_y])?;
            check(
                (1..=1000).contains(&p.duration_ms),
                "duration_ms must be 1..1000",
            )?;
        }
        _ => return Err("unsupported Android operation".into()),
    }
    Ok(())
}

fn operation_schema<P: JsonSchema>(operation: &str) -> Value {
    // Inline definitions so each closed operation alternative is self-contained.
    let settings =
        schemars::generate::SchemaSettings::default().with(|s| s.inline_subschemas = true);
    let mut schema = serde_json::to_value(
        settings
            .into_generator()
            .into_root_schema_for::<Request<P>>(),
    )
    .unwrap();
    schema.as_object_mut().unwrap().remove("$schema");
    schema["properties"]["operation"] = json!({"type":"string", "const":operation});
    if operation == "session.create" {
        schema["properties"]
            .as_object_mut()
            .unwrap()
            .remove("session_id");
    } else if !matches!(operation, "doctor" | "capabilities") {
        schema["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("session_id"));
    }
    schema
}

/// JSON Schema for Android requests only; not advertised as desktop tools.
pub fn request_schema() -> Value {
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema",
    "title":"Experimental Android request", "oneOf":[
        operation_schema::<EmptyParams>("doctor"),
        operation_schema::<EmptyParams>("capabilities"),
        operation_schema::<CreateSessionParams>("session.create"),
        operation_schema::<EmptyParams>("session.inspect"),
        operation_schema::<EmptyParams>("session.renew"),
        operation_schema::<EmptyParams>("session.stop"),
        operation_schema::<LaunchAppParams>("app.launch"),
        operation_schema::<SnapshotParams>("snapshot"),
        operation_schema::<SnapshotParams>("preview"),
        operation_schema::<TapParams>("tap"),
        operation_schema::<SwipeParams>("gesture.swipe")
    ]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_shared_invalid_wire_requests() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct InvalidRequestCase {
            name: String,
            request: Value,
        }

        let cases: Vec<InvalidRequestCase> = serde_json::from_str(include_str!(
            "../../../../android/contract/invalid-requests.json"
        ))
        .unwrap();
        assert!(
            !cases.is_empty(),
            "shared invalid request cases must not be empty"
        );
        let mut names = std::collections::HashSet::new();
        for case in cases {
            assert!(!case.name.is_empty(), "each case must have a name");
            assert!(
                names.insert(case.name.clone()),
                "duplicate case: {}",
                case.name
            );
            assert_eq!(
                case.request["contract_version"], CONTRACT_VERSION,
                "{}",
                case.name
            );
            assert!(case.request["request_id"].is_string(), "{}", case.name);
            assert!(
                validate_request(&case.request).is_err(),
                "shared invalid request was accepted: {}: {}",
                case.name,
                case.request
            );
        }
    }

    fn request(op: &str, params: Value) -> Value {
        let mut r = json!({"contract_version":CONTRACT_VERSION,"request_id":"r","operation":op,"params":params});
        if !matches!(op, "doctor" | "capabilities" | "session.create") {
            r["session_id"] = json!("s");
        }
        r
    }

    #[test]
    fn supported_operations_and_defaults() {
        for op in [
            "doctor",
            "capabilities",
            "session.inspect",
            "session.renew",
            "session.stop",
        ] {
            validate_request(&request(op, json!({}))).unwrap();
        }
        validate_request(&request(
            "session.create",
            json!({"allowed_apps":["ai.example"]}),
        ))
        .unwrap();
        let defaults: CreateSessionParams =
            params(&json!({"allowed_apps":["ai.example"]})).unwrap();
        assert_eq!(
            (defaults.width, defaults.height, defaults.density),
            (1080, 1920, 320)
        );
        validate_request(&request("app.launch", json!({"package":"ai.example"}))).unwrap();
        for op in ["snapshot", "preview"] {
            validate_request(&request(op, json!({"target_id":"t"}))).unwrap();
        }
        validate_request(&request("tap", json!({"snapshot_id":"snap","x":0.5,"y":0}))).unwrap();
        validate_request(&request("gesture.swipe", json!({"snapshot_id":"snap","from_x":0,"from_y":0,"to_x":1,"to_y":2,"duration_ms":1000}))).unwrap();
    }

    #[test]
    fn rejects_malformed_envelopes_and_missing_sessions() {
        for (field, value) in [
            ("contract_version", json!("v1")),
            ("request_id", json!("")),
            ("request_id", json!("x".repeat(129))),
            ("session_id", Value::Null),
            ("session_id", json!("")),
            ("params", json!([])),
            ("extra", json!(true)),
            ("operation", json!("unsupported")),
        ] {
            let mut r = request("doctor", json!({}));
            r[field] = value;
            assert!(validate_request(&r).is_err(), "{r}");
        }
        for op in [
            "session.inspect",
            "session.renew",
            "session.stop",
            "app.launch",
            "snapshot",
            "preview",
            "tap",
            "gesture.swipe",
        ] {
            let mut r = request(op, json!({}));
            r.as_object_mut().unwrap().remove("session_id");
            assert!(validate_request(&r).unwrap_err().contains("session_id"));
        }
        let mut r = request("session.create", json!({"allowed_apps":["ai.example"]}));
        r["session_id"] = json!("s");
        assert!(validate_request(&r).is_err());
    }

    #[test]
    fn rejects_nested_types_extra_fields_and_out_of_range_values() {
        for (field, value) in [
            ("width", json!(319)),
            ("height", json!(2401)),
            ("density", json!(119)),
            ("width", json!(1080.5)),
            ("width", json!("1080")),
            ("height", Value::Null),
            ("allowed_apps", json!([])),
            ("allowed_apps", json!([false])),
            ("allowed_apps", json!(["invalid"])),
            ("allowed_apps", json!(vec!["ai.example"; 9])),
            ("label", Value::Null),
            ("label", json!({})),
            ("label", json!("x".repeat(129))),
            ("extra", json!(1)),
        ] {
            let mut p = json!({"allowed_apps":["ai.example"]});
            p[field] = value;
            assert!(
                validate_request(&request("session.create", p.clone())).is_err(),
                "{p}"
            );
        }
        for p in [
            json!({"snapshot_id":"s","x":"1","y":0}),
            json!({"snapshot_id":{},"x":1,"y":0}),
            json!({"snapshot_id":"s","x":-1,"y":0}),
            json!({"snapshot_id":"s","x":null,"y":0}),
            json!({"snapshot_id":"s","x":1,"y":0,"extra":1}),
        ] {
            assert!(validate_request(&request("tap", p)).is_err());
        }
        for duration in [json!(0), json!(1001), json!(1.5), json!("10")] {
            assert!(validate_request(&request("gesture.swipe", json!({"snapshot_id":"s","from_x":0,"from_y":0,"to_x":1,"to_y":1,"duration_ms":duration}))).is_err());
        }
        assert!(coordinates(&[f64::NAN]).is_err());
        assert!(coordinates(&[f64::INFINITY]).is_err());
        for op in ["doctor", "session.inspect"] {
            assert!(validate_request(&request(op, json!({"extra":1}))).is_err());
        }
        for name in [
            "",
            "single",
            "1ai.example",
            "ai..example",
            "ai.example.",
            "ai.ex-ample",
            "ai.\u{e9}xample",
        ] {
            assert!(package(name).is_err());
        }
        for name in ["ai.example", "A_1.2._"] {
            package(name).unwrap();
        }
    }

    #[test]
    fn schema_closes_each_operation_and_records_constraints() {
        let schema = request_schema();
        let variants = schema["oneOf"].as_array().unwrap();
        assert_eq!(variants.len(), 11);
        for variant in variants {
            assert_eq!(variant["additionalProperties"], false);
            assert_eq!(
                variant["properties"]["params"]["additionalProperties"],
                false
            );
            assert!(variant.get("$defs").is_none());
        }
        let create = &variants[2];
        assert!(create["properties"].get("session_id").is_none());
        assert_eq!(
            create["properties"]["params"]["properties"]["width"]["default"],
            1080
        );
        assert_eq!(
            create["properties"]["params"]["properties"]["allowed_apps"]["minItems"],
            1
        );
        assert!(variants[3]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("session_id")));
    }
}
